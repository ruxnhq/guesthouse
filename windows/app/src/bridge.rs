//! Between the window and the service. A call on the pipe can wait, and the window's thread must
//! never wait, so every call is made on a thread of its own and its answer comes back to the window
//! as a message. One thread asks how things stand every two seconds and makes the calls that have one
//! answer. `Create` and `Watch` answer for as long as their work lasts, so each has a thread too, and
//! the asking goes on meanwhile.
//!
//! Each message is followed by a render of the whole window, so the window is told only what it
//! needs: how things stand only when that has changed, and a machine's events a few times a second
//! at most, however fast its guest writes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use guesthouse_control::client::{self, Unreached};
use guesthouse_control::updates::Latest;
use guesthouse_control::{
    Answer, Bridging, CardInfo, Event, Machine, MachineSettings, NewMachine, Request, ShareInfo,
};
use windows_reactor::ComponentCompletion;

use crate::app::Message;
use crate::pace::{Batch, Changes};
use crate::remote_view::{self, Ask};
use crate::update::{DownloadError, Updater};

/// How often the service is asked how things stand.
const EVERY: Duration = Duration::from_secs(2);
/// How often, and for how long, a watch that's no longer wanted has its read cancelled.
const STOP_APART: Duration = Duration::from_millis(50);
const STOP_TRIES: u32 = 100;
/// The least time between two messages of a machine's events. Long enough that a guest writing as
/// fast as it can costs the window a few renders a second, short enough that the log looks live.
const EVENTS_APART: Duration = Duration::from_millis(200);

/// What the service last said about its machines and cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub version: String,
    pub machines: Vec<Machine>,
    pub cards: Vec<CardInfo>,
    pub bridging: Bridging,
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
    /// What a stopped machine is given at each boot from now on.
    Change(String, MachineSettings),
    /// Let a stopped machine's disk grow to this many GB.
    Grow(String, u64),
    /// Take a snapshot with this label.
    Snapshot(String, String),
    /// Go back to the snapshot with this ID.
    Revert(String, u64),
    DeleteSnapshot(String, u64),
    /// Move a stopped machine's disks to a folder, or with an empty one back to the service's own.
    Move(String, PathBuf),
}

