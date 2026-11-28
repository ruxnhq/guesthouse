//! The remote view's hold on the card's memory clock: `guesthouse-clock-helper hold CARD` while a
//! viewer streams, and its input closed when the viewer has gone, which has the helper put the clock
//! back. The program holds it, not the app, as the program lives as long as the machine does, with or
//! without the app's window.
//!
//! Nothing here can leave a card locked. The helper puts the clock back when its input closes, which
//! the kernel does when this program ends, however it ends. A helper killed itself leaves a mark, and
//! `guesthouse-clock-helper reset CARD` puts that back: this program runs it as it starts and as it
//! ends, and the app when the machine stops.
//!
//! Where the helper isn't installed (a build of the repo, run without install.sh) or the card can't,
//! one line says so in the log, and nothing else happens.

use super::RelayClient;
use guesthouse_remote_view_core::stream_clocks::{Change, Device, StreamClocks, Viewers};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Where install.sh puts the helper, setuid root.
pub const OWN_CLOCK_HELPER: &str = "/usr/local/libexec/guesthouse/guesthouse-clock-helper";
/// Where Guesthouse's NixOS module puts it, as a setuid wrapper.
pub const NIXOS_CLOCK_HELPER: &str = "/run/wrappers/bin/guesthouse-clock-helper";
const CLOCK_HELPERS: [&str; 2] = [OWN_CLOCK_HELPER, NIXOS_CLOCK_HELPER];
const SAID: &str = "remote view: stream clocks:";
/// How long the helper has to put the clock back. It takes a moment: its library is loaded already.
const PUT_BACK_SECONDS: u64 = 10;
const POLL: Duration = Duration::from_millis(50);

/// The helper, if this PC has one.
pub fn clock_helper() -> Option<PathBuf> {
    CLOCK_HELPERS.iter().map(PathBuf::from).find(|place| place.exists())
}

/// Why the helper ended without holding, from what it exited with, in a line for the log.
fn why(code: Option<i32>) -> String {
    match code {
        Some(3) => "the helper isn't setuid root, so the card's clocks are left alone. Run Guesthouse's \
                    installer again to put it in."
            .into(),
        Some(4) => "the card isn't an NVIDIA display card, so its clocks are left alone.".into(),
        Some(5) => "NVIDIA's management library isn't there or doesn't know the card, so its clocks are \
                    left alone."
            .into(),
        Some(6) => "this card or its driver can't lock the memory clock, so it's left alone.".into(),
        Some(7) => "this user can't open the card's device file, so its clocks are left alone.".into(),
        Some(code) => format!("the helper ended with {code}, so the card's clocks are left alone."),
        None => "the helper was stopped by a signal, so the card's clocks are left alone.".into(),
    }
}

/// Waits for a helper to end, for so long. Gives whether it did.
fn wait_for(child: &mut Child, seconds: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return true,
            Ok(None) if Instant::now() >= end => return false,
            Ok(None) => thread::sleep(POLL),
        }
    }
}

/// Where the helper's lines go.
pub enum Said {
    /// Where this program's go: the remote view program's are the app's log.
    Here,
    /// Into this file, as the app's log, so nothing reaches a terminal.
    Into(File),
    Nowhere,
}

impl Said {
    fn outputs(self) -> (Stdio, Stdio) {
        match self {
            Said::Here => (Stdio::inherit(), Stdio::inherit()),
            Said::Into(file) => match file.try_clone() {
                Ok(copy) => (Stdio::from(copy), Stdio::from(file)),
                Err(_) => (Stdio::null(), Stdio::null()),
            },
            Said::Nowhere => (Stdio::null(), Stdio::null()),
        }
    }
}

/// Puts back a lock a killed helper left on `card`, if nobody holds the card now. Quiet where there's
/// no helper: whoever holds says that. With nothing to put back the helper doesn't load NVIDIA's
/// library, so this costs a moment.
pub fn sweep(helper: Option<&Path>, card: &str, said: Said) {
    let Some(helper) = helper else { return };
    let (out, errors) = said.outputs();
    let started = Command::new(helper)
        .args(["reset", card])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(errors)
        .spawn();
    if let Ok(mut child) = started
        && !wait_for(&mut child, PUT_BACK_SECONDS)
    {
        // It ends by itself, and init takes it then.
        eprintln!("{SAID} the helper's reset of {card} is taking long; it is left to finish.");
    }
}

