//! Ready-made machines: a recipe is a small text file that says what a machine is for. It can set a
//! few of the machine's settings when it's made, and give the guest software once it has installed.
//!
//! `format` reads and checks a recipe, `shelf` finds the ones that come with the app and the user's
//! own, `preview` says in words what one will do, `record` keeps what a machine was made with and how
//! its steps went, and `settings` fits a recipe's settings to this PC's limits. The steps themselves
//! run in `actions/recipe`. [docs/recipes.md](../../../../docs/recipes.md) is the file format.
//!
//! The rules and the words are `guesthouse_recipes_core`'s, which every host can build on. This
//! module keeps the names they have always had here, and adds what is this host's: its folders, its
//! kinds of machine, its machine folders and its PC's limits.

mod format;
mod preview;
mod record;
mod settings;
mod shelf;

pub use format::{Guest, GuestKind, Recipe, RecipeKind, SettingsAsked, Steps};
pub use preview::{origin_and_outcome, origin_text, summary, what_it_will_run};
pub use record::{
    CHOSEN_FILE, Chosen, OUTCOME_FILE, Outcome, read_chosen, read_outcome, write_chosen, write_outcome,
};
pub use settings::{Fitted, Usual, fit};
pub use shelf::{Entry, Listing, Source, find, list, list_for};

impl From<guesthouse_recipes_core::Failure> for crate::Failure {
    fn from(failure: guesthouse_recipes_core::Failure) -> Self {
        Self(failure.0)
    }
}
