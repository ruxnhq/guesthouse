//! Step one: what this PC has.

use super::done_page::done_page;
use super::driver_page::DriverPage;
use super::{Step, Wizard};
use crate::common::in_background;
use crate::rows::{row_button, state_row};
use adw::prelude::*;
use guesthouse_app_core::setup::checks::{AMD_NEEDS_NO_SETUP, Check, Part, check_pc, gpu_verdict};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Parts {
    wizard: Rc<Wizard>,
    step: Step,
    group: adw::PreferencesGroup,
    rows: RefCell<Vec<adw::ActionRow>>,
    /// Whether this PC's only cards are AMD's, which need no more setup, so Next ends it.
    amd_only: Cell<bool>,
}

#[derive(Clone)]
pub struct CheckPage(Rc<Parts>);

impl CheckPage {
    pub fn new(wizard: &Rc<Wizard>, driver: &DriverPage) -> Self {
        let step = Step::new("Check This PC", "check", 1);
        let group = adw::PreferencesGroup::new();
        step.content.add(&group);
        let again = row_button("Check Again");
        group.set_header_suffix(Some(&again));
        let this = Self(Rc::new(Parts {
            wizard: wizard.clone(),
            step,
            group,
            rows: RefCell::default(),
            amd_only: Cell::default(),
        }));
        let handle = this.clone();
        again.connect_clicked(move |_| handle.check());
        let (pages, next, choosing) = (wizard.pages.clone(), driver.page().clone(), this.clone());
        this.0.step.next.connect_clicked(move |_| {
            if choosing.0.amd_only.get() {
                pages.push(&done_page(&choosing.0.wizard));
            } else {
                pages.push(&next);
            }
        });
        this.check();
        this
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.0.step.page
    }

    fn check(&self) {
        self.0.step.next.set_sensitive(false);
        self.0.group.set_description(Some("Looking at this PC"));
        let context = self.0.wizard.window.context();
        let looking = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || Ok(check_pc(&looking)),
            move |result| match result {
                Ok(checks) => this.show(&checks),
                Err(text) => this.0.wizard.window.problem(&text),
            },
        );
    }

    fn show(&self, checks: &[Check]) {
        let parts = &self.0;
        for row in parts.rows.borrow_mut().drain(..) {
            parts.group.remove(&row);
        }
        for check in checks {
            let row = state_row(&check.title, check.state, &check.text, &check.fix);
            parts.group.add(&row);
            parts.rows.borrow_mut().push(row);
        }
        let verdict = gpu_verdict(checks);
        let has_nvidia = checks.iter().any(|check| check.part == Part::Nvidia);
        parts.amd_only.set(verdict.amd && !has_nvidia);
        let text = if verdict.nvidia {
            "This PC can run Windows with GPU-PV. Nothing has been changed."
        } else if parts.amd_only.get() {
            AMD_NEEDS_NO_SETUP
        } else if verdict.amd {
            "Windows can run on the AMD card, which needs no setup. The NVIDIA card can't yet: fix what has a \
             red mark, then check again."
        } else {
            "Windows with GPU-PV cannot run yet. Fix what has a red mark, then check again. Plain VMs and Ubuntu \
             do not need any of this."
        };
        parts.group.set_description(Some(text));
        parts.step.next.set_sensitive(verdict.nvidia || parts.amd_only.get());
    }
}