/// One machine's hold, in its remote view program.
pub struct ClockHolder {
    helper: Option<PathBuf>,
    card: Option<String>,
    clocks: StreamClocks,
    viewers: Viewers,
    holding: Option<Child>,
    /// Helpers told to let go that haven't ended yet.
    leaving: Vec<Child>,
    /// The end has been seen to, so a second one, as when the holder is dropped, does nothing.
    ended: bool,
}

impl ClockHolder {
    /// For `card`, the NVIDIA card's PCI address, with the helper at `helper`. With no card nothing is
    /// held until [`ClockHolder::set_card`] gives one.
    pub fn new(helper: Option<PathBuf>, card: Option<String>) -> Self {
        let mut holder = Self {
            helper,
            card: None,
            clocks: StreamClocks::new(false),
            viewers: Viewers::new(),
            holding: None,
            leaving: Vec::new(),
            ended: false,
        };
        holder.set_card(card);
        holder
    }

    pub fn held(&self) -> bool {
        self.clocks.held()
    }

    /// The machine's setting: hold this card, or with none, nothing.
    pub fn set_card(&mut self, card: Option<String>) {
        if card.is_some() && card != self.card {
            self.end();
        }
        let Some(card) = card else {
            self.card = None;
            let change = self.clocks.set_on(false);
            self.apply(change);
            return;
        };
        if self.card.as_deref() == Some(card.as_str()) && self.clocks.is_on() {
            return;
        }
        match &self.helper {
            None => {
                eprintln!(
                    "{SAID} there's no clock helper at {OWN_CLOCK_HELPER} (a build of the repo has none), \
                     so the card's clocks are left alone."
                );
                self.clocks.give_up();
            }
            Some(helper) => {
                sweep(Some(helper), &card, Said::Here);
                self.clocks.set_on(true);
            }
        }
        self.card = Some(card);
        self.ended = false;
    }

    /// One look at the devices the relay has, `now` in milliseconds of the program's own clock.
    pub fn tick(&mut self, now: u64, clients: &[RelayClient]) {
        self.leaving.retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_)) | Err(_)));
        if let Some(child) = &mut self.holding
            && let Ok(Some(status)) = child.try_wait()
        {
            // It ended while it was meant to hold: it couldn't, and won't be able to this run.
            self.holding = None;
            eprintln!("{SAID} {}", why(status.code()));
            self.clocks.give_up();
        }
        let devices: Vec<Device> = clients
            .iter()
            .map(|client| Device {
                address: client.address.clone(),
                idle_ms: client.idle_ms,
                down_packets: client.down_packets,
            })
            .collect();
        let streaming = self.viewers.streaming(now, &devices);
        let change = self.clocks.step(now, streaming);
        self.apply(change);
    }

    fn apply(&mut self, change: Option<Change>) {
        match change {
            Some(Change::Hold) => self.hold(),
            Some(Change::LetGo) => {
                if let Some(mut child) = self.holding.take() {
                    // Its input closing is what has it put the clock back.
                    drop(child.stdin.take());
                    self.leaving.push(child);
                }
            }
            None => {}
        }
    }

    fn hold(&mut self) {
        let (Some(helper), Some(card)) = (&self.helper, &self.card) else {
            self.clocks.give_up();
            return;
        };
        // Its lines go where this program's do, the app's log.
        let started = Command::new(helper).args(["hold", card]).env_clear().stdin(Stdio::piped()).spawn();
        match started {
            Ok(child) => self.holding = Some(child),
            Err(error) => {
                eprintln!(
                    "{SAID} the clock helper didn't start ({error}), so the card's clocks are left alone."
                );
                self.clocks.give_up();
            }
        }
    }

    /// The machine or its screen ended: the clock goes back, and this waits until it has.
    pub fn end(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        let change = self.clocks.end();
        self.apply(change);
        for mut child in std::mem::take(&mut self.leaving) {
            if !wait_for(&mut child, PUT_BACK_SECONDS) {
                eprintln!("{SAID} the helper is taking long to put the clock back; it is left to finish.");
            }
        }
        if let Some(card) = self.card.clone() {
            sweep(self.helper.as_deref(), &card, Said::Here);
        }
    }
}

