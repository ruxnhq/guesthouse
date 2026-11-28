//! What a lean Windows leaves out and switches off, each with the reason, in one place:
//! `guesthouse_recipes_core::lean` has the tables, and the words and the guest's data made from them.

#[cfg(test)]
use guesthouse_recipes_core::lean::NOT_ADDED;
pub use guesthouse_recipes_core::lean::{APPS, LEFT_ALONE, POLICIES, guest_data, what_it_does};

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
