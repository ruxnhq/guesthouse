//! The connection to one machine's helper, kept by a thread of its own so that nothing waits for it.
//! The thread connects, sends the token, waits for `hello`, passes on what the helper says, sends
//! requests and matches their answers, and when the connection goes it connects again. A forward
//! through QEMU accepts a connection whether or not anything listens behind it, so "nothing is
//! there" and "the token was wrong" look the same: the connection closes, or says nothing. Either
//! way the helper is started through the agent, no more often than [`Timing::start_cooldown`].
//!
//! [`super::Client`] is the end that the rest of the program holds. This is the thread behind it.

use super::message::{Answer, parse_guest_line};
use super::message::{
    GuestMessage, LineProblem, MOST_APPS_LINE_BYTES, MOST_LINE_BYTES, Request, side_token_line, token_line,
};
use crate::clock::Clock;
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

/// How the link stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Trying, or waiting to try again.
    Connecting,
    /// The helper has said hello.
    Up,
    /// Why there is no helper to talk to, in words.
    Down(String),
}

/// Why a request got no answer from the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestFailure {
    /// The link was not up when it was sent, so nothing was sent: a request is about what is on the
    /// screen now, and one sent later would act on what is not.
    NotConnected,
    /// The helper did not answer in time. It may yet have done it.
    TimedOut,
    /// The connection went before the answer came. It may yet have been done.
    ConnectionLost,
}

impl std::fmt::Display for RequestFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotConnected => "the helper in the guest is not connected",
            Self::TimedOut => "the helper in the guest did not answer in time",
            Self::ConnectionLost => "the connection to the helper in the guest was lost before it answered",
        })
    }
}

/// What starts the helper in the guest when nothing answers. It may take long, and it runs on a thread
/// of its own.
pub trait HelperStarter: Send + Sync {
    /// `Err` says in words why the helper cannot be started, which is shown as why the link is down.
    fn start(&self) -> std::result::Result<(), String>;
}

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub connect: Duration,
    /// How long a helper may take over `hello` once it has the token.
    pub hello: Duration,
    pub write: Duration,
    /// The wait before the first new try, which doubles up to `reconnect_most`. A second client of the
    /// same machine takes the connection from the first, and this is what keeps them from taking it
    /// back at once, for ever.
    pub reconnect_first: Duration,
    pub reconnect_most: Duration,
    pub start_cooldown: Duration,
    pub request: Duration,
    /// Listing the Start menu with its icons takes the helper longer than anything else it does.
    pub apps_request: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(2),
            hello: Duration::from_secs(5),
            write: Duration::from_secs(5),
            reconnect_first: Duration::from_millis(500),
            reconnect_most: Duration::from_secs(5),
            start_cooldown: Duration::from_secs(30),
            request: Duration::from_secs(10),
            apps_request: Duration::from_secs(30),
        }
    }
}

/// What a process needs to reach helpers, as one value that a test can fill.
#[derive(Debug, Clone, Copy)]
pub struct LinkSettings {
    /// A port on this PC to use for every machine, in place of the slot's own.
    pub port: Option<u16>,
    pub timing: Timing,
    /// How long a command waits for the helper to come up, which includes starting it in the guest.
    pub patience: Duration,
}

impl Default for LinkSettings {
    fn default() -> Self {
        Self { port: None, timing: Timing::default(), patience: Duration::from_secs(60) }
    }
}

pub struct LinkConfig {
    /// On this PC, where the machine's forward is.
    pub port: u16,
    pub token: String,
    /// A side connection, which does not take the helper's driving connection from the viewer.
    pub side: bool,
    pub starter: Option<Arc<dyn HelperStarter>>,
    pub timing: Timing,
    /// What [`Timing::start_cooldown`] is measured by.
    pub clock: Arc<dyn Clock>,
}

/// What the thread tells the client.
#[derive(Debug)]
pub(super) enum Event {
    Status(Status),
    /// A message of the helper's, with the line it was read from if it is a layout.
    Message {
        message: GuestMessage,
        raw: Option<String>,
    },
    Answered {
        n: u64,
        result: Result<Answer, RequestFailure>,
    },
    /// Something for a log: a line that could not be read, a start that failed.
    Note(String),
}

