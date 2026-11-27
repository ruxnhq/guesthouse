//! When the host card's memory clock is held up for a machine's screen: while a viewer streams it,
//! and for a short while after.
//!
//! An NVIDIA card clocks itself up for steady work, but not for the short bursts of a desktop that
//! changes now and then, so a guest's encoder takes three or four times as long for each new picture
//! while the card idles. Holding the memory clock up fixes most of it, at the cost of the card's idle
//! power: about 17 W to 100 W on an RTX 3080 Ti. So it's held only while somebody is watching.
//!
//! Two parts, both without a clock or a system of their own, so every host can use them:
//!
//! - [`Viewers`] says whether a viewer streams, from what a host knows of the devices sending to the
//!   guest: how long since each was last heard, and whether the guest still answers it. remoteman's
//!   transport sends a keep-alive every second and gives a connection up after ten seconds of
//!   silence, so a device heard within that and answered within that is a viewer. Both ends do it:
//!   remoteman's host listens with the same QUIC settings as its viewer, a keep-alive every second,
//!   which QUIC sends as a ping whenever nothing else went out, and it acknowledges the viewer's. So
//!   the guest answers a viewer every second while its desktop is still, and a still desktop keeps
//!   its viewer. A host that knows
//!   it some other way, as from remoteman's own `status.client`, skips this part.
//! - [`StreamClocks`] turns that into holding and letting go, with the setting and the end of the
//!   machine.

use crate::pairing::Millis;
use std::collections::HashMap;

/// What `guesthouse-vm set` calls the setting.
pub const SETTING: &str = "stream-clocks";
/// remoteman gives a connection up after this much silence, so a device quieter than this is gone.
pub const SILENT_MS: Millis = 10_000;
/// How long the clock stays held after the last viewer was seen, so a viewer that comes back at
/// once, or a stream that pauses, doesn't wait for the card again.
pub const LET_GO_MS: Millis = 30_000;

/// A device sending to the guest, as the host's relay sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Its address and port, which tells it apart from the others.
    pub address: String,
    /// How long since it last sent something.
    pub idle_ms: Millis,
    /// How many packets the guest has sent it, in all.
    pub down_packets: u64,
}

/// Whether a viewer streams, from the devices a host's relay sees.
#[derive(Debug, Default)]
pub struct Viewers {
    /// Each device's packets from the guest as last seen, and when that last grew.
    answered: HashMap<String, (u64, Option<Millis>)>,
}

impl Viewers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any device is a viewer now: heard within [`SILENT_MS`], and answered by the guest
    /// within it too. A stranger whose packets the guest ignores never counts.
    pub fn streaming(&mut self, now: Millis, devices: &[Device]) -> bool {
        let mut answered = HashMap::new();
        let mut streaming = false;
        for device in devices {
            let grew = match self.answered.get(&device.address) {
                Some(&(down, at)) if device.down_packets == down => at,
                // Fewer than before is a new device on an old address, which counts from zero.
                Some(&(down, _)) if device.down_packets < down => (device.down_packets > 0).then_some(now),
                Some(_) => Some(now),
                None => (device.down_packets > 0).then_some(now),
            };
            let recent = |at: Millis| now.saturating_sub(at) < SILENT_MS;
            streaming |= device.idle_ms < SILENT_MS && grew.is_some_and(recent);
            answered.insert(device.address.clone(), (device.down_packets, grew));
        }
        self.answered = answered;
        streaming
    }
}

/// What the holder should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Hold,
    LetGo,
}

/// Holding the clock for one machine's screen.
#[derive(Debug)]
pub struct StreamClocks {
    on: bool,
    held: bool,
    last_streaming: Option<Millis>,
}

impl StreamClocks {
    /// `on` is the machine's setting.
    pub fn new(on: bool) -> Self {
        Self { on, held: false, last_streaming: None }
    }

