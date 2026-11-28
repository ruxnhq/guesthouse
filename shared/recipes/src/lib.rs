//! Ready-made machines and lean Windows, as the Linux host has them. A recipe is a small text file
//! that says what a machine is for. It can set a few of the machine's settings when it's made, and
//! give the guest software once it has installed. Lean Windows is a Windows guest without the apps
//! Windows adds for every new account, which a recipe can turn on.
//!
//! `format` reads and checks a recipe, `shelf` finds the ones in a folder of the app's and one of the
//! user's, `preview` says in words what one will do, `record` keeps what a machine was made with and
//! how its steps went, and `settings` fits a recipe's settings to a PC's limits. [`lean`] has what lean
//! Windows takes out and switches off, and the files a machine keeps of it.
//!
//! What stays with a host: where its folders are, which recipes a kind of machine is offered, finding a
//! PC's limits, and running a recipe's steps and lean Windows in a guest. The line of a Windows answer
//! file that runs lean Windows from the disc is in `guesthouse-answers-core`. Only the Linux host has
//! recipes and lean Windows today, so the crate has no module per host.

mod failure;
mod format;
pub mod lean;
mod preview;
mod record;
mod settings;
mod shelf;

pub use failure::{Failure, Result};
pub use format::{
    DESCRIPTION_LONGEST, Guest, NAME_LONGEST, PACKAGES_MOST, Recipe, SCRIPT_LONGEST, SettingsAsked, Steps,
};
pub use preview::{origin_and_outcome, origin_text, summary, what_it_will_run};
pub use record::{
    CHOSEN_FILE, Chosen, OUTCOME_FILE, Outcome, read_chosen, read_outcome, write_chosen, write_outcome,
};
pub use settings::{Fitted, Limits, Usual, fit};
pub use shelf::{Entry, Listing, Source, find, list};

#[cfg(test)]
mod tests;