/// What the thread is told, by the client and by its own helpers.
pub(super) enum Input {
    Send { n: u64, request: Request },
    Stop,
    Line { connection: u64, text: String },
    Unreadable { connection: u64, why: String },
    Closed { connection: u64, why: String },
    Started(std::result::Result<(), String>),
}

struct Connection {
    id: u64,
    stream: TcpStream,
    /// How long a line may be now, which the thread that reads it needs to know.
    limit: Arc<AtomicUsize>,
    said_hello: bool,
    hello_by: Instant,
}

struct Pending {
    by: Instant,
    long: bool,
}

pub(super) fn spawn(config: LinkConfig, inputs: Receiver<Input>, own: Sender<Input>, events: Sender<Event>) {
    let link = Link {
        config,
        own,
        events,
        connection: None,
        next_id: 0,
        pending: HashMap::new(),
        next_try: Instant::now(),
        wait: Duration::ZERO,
        starting: false,
        last_start: None,
        status: Status::Connecting,
        up_since: None,
        why_down: String::new(),
        restarted_for_version: false,
    };
    thread::spawn(move || link.run(&inputs));
}

struct Link {
    config: LinkConfig,
    own: Sender<Input>,
    events: Sender<Event>,
    connection: Option<Connection>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    next_try: Instant,
    wait: Duration,
    starting: bool,
    /// By the config's clock.
    last_start: Option<Duration>,
    status: Status,
    /// When the helper last said hello. A connection that lasted is no reason to wait long to try again.
    up_since: Option<Instant>,
    why_down: String,
    /// A helper of another version is restarted once, as the one that has it may be a leftover of an
    /// older program. If it still says another, nothing more is done for it.
    restarted_for_version: bool,
}

impl Link {
    fn run(mut self, inputs: &Receiver<Input>) {
        self.set_status(Status::Connecting);
        loop {
            let wake = self.next_wake();
            let input = match inputs.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(input) => Some(input),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            match input {
                Some(Input::Stop) => break,
                Some(input) => self.handle(input),
                None => {}
            }
            self.expire();
            if self.connection.is_none() && !self.starting && Instant::now() >= self.next_try {
                self.connect();
            }
        }
        self.drop_connection();
    }

    fn next_wake(&self) -> Instant {
        let far = Instant::now() + Duration::from_secs(3600);
        let mut wake = far;
        if let Some(connection) = &self.connection {
            if !connection.said_hello {
                wake = wake.min(connection.hello_by);
            }
        } else if !self.starting {
            wake = wake.min(self.next_try);
        }
        for pending in self.pending.values() {
            wake = wake.min(pending.by);
        }
        wake
    }