    pub fn held(&self) -> bool {
        self.held
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// One look: whether a viewer streams now. Says what to change, if anything.
    pub fn step(&mut self, now: Millis, streaming: bool) -> Option<Change> {
        if streaming {
            self.last_streaming = Some(now);
        }
        let wanted = self.on && self.last_streaming.is_some_and(|at| now.saturating_sub(at) < LET_GO_MS);
        self.change_to(wanted)
    }

    /// The setting changed. Turned off, a held clock goes at once.
    pub fn set_on(&mut self, on: bool) -> Option<Change> {
        self.on = on;
        if on { None } else { self.change_to(false) }
    }

    /// The holder couldn't hold, and won't try again: nothing is held.
    pub fn give_up(&mut self) {
        self.on = false;
        self.held = false;
    }

    /// The machine or its screen ended: whatever is held goes.
    pub fn end(&mut self) -> Option<Change> {
        self.last_streaming = None;
        self.change_to(false)
    }

    fn change_to(&mut self, wanted: bool) -> Option<Change> {
        if wanted == self.held {
            return None;
        }
        self.held = wanted;
        Some(if wanted { Change::Hold } else { Change::LetGo })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(address: &str, idle_ms: Millis, down_packets: u64) -> Device {
        Device { address: address.into(), idle_ms, down_packets }
    }

    #[test]
    fn a_device_the_guest_answers_is_a_viewer() {
        let mut viewers = Viewers::new();
        assert!(!viewers.streaming(0, &[]));
        assert!(viewers.streaming(0, &[device("a:1", 0, 4)]));
        // Keep-alives both ways, a second apart.
        assert!(viewers.streaming(1_000, &[device("a:1", 200, 6)]));
        assert!(viewers.streaming(9_000, &[device("a:1", 8_000, 6)]), "answered within ten seconds");
        assert!(!viewers.streaming(11_000, &[device("a:1", 10_000, 6)]), "gone quiet");
    }

    #[test]
    fn a_viewer_of_a_still_desktop_stays_a_viewer() {
        let mut viewers = Viewers::new();
        let mut clocks = StreamClocks::new(true);
        // Five minutes of nothing on the screen: only the keep-alives, a ping each way every second.
        for second in 0..300 {
            let now = second * 1_000;
            let streaming = viewers.streaming(now, &[device("a:1", 300, 2 * second + 1)]);
            assert!(streaming, "at {second} s");
            clocks.step(now, streaming);
            assert!(clocks.held(), "at {second} s");
        }
    }

    #[test]
    fn a_stranger_the_guest_ignores_is_not_a_viewer() {
        let mut viewers = Viewers::new();
        assert!(!viewers.streaming(0, &[device("s:1", 0, 0)]));
        assert!(!viewers.streaming(5_000, &[device("s:1", 0, 0)]));
        // The guest answered its first try and then nothing more: it isn't paired.
        assert!(viewers.streaming(6_000, &[device("s:1", 0, 1)]));
        assert!(!viewers.streaming(17_000, &[device("s:1", 0, 1)]), "still sending, never answered again");
    }

    #[test]
    fn one_viewer_among_others_is_enough() {
        let mut viewers = Viewers::new();
        let devices = [device("s:1", 0, 0), device("a:1", 100, 9)];
        assert!(viewers.streaming(0, &devices));
    }

    #[test]
    fn the_clock_is_held_while_viewed_and_let_go_a_while_after() {
        let mut clocks = StreamClocks::new(true);
        assert_eq!(clocks.step(0, false), None);
        assert_eq!(clocks.step(1_000, true), Some(Change::Hold));
        assert_eq!(clocks.step(2_000, true), None);
        assert_eq!(clocks.step(2_000 + LET_GO_MS - 1, false), None, "a pause keeps it");
        assert_eq!(clocks.step(2_000 + LET_GO_MS, false), Some(Change::LetGo));
        assert!(!clocks.held());
        assert_eq!(clocks.step(2_000 + LET_GO_MS + 1, false), None);
    }

    #[test]
    fn turned_off_nothing_is_held_and_a_held_clock_goes_at_once() {
        let mut clocks = StreamClocks::new(false);
        assert_eq!(clocks.step(0, true), None);
        assert_eq!(clocks.set_on(true), None);
        assert_eq!(clocks.step(100, true), Some(Change::Hold));
        assert_eq!(clocks.set_on(false), Some(Change::LetGo));
        assert_eq!(clocks.step(200, true), None);
    }

    #[test]
    fn the_end_lets_go_of_a_held_clock() {
        let mut clocks = StreamClocks::new(true);
        assert_eq!(clocks.end(), None, "nothing held");
        clocks.step(0, true);
        assert_eq!(clocks.end(), Some(Change::LetGo));
        assert_eq!(clocks.step(1, false), None, "the last viewer is forgotten");
    }

    #[test]
    fn giving_up_stops_trying() {
        let mut clocks = StreamClocks::new(true);
        assert_eq!(clocks.step(0, true), Some(Change::Hold));
        clocks.give_up();
        assert!(!clocks.held());
        assert_eq!(clocks.step(1, true), None);
        assert_eq!(clocks.end(), None);
    }
}