impl Job {
    fn request(&self) -> Request {
        match self.clone() {
            Job::Change(name, settings) => Request::Change { name, settings },
            Job::Start(name) => Request::Start { name },
            Job::Stop(name) => Request::Stop { name },
            Job::Delete(name) => Request::Delete { name },
            Job::Account(name) => Request::Account { name },
            Job::Screen(name) => Request::Screen { name },
            Job::Grow(name, size_gb) => Request::Grow { name, size_gb },
            Job::Snapshot(name, label) => Request::Snapshot { name, label },
            Job::Revert(name, id) => Request::Revert { name, id },
            Job::DeleteSnapshot(name, id) => Request::DeleteSnapshot { name, id },
            Job::Move(name, folder) => Request::Move { name, folder },
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
        Ok(Answer::Status { version, machines, cards, bridging, .. }) => {
            Ok(Status { version, machines, cards, bridging })
        }
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
        // What the window was last told, as an answer that's the same again changes nothing there.
        // The first answer goes, and so does each change between the service being there and not.
        let mut told = Changes::new();
        loop {
            match inbox.recv_timeout(EVERY) {
                Ok(job) => {
                    let answer = client::ask(&job.request());
                    if !window.tell(Message::Answered(job, answer)) {
                        return;
                    }
                    // The window may have changed its own copy for the call, as for Grow, so it's
                    // told how things stand after a call whether or not that has changed.
                    told.forget();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // After a call too, so what it changed shows at once.
            if let Some(seen) = told.news(status())
                && !window.tell(Message::Status(seen))
            {
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

/// Asks the file drop for the newest Windows host through `updater`, on a thread of its own, and
/// hands the window what came back, for the look numbered `attempt`.
pub fn look_for_update(updater: Arc<dyn Updater>, attempt: u64, window: Window) {
    let _ = thread::Builder::new().name("update".into()).spawn(move || {
        let _ = window.tell(Message::Looked(attempt, updater.fetch()));
    });
}

/// How often, at most, the window is told how far a download has got. It's told only when the
/// number it shows changes, too.
const PROGRESS_APART: Duration = Duration::from_millis(500);

/// Downloads an update's installer through `updater` on a thread of its own, as download
/// `attempt`, telling the window how far it's got, now and then, and how it ended. Setting `stop`
/// stops it.
pub fn download_update(
    updater: Arc<dyn Updater>,
    attempt: u64,
    latest: Latest,
    stop: Arc<AtomicBool>,
    window: Window,
) {
    let failed = window.clone();
    let spawned = thread::Builder::new().name("update download".into()).spawn(move || {
        let mut told = Changes::new();
        let mut last: Option<Instant> = None;
        let mut progress = |got: u64, total: Option<u64>| {
            // In whole MB, as it's shown.
            let shown = (got >> 20, total.map(|total| total >> 20));
            let due = last.is_none_or(|last| last.elapsed() >= PROGRESS_APART);
            if due && told.news(shown).is_some() {
                last = Some(Instant::now());
                let _ = window.tell(Message::UpdateProgress(attempt, got, total));
            }
        };
        let ended = updater.download(&latest, attempt, &mut progress, &stop);
        let _ = window.tell(Message::Downloaded(attempt, ended));
    });
    if let Err(error) = spawned {
        let _ = failed.tell(Message::Downloaded(attempt, Err(DownloadError::Other(error.to_string()))));
    }
}

/// Checks download `attempt` against its SHA-256 on a thread of its own, as reading it takes a moment.
pub fn hash_update(updater: Arc<dyn Updater>, attempt: u64, window: Window) {
    let failed = window.clone();
    let spawned = thread::Builder::new().name("update check".into()).spawn(move || {
        let _ = window.tell(Message::Hashed(attempt, updater.hash(attempt)));
    });
    if let Err(error) = spawned {
        let _ = failed.tell(Message::Hashed(attempt, Err(error.to_string())));
    }
}

/// Lets go of what's been downloaded, and deletes it.
pub fn discard_update(updater: Arc<dyn Updater>) {
    let _ = thread::Builder::new().name("update discard".into()).spawn(move || updater.discard());
}

/// Starts install `attempt`'s installer on a thread of its own, which then waits for it, telling
/// the window that it started and how it ended. First it does what the app does as it goes, as the
/// installer ends the app once it has stopped the service: it closes a pairing the app left open,
/// which is nothing to a preview.
pub fn launch_update(updater: Arc<dyn Updater>, attempt: u64, window: Window) {
    let failed = window.clone();
    let spawned = thread::Builder::new().name("update install".into()).spawn(move || {
        close_pairing_left_open();
        match updater.launch(attempt) {
            Err(why) => {
                let _ = window.tell(Message::Launched(attempt, Err(why)));
            }
            Ok(running) => {
                let _ = window.tell(Message::Launched(attempt, Ok(())));
                let code = running.wait();
                let _ = window.tell(Message::InstallerEnded(attempt, code));
            }
        }
    });
    if let Err(error) = spawned {
        let _ = failed.tell(Message::Launched(attempt, Err(error.to_string())));
    }
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

/// Asks the service to turn Windows' Hyper-V feature on, on a thread of its own as it takes
/// minutes, sending the window how far it has got and then how it ended: `None` when it worked.
pub fn enable_hyper_v(window: Window) {
    let failed = window.clone();
    let spawned = thread::Builder::new().name("hyper-v".into()).spawn(move || {
        let mut ended = None;
        let streamed = client::stream(&Request::EnableHyperV, |answer| match answer {
            Answer::Event { .. } => window.tell(Message::HyperVNote),
            Answer::Done { reason } => {
                ended = Some(reason);
                false
            }
            Answer::Refused { reason } => {
                ended = Some(Some(reason));
                false
            }
            _ => true,
        });
        let ended = match (ended, streamed) {
            (Some(reason), _) => reason,
            (None, Err(unreached)) => Some(unreached.words().to_owned()),
            (None, Ok(())) => {
                Some("The service stopped before it said how turning on Hyper-V ended.".to_owned())
            }
        };
        let _ = window.tell(Message::HyperVDone(ended));
    });
    if let Err(error) = spawned {
        let _ = failed.tell(Message::HyperVDone(Some(format!("The app couldn't ask the service: {error}"))));
    }
}

/// A machine's events, coming in on a thread of their own. Dropping it ends them.
pub struct Watch {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// What the thread reading a watch hands the thread that tells the window.
enum Heard {
    Event(Event),
    Ended(Ended),
}

/// Starts watching a machine. Its messages carry `id`, so the window can tell them from the
/// messages of a watch it has let go of that were already on their way.
///
/// One thread reads the events, as a read on the pipe waits, and another tells the window, gathering
/// the events that come close together into one message, so it can send what's waiting when its
/// time comes even while the read waits for more.
pub fn watch(id: u64, name: String, window: Window) -> Watch {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let failed = window.clone();
    let (heard, inbox) = mpsc::channel::<Heard>();
    if let Err(error) =
        thread::Builder::new().name("watch tell".into()).spawn(move || tell_events(id, inbox, window))
    {
        let why = Ended::Unreached(format!("The app couldn't start watching: {error}"));
        let _ = failed.tell(Message::WatchEnded(id, why));
        return Watch { stop, thread: None };
    }
    let spawned = thread::Builder::new().name("watch".into()).spawn(move || {
        let mut refused = None;
        let streamed = client::stream(&Request::Watch { name }, |answer| {
            if stopped.load(Ordering::Relaxed) {
                return false;
            }
            match answer {
                // Fails once the other thread has ended, as when the window has gone.
                Answer::Event { event } => heard.send(Heard::Event(event)).is_ok(),
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
        let _ = heard.send(Heard::Ended(why));
    });
    match spawned {
        Ok(thread) => Watch { stop, thread: Some(thread) },
        Err(error) => {
            // The other thread ends as nothing can reach it now, and tells the window nothing.
            let why = Ended::Unreached(format!("The app couldn't start watching: {error}"));
            let _ = failed.tell(Message::WatchEnded(id, why));
            Watch { stop, thread: None }
        }
    }
}

/// Tells the window a watch's events, a batch at most every [`EVENTS_APART`], and then how it
/// ended, after the last of them. Ends when the reading thread has, or the window has gone.
fn tell_events(id: u64, inbox: Receiver<Heard>, window: Window) {
    let mut batch = Batch::new(EVENTS_APART);
    loop {
        let heard = match batch.due() {
            Some(due) => inbox.recv_timeout(due.saturating_duration_since(Instant::now())),
            None => inbox.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let events = match heard {
            Ok(Heard::Event(event)) => batch.add(event, Instant::now()),
            Ok(Heard::Ended(why)) => {
                let events = batch.take_all();
                if !events.is_empty() && !window.tell(Message::Watched(id, events)) {
                    return;
                }
                let _ = window.tell(Message::WatchEnded(id, why));
                return;
            }
            Err(RecvTimeoutError::Timeout) => batch.take_due(Instant::now()),
            // Let go of: what's waiting is for a watch the window no longer shows.
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if let Some(events) = events
            && !window.tell(Message::Watched(id, events))
        {
            return;
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

/// What a remote view's thread is given to do.
enum Look {
    /// Ask this of the service.
    Ask(Ask),
    /// Look how it stands now, as when its switch went on.
    Now,
}

/// A shown Windows machine's remote view, asked about on a thread of its own: what the person asks
/// of it, and how it stands every [`EVERY`] while its switch is on. A reading of remoteman in the
/// guest can take seconds, which mustn't hold up the service's other answers. Dropping it ends the
/// thread once its call in hand is answered.
pub struct Looking {
    work: Sender<Look>,
    on: Arc<AtomicBool>,
}

impl Looking {
    pub fn ask(&self, ask: Ask) {
        let _ = self.work.send(Look::Ask(ask));
    }

    /// Whether to keep looking, as the switch is. Turned on, it looks at once.
    pub fn set_on(&self, on: bool) {
        if !self.on.swap(on, Ordering::Relaxed) && on {
            let _ = self.work.send(Look::Now);
        }
    }
}

/// The answer to a remote view's request, or the words for why there's none.
fn share_answer(answer: Result<Answer, Unreached>) -> Result<ShareInfo, String> {
    match answer {
        Ok(Answer::Share { info }) => Ok(info),
        Ok(Answer::Refused { reason }) => Err(reason),
        Ok(_) => Err(Unreached::Garbled.words().to_owned()),
        Err(unreached) => Err(unreached.words().to_owned()),
    }
}

/// Starts asking about `name`'s remote view. Its messages carry `id`, so the window can tell them
/// from those of a machine it no longer shows. Each answer to what was asked goes to the window;
/// a look goes only when it differs from the last one told, with this PC's address, which a device
/// types in.
pub fn look(id: u64, name: String, on: bool, window: Window) -> Looking {
    let (work, inbox) = mpsc::channel::<Look>();
    let on = Arc::new(AtomicBool::new(on));
    let looking = Arc::clone(&on);
    let failed = window.clone();
    let spawned = thread::Builder::new().name("remote view".into()).spawn(move || {
        let mut told = Changes::new();
        // The first look is at once.
        let mut wait = Duration::ZERO;
        loop {
            match inbox.recv_timeout(wait) {
                Ok(Look::Ask(ask)) => {
                    let answer = share_answer(client::ask(&ask.request(&name)));
                    if !window.tell(Message::RemoteViewAnswered(id, ask, answer)) {
                        return;
                    }
                    // The answer is how it stands now, which the window has taken.
                    told.forget();
                    wait = EVERY;
                    continue;
                }
                Ok(Look::Now) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            wait = EVERY;
            if !looking.load(Ordering::Relaxed) {
                continue;
            }
            let answer = share_answer(client::ask(&Request::RemoteView { name: name.clone() }));
            let address = guesthouse_relay::lan_address();
            let seen = (answer.as_ref().map(remote_view::settled).map_err(Clone::clone), address);
            if told.news(seen).is_some() && !window.tell(Message::RemoteViewSeen(id, answer, address)) {
                return;
            }
        }
    });
    if let Err(error) = spawned {
        let words = format!("The app couldn't start asking about the remote view: {error}");
        let _ = failed.tell(Message::RemoteViewSeen(id, Err(words), None));
    }
    Looking { work, on }
}

/// The machine whose pairing the app last saw open, to close when the app goes, as nobody would be
/// there to answer a device.
static PAIRING_OPEN: Mutex<Option<String>> = Mutex::new(None);

/// Notes whether `name`'s pairing is open, as an answer said.
pub fn note_pairing(name: &str, open: bool) {
    let mut noted = PAIRING_OPEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if open {
        *noted = Some(name.to_owned());
    } else if noted.as_deref() == Some(name) {
        *noted = None;
    }
}

/// Closes the pairing the app left open, if it did, as it goes. Best effort: the service closes it
/// itself when its time is up.
pub fn close_pairing_left_open() {
    let noted = PAIRING_OPEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    if let Some(name) = noted {
        let _ = client::ask(&Request::PairClose { name });
    }
}