    fn tell(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn set_status(&mut self, status: Status) {
        if self.status != status {
            self.status = status.clone();
            self.tell(Event::Status(status));
        }
    }

    fn handle(&mut self, input: Input) {
        match input {
            Input::Stop => {}
            Input::Send { n, request } => self.send(n, &request),
            Input::Line { connection, text } if self.is_current(connection) => self.line(&text),
            Input::Unreadable { connection, why } if self.is_current(connection) => {
                self.tell(Event::Note(format!("A line from the helper was left out: {why}.")));
            }
            Input::Closed { connection, why } if self.is_current(connection) => self.lost(&why),
            Input::Line { .. } | Input::Unreadable { .. } | Input::Closed { .. } => {}
            Input::Started(result) => self.started(result),
        }
    }

    fn is_current(&self, id: u64) -> bool {
        self.connection.as_ref().is_some_and(|connection| connection.id == id)
    }

    fn send(&mut self, n: u64, request: &Request) {
        let up = self.connection.as_ref().is_some_and(|connection| connection.said_hello);
        if !up {
            self.tell(Event::Answered { n, result: Err(RequestFailure::NotConnected) });
            return;
        }
        let long = request.answer_is_long();
        let patience = if long { self.config.timing.apps_request } else { self.config.timing.request };
        let mut line = request.line(n);
        line.push('\n');
        let sent = self
            .connection
            .as_mut()
            .is_some_and(|connection| connection.stream.write_all(line.as_bytes()).is_ok());
        if !sent {
            self.tell(Event::Answered { n, result: Err(RequestFailure::ConnectionLost) });
            self.lost("the helper could not be written to");
            return;
        }
        self.pending.insert(n, Pending { by: Instant::now() + patience, long });
        self.set_limit();
    }

    /// An answer to `apps` may be much longer than any other line.
    fn set_limit(&self) {
        let most = if self.pending.values().any(|pending| pending.long) {
            MOST_APPS_LINE_BYTES
        } else {
            MOST_LINE_BYTES
        };
        if let Some(connection) = &self.connection {
            connection.limit.store(most, Ordering::Relaxed);
        }
    }

    fn line(&mut self, text: &str) {
        let limit = self.connection.as_ref().map_or(MOST_LINE_BYTES, |c| c.limit.load(Ordering::Relaxed));
        let message = match parse_guest_line(text, limit) {
            Ok(message) => message,
            Err(LineProblem::Version(found)) => return self.another_version(found),
            Err(problem) => {
                self.tell(Event::Note(format!("A line from the helper was left out: {problem}.")));
                return;
            }
        };
        match message {
            GuestMessage::Answer(answer) => match self.pending.remove(&answer.re) {
                Some(_) => {
                    self.tell(Event::Answered { n: answer.re, result: Ok(answer) });
                    self.set_limit();
                }
                None => self
                    .tell(Event::Note(format!("An answer to request {} that nobody waits for.", answer.re))),
            },
            GuestMessage::Hello(hello) => {
                if let Some(connection) = &mut self.connection {
                    connection.said_hello = true;
                }
                self.up_since = Some(Instant::now());
                self.why_down.clear();
                self.restarted_for_version = false;
                self.tell(Event::Message { message: GuestMessage::Hello(hello), raw: None });
                self.set_status(Status::Up);
            }
            layout @ GuestMessage::Layout(_) => {
                self.tell(Event::Message { message: layout, raw: Some(text.to_owned()) });
            }
            other => self.tell(Event::Message { message: other, raw: None }),
        }
    }

    fn another_version(&mut self, found: u64) {
        let why = format!(
            "The helper in the guest speaks version {found} of the messages, which this program does not."
        );
        self.drop_connection();
        self.why_down.clone_from(&why);
        self.set_status(Status::Down(why));
        self.schedule_retry();
        if !self.restarted_for_version {
            self.restarted_for_version = true;
            self.start_helper();
        }
    }

    /// The connection ended, or never got as far as `hello`.
    fn lost(&mut self, why: &str) {
        let said_hello = self.connection.as_ref().is_some_and(|connection| connection.said_hello);
        if self.up_since.take().is_some_and(|since| since.elapsed() >= self.config.timing.reconnect_most) {
            self.wait = Duration::ZERO;
        }
        self.drop_connection();
        let reason = if said_hello {
            format!("The connection to the helper in the guest was lost: {why}.")
        } else {
            format!("Nothing in the guest answered: {why}.")
        };
        // A start that failed, or a helper of another version, says more than that nothing answered.
        let shown = if self.why_down.is_empty() { reason } else { self.why_down.clone() };
        self.set_status(Status::Down(shown));
        self.schedule_retry();
        if !said_hello {
            self.start_helper();
        }
    }

    fn drop_connection(&mut self) {
        if let Some(connection) = self.connection.take() {
            let _ = connection.stream.shutdown(Shutdown::Both);
        }
        let lost: Vec<u64> = self.pending.keys().copied().collect();
        self.pending.clear();
        for n in lost {
            self.tell(Event::Answered { n, result: Err(RequestFailure::ConnectionLost) });
        }
    }

    fn schedule_retry(&mut self) {
        let timing = self.config.timing;
        self.wait = if self.wait.is_zero() {
            timing.reconnect_first
        } else {
            (self.wait * 2).min(timing.reconnect_most)
        };
        self.next_try = Instant::now() + self.wait;
    }

    fn expire(&mut self) {
        let now = Instant::now();
        let late: Vec<u64> =
            self.pending.iter().filter(|(_, pending)| pending.by <= now).map(|(n, _)| *n).collect();
        for n in late {
            self.pending.remove(&n);
            self.tell(Event::Answered { n, result: Err(RequestFailure::TimedOut) });
        }
        self.set_limit();
        let no_hello = self.connection.as_ref().is_some_and(|c| !c.said_hello && c.hello_by <= now);
        if no_hello {
            self.lost("it did not say hello");
        }
    }

    fn connect(&mut self) {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.config.port));
        let timing = self.config.timing;
        let opened = TcpStream::connect_timeout(&address, timing.connect).and_then(|stream| {
            stream.set_write_timeout(Some(timing.write))?;
            stream.set_nodelay(true)?;
            let mut writer = stream.try_clone()?;
            let first = if self.config.side {
                side_token_line(&self.config.token)
            } else {
                token_line(&self.config.token)
            };
            writer.write_all(first.as_bytes())?;
            Ok(stream)
        });
        let stream = match opened {
            Ok(stream) => stream,
            Err(error) => {
                let why = format!("{error}");
                self.lost(&why);
                return;
            }
        };
        self.next_id += 1;
        let limit = Arc::new(AtomicUsize::new(MOST_LINE_BYTES));
        let id = self.next_id;
        let Ok(reading) = stream.try_clone() else { return self.lost("the connection could not be shared") };
        let (own, limit_for_reader) = (self.own.clone(), limit.clone());
        thread::spawn(move || read_lines(reading, id, &limit_for_reader, &own));
        self.connection = Some(Connection {
            id,
            stream,
            limit,
            said_hello: false,
            hello_by: Instant::now() + timing.hello,
        });
    }

    /// Starts the helper in the guest, unless one start is going or one was tried just now.
    fn start_helper(&mut self) {
        let Some(starter) = self.config.starter.clone() else { return };
        let now = self.config.clock.now();
        let cooled =
            self.last_start.is_none_or(|at| now.saturating_sub(at) >= self.config.timing.start_cooldown);
        if self.starting || !cooled {
            return;
        }
        self.starting = true;
        self.last_start = Some(now);
        let own = self.own.clone();
        thread::spawn(move || {
            let _ = own.send(Input::Started(starter.start()));
        });
    }

    fn started(&mut self, result: std::result::Result<(), String>) {
        self.starting = false;
        match result {
            Ok(()) => {
                self.why_down.clear();
                self.wait = Duration::ZERO;
                self.next_try = Instant::now();
            }
            Err(why) => {
                self.tell(Event::Note(why.clone()));
                self.why_down.clone_from(&why);
                self.set_status(Status::Down(why));
                self.schedule_retry();
            }
        }
    }
}

