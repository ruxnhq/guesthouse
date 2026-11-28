//! Starting the helper in the guest through the agent, and the link to a machine's helper made for it.
//! Nothing here runs for a machine that has app windows off: the link is refused first, so no agent
//! request is made and no helper starts.

use super::client::{Client, Update};
use super::forward::{forward_word, host_port, running_machine_has_forward};
use super::link::{HelperStarter, LinkConfig, Status};
use super::switch::{is_on, off_text};
use crate::agent::{helper_start_with, read_token, stop_helper};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::guest_end::GuestEnd;
use crate::machine::{Kind, Machine};
use guesthouse_daemon::slot::Slot;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The helper's file, start script and task share this name, as the other helpers' do. The task is
/// found by the file's name, and the helper before it is ended by it.
pub const HELPER: &str = "app-windows";
/// The helper's C# source, which goes along with the script for the guest to compile. The start script
/// finds it as `$source`, in Base64, as it finds the script as `$helper`.
pub const SOURCE_FILE: &str = "app-windows.cs";
const SOURCE_VARIABLE: &str = "source";
const SCRIPT_FILE: &str = "app-windows.ps1";
const START_SCRIPT_FILE: &str = "app-windows-start.ps1";
/// The helper compiles its calls to Windows before it listens, and the first start also opens the
/// port in the firewall.
const START_SECONDS: u64 = 30;
const STARTED_WORD: &str = "started";
const NOBODY_WORD: &str = "nobody is logged in";
/// How often a command that waits looks at the link.
/// What the link says of a line it could not use, which a log shows and a command quotes.
const LEFT_OUT: &str = "was left out";
const LOOK: Duration = Duration::from_millis(100);

/// Starts the machine's helper through its agent.
pub struct AgentStarter {
    context: Arc<Context>,
    machine: Machine,
    slot: Option<Slot>,
    started: AtomicBool,
    /// Whether the last start failed for a reason that time can mend: a guest that is still starting
    /// has no session yet and an agent that is not up.
    may_pass: AtomicBool,
    last: Mutex<Option<std::result::Result<(), String>>>,
}

impl AgentStarter {
    fn new(context: Arc<Context>, machine: Machine, slot: Option<Slot>) -> Self {
        Self {
            context,
            machine,
            slot,
            started: AtomicBool::new(false),
            may_pass: AtomicBool::new(false),
            last: Mutex::new(None),
        }
    }

    /// Whether this starter started a helper that was not running.
    pub fn did_start(&self) -> bool {
        self.started.load(Ordering::Relaxed)
    }

    /// Why the last start failed, if it did.
    pub fn may_pass(&self) -> bool {
        self.may_pass.load(Ordering::Relaxed)
    }

    pub fn failure(&self) -> Option<String> {
        self.last.lock().ok()?.clone()?.err()
    }

    fn try_start(&self) -> std::result::Result<(), String> {
        let (context, machine) = (&*self.context, &self.machine);
        let name = &machine.name;
        if !is_on(context, machine) {
            return Err(off_text(machine));
        }
        // A machine started before app windows was on has no way in for the helper, and starting the
        // helper would only leave it listening where nothing reaches.
        if running_machine_has_forward(context, machine, self.slot) == Some(false) {
            return Err(format!(
                "{name} was started without the way in that app windows needs ({}). Restart it.",
                forward_word(host_port(context, self.slot))
            ));
        }
        let files = context.paths.guest_files();
        if !files.join(SCRIPT_FILE).is_file() || !files.join(START_SCRIPT_FILE).is_file() {
            return Err(
                "This program folder has no app windows helper for the guest. Update Guesthouse.".into()
            );
        }
        let script = helper_start_with(context, HELPER, &[(SOURCE_VARIABLE, SOURCE_FILE)])
            .map_err(|failure| failure.0)?;
        let said = context.ask_guest(machine, &script, START_SECONDS, self.slot);
        if said.contains(STARTED_WORD) {
            return Ok(());
        }
        self.may_pass.store(said.contains(NOBODY_WORD) || said.trim().is_empty(), Ordering::Relaxed);
        Err(if said.contains(NOBODY_WORD) {
            format!("Nobody is logged in to {name}, so there is no session for the app windows helper.")
        } else if said.trim().is_empty() {
            format!("{name}'s agent did not answer, so the app windows helper could not be started.")
        } else {
            format!("The app windows helper did not start in {name}.")
        })
    }
}

