//! Between the window and the service. A call on the pipe can wait, and the window's thread must
//! never wait, so every call is made on a thread of its own and its answer comes back to the window
//! as a message. One thread asks how things stand every two seconds and makes the calls that have one
//! answer. `Create` and `Watch` answer for as long as their work lasts, so each has a thread too, and
//! the asking goes on meanwhile.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use guesthouse_control::client::{self, Unreached};
use guesthouse_control::{Answer, CardInfo, Machine, NewMachine, Request};
use windows_reactor::ComponentCompletion;

use crate::app::Message;

/// How often the service is asked how things stand.
const EVERY: Duration = Duration::from_secs(2);
/// How often, and for how long, a watch that's no longer wanted has its read cancelled.
const STOP_APART: Duration = Duration::from_millis(50);
const STOP_TRIES: u32 = 100;

/// What the service last said about its machines and cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub version: String,
    pub machines: Vec<Machine>,
    pub cards: Vec<CardInfo>,
}

/// How things stand, or the words for why that isn't known.
pub type Seen = Result<Status, String>;

/// A call with one answer, for a machine named in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    Start(String),
    Stop(String),
    Delete(String),
    Account(String),
    /// What remoteman's viewer needs to show a Windows guest's screen.
    Screen(String),
}

impl Job {
    fn request(&self) -> Request {
        match self.clone() {
            Job::Start(name) => Request::Start { name },
            Job::Stop(name) => Request::Stop { name },
            Job::Delete(name) => Request::Delete { name },
            Job::Account(name) => Request::Account { name },
            Job::Screen(name) => Request::Screen { name },
        }
    }
}

/// Where messages go to the window, from any thread.
#[derive(Clone)]
pub struct Window(pub ComponentCompletion<Message>);

impl Window {
    /// Returns `false` once the window has gone, so the thread sending it can end.
    pub fn tell(&self, message: Message) -> bool {
        self.0.clone().complete(message)
    }
}

/// Asks how things stand.
pub fn status() -> Seen {
    match client::ask(&Request::Status) {
        Ok(Answer::Status { version, machines, cards }) => Ok(Status { version, machines, cards }),
        Ok(Answer::Refused { reason }) => Err(reason),
        Ok(_) => Err(Unreached::Garbled.words().to_owned()),
        Err(unreached) => Err(unreached.words().to_owned()),
    }
}

/// Starts the thread that asks how things stand and makes the calls with one answer. Returns where
/// to send those calls.
pub fn start(window: Window) -> Sender<Job> {
    let (jobs, inbox) = mpsc::channel::<Job>();
    let failed = window.clone();
    let spawned = thread::Builder::new().name("service".into()).spawn(move || {
        loop {
            match inbox.recv_timeout(EVERY) {
                Ok(job) => {
                    let answer = client::ask(&job.request());
                    if !window.tell(Message::Answered(job, answer)) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // After a call too, so what it changed shows at once.
            if !window.tell(Message::Status(status())) {
                return;
            }
        }
    });
    if let Err(error) = spawned {
        // Nothing would ever be asked, which the window can only show as a service that's away.
        let _ =
            failed.tell(Message::Status(Err(format!("The app couldn't start asking the service: {error}"))));
    }
    jobs
}

/// Makes a new machine, sending the window each of its events and then how it ended.
pub fn create(machine: NewMachine, window: Window) {
    let failed = window.clone();
    let spawned = thread::Builder::new().name("create".into()).spawn(move || {
        let mut ended = None;
        let streamed = client::stream(&Request::Create { machine }, |answer| match answer {
            Answer::Event { event } => window.tell(Message::Creating(event)),
            Answer::Done { reason } => {
                ended = Some(reason);
                false
            }
            Answer::Refused { reason } => {
                ended = Some(Some(reason));
                false
            }
            // Nothing else belongs here. Leave it, and keep listening for the end.
            _ => true,
        });
        let ended = match (ended, streamed) {
            (Some(reason), _) => reason,
            (None, Err(unreached)) => Some(unreached.words().to_owned()),
            (None, Ok(())) => Some("The service stopped before it said how the work ended.".to_owned()),
        };
        let _ = window.tell(Message::Created(ended));
    });
    if let Err(error) = spawned {
        let _ = failed.tell(Message::Created(Some(format!("The app couldn't start the work: {error}"))));
    }
}

/// A machine's events, coming in on a thread of their own. Dropping it ends them.
pub struct Watch {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Starts watching a machine. Its messages carry `id`, so the window can tell them from the
/// messages of a watch it has let go of that were already on their way.
pub fn watch(id: u64, name: String, window: Window) -> Watch {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let failed = window.clone();
    let spawned = thread::Builder::new().name("watch".into()).spawn(move || {
        let mut refused = None;
        let streamed = client::stream(&Request::Watch { name }, |answer| {
            if stopped.load(Ordering::Relaxed) {
                return false;
            }
            match answer {
                Answer::Event { event } => window.tell(Message::Watched(id, event)),
                Answer::Refused { reason } => {
                    refused = Some(reason);
                    false
                }
                _ => true,
            }
        });
        if stopped.load(Ordering::Relaxed) {
            return;
        }
        let why = match (refused, streamed) {
            (Some(reason), _) => Ended::Refused(reason),
            (None, Err(unreached)) => Ended::Unreached(unreached.words().to_owned()),
            (None, Ok(())) => Ended::Closed,
        };
        let _ = window.tell(Message::WatchEnded(id, why));
    });
    match spawned {
        Ok(thread) => Watch { stop, thread: Some(thread) },
        Err(error) => {
            let why = Ended::Unreached(format!("The app couldn't start watching: {error}"));
            let _ = failed.tell(Message::WatchEnded(id, why));
            Watch { stop, thread: None }
        }
    }
}

/// Why a watch ended without being let go of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The service said no, and why.
    Refused(String),
    /// The service went away or garbled its answer.
    Unreached(String),
    /// The service closed the pipe, as when it stops.
    Closed,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let Some(thread) = self.thread.take() else { return };
        // The thread is most likely waiting on the pipe for an event that may not come for a long
        // time, as a stopped machine has nothing to say. Cancelling the read ends it at once, and
        // the pipe closes with it, so the service lets go of its end too. A read can only be
        // cancelled once it has begun, and the thread may be just before one, so this is done again
        // until the thread has gone. If it somehow outlasts the tries, it's left to end at the
        // machine's next event, which it won't pass on.
        let _ = thread::Builder::new().name("watch stop".into()).spawn(move || {
            for _ in 0..STOP_TRIES {
                if thread.is_finished() {
                    return;
                }
                crate::system::cancel_read(&thread);
                thread::sleep(STOP_APART);
            }
        });
    }
}
