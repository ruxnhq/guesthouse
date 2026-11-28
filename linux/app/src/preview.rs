//! Made-up machines in place of the store's, for looking at the library in each of a machine's
//! states without a VM, a GPU or a store. Only a build with the `preview` feature has it, as on the
//! Windows host, and with the same variables: GUESTHOUSE_APP_PREVIEW names what to show,
//! GUESTHOUSE_APP_THEME may say `light` or `dark`, and GUESTHOUSE_APP_LOOK may name a look
//! (`automatic`, `tide`, `kde` or `system`):
//!
//! ```sh
//! cargo build -p guesthouse-app --features preview
//! GUESTHOUSE_APP_PREVIEW=home GUESTHOUSE_APP_LOOK=tide target/debug/guesthouse
//! ```
//!
//! `home` is the library with a machine in each state: running, needing the user, failed and off.
//! `settings` is the same with the settings of the machine that is off open. The machines are never
//! saved. What a machine keeps in its folder in the store, its disk (`disk.qcow2`), its token and
//! the picture of its screen, is read from there as for a real one, so a scratch store can hold
//! them to show.

use guesthouse_app_core::context::Context;
use guesthouse_app_core::machine::{Kind, Machine};
use guesthouse_app_core::settings::Fields;
use guesthouse_app_core::tasks::{State, Status};
use serde_json::Value;
use std::env;

const PREVIEW: &str = "GUESTHOUSE_APP_PREVIEW";
const THEME: &str = "GUESTHOUSE_APP_THEME";
const LOOK: &str = "GUESTHOUSE_APP_LOOK";
/// Where a made-up machine's disk would be, in its folder.
const DISK: &str = "disk.qcow2";
/// What the preview can show.
pub const HOME: &str = "home";
pub const SETTINGS: &str = "settings";

/// What the preview is asked to show, if it's on.
pub fn wanted() -> Option<String> {
    env::var(PREVIEW).ok().filter(|value| !value.is_empty())
}

/// The preferences, with the colours and look the preview is asked for in place of the saved ones.
pub fn with_asked(mut prefs: Fields) -> Fields {
    if let Ok(scheme) = env::var(THEME) {
        prefs.insert(crate::theme::SCHEME.to_owned(), Value::String(scheme));
    }
    if let Ok(look) = env::var(LOOK) {
        prefs.insert(crate::theme::LOOK_OF.to_owned(), Value::String(look));
    }
    prefs
}

fn machine(context: &Context, id: &str, name: &str, kind: Kind, cores: u32, ram_gib: u32) -> Machine {
    let mut machine = Machine::new(id, name, "", "");
    machine.disk = machine.folder(context).join(DISK).display().to_string();
    machine.kind = kind;
    machine.cores = cores;
    machine.ram_gib = ram_gib;
    machine
}

fn status(state: State) -> Status {
    Status { state, ..Status::default() }
}

/// The machines, each with the status it shows, in the library's order.
pub fn machines(context: &Context) -> Vec<(Machine, Status)> {
    let mut windows = machine(context, "preview-windows", "Windows 11", Kind::Windows, 8, 16);
    windows.screen = "2560x1440".into();
    windows.refresh_hz = 144;
    let running = Status { screen: "2560x1440 at 144 Hz".into(), ..status(State::Running) };
    let insider = machine(context, "preview-insider", "Windows Insider", Kind::Windows, 6, 12);
    let wants_you = Status {
        screen: "1920x1080 at 60 Hz".into(),
        display_problem: "Windows wants a restart to finish its updates.".into(),
        ..status(State::Running)
    };
    let mut fedora = machine(context, "preview-fedora", "Fedora 43", Kind::Plain, 4, 8);
    fedora.linux_gpu = true;
    let failed = Status {
        error: "It did not start: the NVIDIA driver changed since its last start.".into(),
        ..status(State::Off)
    };
    let ubuntu = machine(context, "preview-ubuntu", "Ubuntu 26.04", Kind::Linux, 4, 8);
    vec![(windows, running), (insider, wants_you), (fedora, failed), (ubuntu, status(State::Off))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::weight::Weight;

    #[test]
    fn the_preview_has_a_machine_of_each_weight() {
        use guesthouse_app_core::paths::{Paths, Variables};
        let paths = Paths::new(&Variables::of(&[("HOME", "/nowhere")]), None);
        let context = Context::new(paths, guesthouse_app_core::slots::first());
        let weights: Vec<Weight> = machines(&context)
            .iter()
            .map(|(machine, status)| Weight::of(status, machine.installing))
            .collect();
        assert_eq!(weights, [Weight::Stirs, Weight::Swells, Weight::Spills, Weight::Calm]);
    }
}