impl HelperStarter for AgentStarter {
    fn start(&self) -> std::result::Result<(), String> {
        let result = self.try_start();
        if result.is_ok() {
            self.started.store(true, Ordering::Relaxed);
        }
        if let Ok(mut last) = self.last.lock() {
            *last = Some(result.clone());
        }
        result
    }
}

/// A machine's client, with what it takes to say how it got on.
pub struct MachineLink {
    pub client: Client,
    starter: Arc<AgentStarter>,
    context: Arc<Context>,
    machine: Machine,
    slot: Option<Slot>,
}

/// Starts the link to a machine's helper. Refused for a machine that is not Windows, that has app
/// windows off, or that has no token, with the reason in words. It does not wait: the link comes up on
/// its thread, and starts the helper itself if nothing answers.
pub fn connect(context: &Arc<Context>, machine: &Machine, slot: Option<Slot>) -> Result<MachineLink> {
    connect_as(context, machine, slot, false)
}

/// The same as a side connection, which a command uses beside a running viewer: the helper keeps the
/// viewer's connection and the viewer is not disturbed.
pub fn connect_side(context: &Arc<Context>, machine: &Machine, slot: Option<Slot>) -> Result<MachineLink> {
    connect_as(context, machine, slot, true)
}

fn connect_as(
    context: &Arc<Context>,
    machine: &Machine,
    slot: Option<Slot>,
    side: bool,
) -> Result<MachineLink> {
    let name = &machine.name;
    if machine.kind != Kind::Windows {
        return Err(Failure(format!(
            "{name} is not a Windows machine, and only Windows has a helper for this."
        )));
    }
    if !is_on(context, machine) {
        return Err(Failure(off_text(machine)));
    }
    let token = read_token(context, machine)
        .ok_or_else(|| Failure(format!("{name} has no token, so there is no agent to start the helper.")))?;
    let starter = Arc::new(AgentStarter::new(context.clone(), machine.clone(), slot));
    let config = LinkConfig {
        end: GuestEnd::new(context, machine, slot, host_port(context, slot)),
        token,
        side,
        starter: Some(starter.clone()),
        timing: context.app_windows.timing,
    };
    Ok(MachineLink {
        client: Client::start(config),
        starter,
        context: context.clone(),
        machine: machine.clone(),
        slot,
    })
}

impl MachineLink {
    /// Waits for the helper's hello and its first layout, which includes starting it if it was not
    /// running. Gives up at once when the start itself said it cannot.
    pub fn wait_ready(&mut self) -> Result<()> {
        let timing = self.context.app_windows.timing;
        let until = Instant::now() + self.context.app_windows.patience;
        let name = self.machine.name.clone();
        // A helper that says hello and then sends a layout this build cannot read is the likeliest
        // mismatch with the guest, and waiting out the whole time for it would say nothing of why.
        let mut left_out: Option<(Instant, String)> = None;
        loop {
            for update in self.client.poll() {
                if let Update::Note(text) = update
                    && text.contains(LEFT_OUT)
                {
                    left_out.get_or_insert((Instant::now(), text));
                }
            }
            if self.client.model().seq().is_some() {
                return Ok(());
            }
            if let Some(why) = self.starter.failure() {
                return Err(Failure(why));
            }
            let up = *self.client.status() == Status::Up;
            if let Some((since, text)) = &left_out
                && up
                && since.elapsed() >= timing.hello
            {
                return Err(Failure(format!(
                    "The helper in {name} sent no layout this program can read. {text}"
                )));
            }
            if Instant::now() >= until {
                break;
            }
            self.client.wait_for(LOOK, |client| client.model().seq().is_some());
        }
        Err(Failure(match self.client.status() {
            Status::Down(why) => why.clone(),
            _ => format!("The app windows helper in {name} did not answer."),
        }))
    }

    /// Whether the helper's start failed in a way that a guest that is still coming up explains, so
    /// that asking again a little later can work: nobody is logged in yet, or the agent does not answer.
    pub fn start_may_pass(&self) -> bool {
        self.starter.may_pass()
    }

    /// Ends the helper if this link started it, so that no helper a look started is left running. What
    /// the helper did to the guest's windows while it ran is not undone by this.
    pub fn end_if_started(self) {
        let (context, machine, slot) = (self.context.clone(), self.machine.clone(), self.slot);
        let started = self.starter.did_start();
        drop(self);
        if started {
            stop_helper(&context, &machine, slot, HELPER);
        }
    }
}
