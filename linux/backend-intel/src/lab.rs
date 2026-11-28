//! The lab's card, in a build with the `lab-fake-card` feature only: a recorded card's guest runs
//! the work path against a stand-in for xe ([`crate::xe::fake::FakeXe::lab`]) whose buffers are
//! host memory and whose batches end at once, unrun. Maps, CPU locks, guest pages, exports and
//! frame copies all meet real bytes; nothing is drawn or computed. What it shows is the work
//! path's plumbing with a real guest, never what a card does.

use crate::IntelBackend;
use crate::stage::{Fallback, Stage};
use crate::watch::PREFIX;
use crate::xe::fake::FakeXe;
use std::ffi::OsStr;

/// `1` sends a recorded card's guest to the lab's card.
pub const FAKE_CARD_VARIABLE: &str = "GUESTHOUSE_INTEL_LAB_FAKE_CARD";

/// Whether the lab's card was asked for. Only a recorded card gets here.
pub fn fake_card_asked(switch: Option<&OsStr>) -> bool {
    switch.is_some_and(|value| value == "1")
}

impl IntelBackend {
    /// Starts work on the lab's card if it was asked for, at the stage asked. False leaves the
    /// recorded card to listen as it does in every other build.
    pub(crate) fn start_on_lab_card(
        &mut self,
        asked: (Stage, Option<Fallback>),
        switch: Option<&OsStr>,
    ) -> bool {
        if !fake_card_asked(switch) {
            return false;
        }
        let card = FakeXe::lab();
        let given = card.clone();
        log::warn!(
            "{PREFIX} lab card: {FAKE_CARD_VARIABLE}=1 on a recorded card. The guest's work goes to a \
             stand-in for xe with host memory, and every batch ends at once, unrun. Lab plumbing only"
        );
        self.start_work_on_card(asked, move || Ok(Box::new(given) as _), None);
        card.stop_running();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_one_asks_for_the_lab_card() {
        assert!(fake_card_asked(Some(OsStr::new("1"))));
        for other in ["0", "yes", "", "true"] {
            assert!(!fake_card_asked(Some(OsStr::new(other))), "{other}");
        }
        assert!(!fake_card_asked(None));
    }

    #[test]
    fn the_lab_card_runs_everything_and_leaves_a_recorded_card_listening_when_not_asked() {
        let mut backend = IntelBackend::from_card(crate::card::tests::lunar_lake()).unwrap();
        assert!(!backend.start_on_lab_card(Stage::asked(None), None));
        assert!(backend.work.is_none());
        assert!(backend.start_on_lab_card(Stage::asked(None), Some(OsStr::new("1"))));
        assert_eq!(backend.stage(), Stage::Everything, "the self-test passed on the lab card");
    }
}