impl Drop for ClockHolder {
    fn drop(&mut self) {
        self.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::fs;

    const CARD: &str = "0000:01:00.0";

    /// A helper that writes down what it was asked, holds until its input ends, and says when.
    fn stand_in(scratch: &Scratch, exit: u8) -> (PathBuf, PathBuf) {
        let said = scratch.path("said");
        let helper = scratch.program(
            "guesthouse-clock-helper",
            &format!(
                "#!/bin/sh\necho \"$@\" >> {said}\n[ {exit} = 0 ] || exit {exit}\n\
                 if [ \"$1\" = hold ]; then cat >/dev/null; echo put back >> {said}; fi\n",
                said = said.display()
            ),
        );
        (helper, said)
    }

    fn viewer(idle_ms: u64, down_packets: u64) -> RelayClient {
        RelayClient { address: "192.168.1.9:5000".into(), idle_ms, up_packets: 1, down_packets }
    }

    fn said(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    fn eventually(mut what: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(5);
        while !what() && Instant::now() < end {
            thread::sleep(POLL);
        }
        what()
    }

    #[test]
    fn a_viewer_has_the_clock_held_and_the_end_puts_it_back() {
        let scratch = Scratch::new();
        let (helper, log) = stand_in(&scratch, 0);
        let mut holder = ClockHolder::new(Some(helper), Some(CARD.into()));
        assert_eq!(said(&log), format!("reset {CARD}\n"), "what a killed helper left is put back first");
        holder.tick(0, &[]);
        assert!(!holder.held());
        holder.tick(100, &[viewer(0, 5)]);
        assert!(holder.held());
        assert!(eventually(|| said(&log).contains(&format!("hold {CARD}"))));
        holder.end();
        assert!(!holder.held());
        assert!(said(&log).ends_with(&format!("put back\nreset {CARD}\n")), "{}", said(&log));
    }

    #[test]
    fn the_clock_goes_back_a_while_after_the_viewer_has_gone() {
        let scratch = Scratch::new();
        let (helper, log) = stand_in(&scratch, 0);
        let mut holder = ClockHolder::new(Some(helper), Some(CARD.into()));
        holder.tick(0, &[viewer(0, 5)]);
        holder.tick(20_000, &[viewer(20_000, 5)]);
        assert!(holder.held(), "still within the while after");
        holder.tick(40_000, &[]);
        assert!(!holder.held());
        assert!(eventually(|| said(&log).contains("put back")));
    }

    #[test]
    fn dropping_the_holder_puts_the_clock_back() {
        let scratch = Scratch::new();
        let (helper, log) = stand_in(&scratch, 0);
        let mut holder = ClockHolder::new(Some(helper), Some(CARD.into()));
        holder.tick(0, &[viewer(0, 5)]);
        drop(holder);
        assert!(said(&log).contains("put back"));
    }

    #[test]
    fn turning_the_setting_off_puts_the_clock_back_at_once() {
        let scratch = Scratch::new();
        let (helper, log) = stand_in(&scratch, 0);
        let mut holder = ClockHolder::new(Some(helper), None);
        holder.tick(0, &[viewer(0, 5)]);
        assert!(!holder.held(), "no card, nothing held");
        holder.set_card(Some(CARD.into()));
        holder.tick(100, &[viewer(0, 6)]);
        assert!(holder.held());
        holder.set_card(None);
        assert!(!holder.held());
        assert!(eventually(|| said(&log).contains("put back")));
        holder.tick(200, &[viewer(0, 7)]);
        assert!(!holder.held());
    }

    #[test]
    fn without_a_helper_nothing_is_tried() {
        let mut holder = ClockHolder::new(None, Some(CARD.into()));
        holder.tick(0, &[viewer(0, 5)]);
        assert!(!holder.held());
        holder.end();
    }

    #[test]
    fn a_helper_that_cannot_hold_is_not_started_again() {
        let scratch = Scratch::new();
        let (helper, log) = stand_in(&scratch, 6);
        let mut holder = ClockHolder::new(Some(helper), Some(CARD.into()));
        holder.tick(0, &[viewer(0, 5)]);
        let mut now = 0;
        assert!(eventually(|| {
            now += 1;
            holder.tick(now, &[viewer(0, 5 + now)]);
            !holder.held()
        }));
        holder.tick(200, &[viewer(0, 7)]);
        assert_eq!(said(&log).matches("hold").count(), 1);
        assert!(why(Some(3)).contains("setuid"));
    }
}
