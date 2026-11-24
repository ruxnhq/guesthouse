//! What the rest of the program holds of the link to a machine's helper: a model of its cells that is
//! kept up as messages arrive, and requests whose answers come back. Nothing here waits unless it is
//! asked to. A GTK main loop calls [`Client::poll`] from a timeout, as the window-size helper's
//! sizes are read, and acts on what it returns. The command line calls [`Client::ask`] and
//! [`Client::wait_for`], which block for as long as they are told and no longer.

use super::link::{Event, Input, LinkConfig, RequestFailure, Status, channels, spawn};
use super::message::{Answer, GuestMessage, Request};
use super::model::{Change, Model};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// What a call to [`Client::poll`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Status(Status),
    /// The model has taken these changes already: it says how things stand, and this what moved.
    Changes(Vec<Change>),
    /// An answer, or why there is none, to the request that was given this number.
    Answered {
        n: u64,
        result: Result<Answer, RequestFailure>,
    },
    /// For the log.
    Note(String),
}

pub struct Client {
    inputs: Sender<Input>,
    events: Receiver<Event>,
    model: Model,
    status: Status,
    next_n: u64,
    last_layout: Option<String>,
    /// What came in while a call was waiting for something else, for the next to return.
    held: VecDeque<Update>,
}

impl Client {
    /// Starts the link. It connects, and starts the helper if nothing answers and the config has what
    /// starts one, without this call or any other waiting for it.
    pub fn start(config: LinkConfig) -> Self {
        let (inputs, inputs_rx, events_tx, events) = channels();
        spawn(config, inputs_rx, inputs.clone(), events_tx);
        Self {
            inputs,
            events,
            model: Model::new(),
            status: Status::Connecting,
            next_n: 1,
            last_layout: None,
            held: VecDeque::new(),
        }
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// The helper's last layout line, as it came.
    pub fn last_layout_line(&self) -> Option<&str> {
        self.last_layout.as_deref()
    }

    /// Sends a request and returns its number at once. The answer comes as [`Update::Answered`], and
    /// when the link is down so soon as the thread has looked, as an [`Update::Answered`] that says why not.
    pub fn request(&mut self, request: Request) -> u64 {
        let n = self.next_n;
        self.next_n += 1;
        let _ = self.inputs.send(Input::Send { n, request });
        n
    }

    /// Everything that has come in, without waiting.
    pub fn poll(&mut self) -> Vec<Update> {
        let mut found: Vec<Update> = self.held.drain(..).collect();
        while let Ok(event) = self.events.try_recv() {
            found.extend(self.take(event));
        }
        found
    }

    fn take(&mut self, event: Event) -> Vec<Update> {
        match event {
            Event::Status(status) => {
                self.status = status.clone();
                vec![Update::Status(status)]
            }
            Event::Note(text) => vec![Update::Note(text)],
            Event::Answered { n, result } => vec![Update::Answered { n, result }],
            Event::Message { message, raw } => self.apply(message, raw),
        }
    }

    fn apply(&mut self, message: GuestMessage, raw: Option<String>) -> Vec<Update> {
        let mut updates = Vec::new();
        let changes = match message {
            GuestMessage::Hello(hello) => self.model.hello(&hello),
            GuestMessage::Layout(layout) => {
                self.last_layout = raw;
                let applied = self.model.layout(&layout);
                if applied.seq_went_back {
                    updates.push(Update::Note(format!("The helper's layouts went back to {}.", layout.seq)));
                }
                applied.changes
            }
            GuestMessage::Desktop(desktop) => self.model.desktop(desktop),
            GuestMessage::Answer(_) | GuestMessage::Other(_) => Vec::new(),
        };
        if !changes.is_empty() {
            updates.push(Update::Changes(changes));
        }
        updates
    }

    /// Waits for the next thing to come in, for as long as `patience`. Nothing is returned if nothing came.
    fn wait_one(&mut self, until: Instant) -> Option<Vec<Update>> {
        let left = until.saturating_duration_since(Instant::now());
        match self.events.recv_timeout(left) {
            Ok(event) => Some(self.take(event)),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
        }
    }

    /// Waits until `done` is satisfied by what the model and the status say, or `patience` is up.
    /// Whatever came in meanwhile is kept for the next [`Client::poll`]. Says whether it was.
    pub fn wait_for(&mut self, patience: Duration, mut done: impl FnMut(&Self) -> bool) -> bool {
        let until = Instant::now() + patience;
        let mut stash = Vec::new();
        let ok = loop {
            if done(self) {
                break true;
            }
            match self.wait_one(until) {
                Some(updates) => stash.extend(updates),
                None => break done(self),
            }
        };
        self.held.extend(stash);
        ok
    }

    /// Sends a request and waits for its answer. For the command line: a viewer sends [`Client::request`]
    /// and goes on.
    pub fn ask(&mut self, request: Request, patience: Duration) -> Result<Answer, RequestFailure> {
        let n = self.request(request);
        let until = Instant::now() + patience;
        let mut stash = Vec::new();
        let result = loop {
            let Some(updates) = self.wait_one(until) else { break Err(RequestFailure::TimedOut) };
            let mut mine = None;
            for update in updates {
                match update {
                    Update::Answered { n: of, result } if of == n => mine = Some(result),
                    other => stash.push(other),
                }
            }
            if let Some(result) = mine {
                break result;
            }
        };
        self.held.extend(stash);
        result
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.inputs.send(Input::Stop);
    }
}
