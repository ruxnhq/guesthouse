//! Where this host's recipes are found: the ones that come with the app, and the user's own in a folder
//! of the app's config. Reading the folders is `guesthouse_recipes_core`'s.

use super::format::RecipeKind;
use crate::context::Context;
use crate::failure::Result;
use crate::machine::Kind;
pub use guesthouse_recipes_core::{Entry, Listing, Source};

/// Every recipe there is, the app's first.
pub fn list(context: &Context) -> Listing {
    guesthouse_recipes_core::list(&context.paths.shipped_recipes(), &context.paths.your_recipes())
}

/// The recipes a machine of this kind can have. A kind with none gets none.
pub fn list_for(context: &Context, kind: Kind) -> Listing {
    let mut listing = list(context);
    listing.entries.retain(|entry| entry.recipe.kind() == kind);
    listing
}

/// The recipe a person named, by its key, its file's name or its name in any case. Two that fit are
/// told apart by their keys.
pub fn find(context: &Context, kind: Kind, wanted: &str) -> Result<Entry> {
    Ok(guesthouse_recipes_core::find(list_for(context, kind), wanted)?)
}

#[cfg(test)]
#[path = "shelf_tests.rs"]
mod tests;