/// Reads lines until the connection ends. A line longer than the limit is read past without being
/// kept, so a helper gone wrong cannot make this program hold what it likes.
fn read_lines(stream: TcpStream, connection: u64, limit: &AtomicUsize, to: &Sender<Input>) {
    let mut reader = BufReader::new(stream);
    let mut line: Vec<u8> = Vec::new();
    let mut skipped: usize = 0;
    let ended = loop {
        let chunk = match reader.fill_buf() {
            Ok([]) => break "the helper closed the connection".to_owned(),
            Ok(chunk) => chunk,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => break error.to_string(),
        };
        let (taken, complete) = match chunk.iter().position(|byte| *byte == b'\n') {
            Some(at) => (at + 1, true),
            None => (chunk.len(), false),
        };
        let most = limit.load(Ordering::Relaxed);
        if skipped > 0 || line.len() + taken > most + 1 {
            if skipped == 0 {
                skipped = line.len();
                line.clear();
            }
            skipped += taken;
        } else {
            line.extend_from_slice(&chunk[..taken]);
        }
        reader.consume(taken);
        if !complete {
            continue;
        }
        let input = if skipped > 0 {
            let length = std::mem::take(&mut skipped) - 1;
            Input::Unreadable { connection, why: LineProblem::TooLong { length, most }.to_string() }
        } else {
            line.pop();
            match String::from_utf8(std::mem::take(&mut line)) {
                Ok(text) => Input::Line { connection, text },
                Err(_) => Input::Unreadable { connection, why: "a line that is not UTF-8".to_owned() },
            }
        };
        line.clear();
        if to.send(input).is_err() {
            return;
        }
    };
    let _ = to.send(Input::Closed { connection, why: ended });
}

/// The pieces a [`super::Client`] is made of.
pub(super) fn channels() -> (Sender<Input>, Receiver<Input>, Sender<Event>, Receiver<Event>) {
    let (inputs, inputs_rx) = channel();
    let (events, events_rx) = channel();
    (inputs, inputs_rx, events, events_rx)
}
