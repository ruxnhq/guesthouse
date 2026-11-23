//! Ready-made machines: a recipe is a small text file that says what a machine is for. It can set a
//! few of the machine's settings when it's made, and give the guest software once it has installed.
//!
//! `format` reads and checks a recipe, `shelf` finds the ones that come with the app and the user's
//! own, `preview` says in words what one will do, `record` keeps what a machine was made with and how
//! its steps went, and `settings` fits a recipe's settings to this PC's limits. The steps themselves
//! run in `actions/recipe`. [docs/recipes.md](../../../../docs/recipes.md) is the file format.

mod format;
mod preview;
mod record;
mod settings;
mod shelf;

pub use format::{Guest, Recipe, SettingsAsked, Steps};
pub use preview::{origin_text, summary, what_it_will_run};
pub use record::{
    CHOSEN_FILE, Chosen, OUTCOME_FILE, Outcome, read_chosen, read_outcome, write_chosen, write_outcome,
};
pub use settings::{Fitted, Usual, fit};
pub use shelf::{Entry, Listing, Source, find, list, list_for};
