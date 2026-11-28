//! A recipe file, read and checked, is `guesthouse_recipes_core`'s. What stays here is which kind of
//! machine on this host a recipe's guest is, as the kinds are this host's.

use crate::machine::Kind;
pub use guesthouse_recipes_core::{Guest, Recipe, SettingsAsked, Steps};
#[cfg(test)]
use guesthouse_recipes_core::{NAME_LONGEST, PACKAGES_MOST, SCRIPT_LONGEST};

/// A recipe's guest as a kind of machine here.
pub trait GuestKind: Sized {
    fn kind(self) -> Kind;

    /// The machine kind's recipes, if it has any: a plain VM and a Linux the user installed have no
    /// way in for us.
    fn of_kind(kind: Kind) -> Option<Self>;
}

impl GuestKind for Guest {
    fn kind(self) -> Kind {
        match self {
            Self::Windows => Kind::Windows,
            Self::Ubuntu => Kind::Linux,
        }
    }

    fn of_kind(kind: Kind) -> Option<Self> {
        match kind {
            Kind::Windows => Some(Self::Windows),
            Kind::Linux => Some(Self::Ubuntu),
            Kind::Plain => None,
        }
    }
}

/// The kind of machine a recipe is for.
pub trait RecipeKind {
    fn kind(&self) -> Kind;
}

impl RecipeKind for Recipe {
    fn kind(&self) -> Kind {
        self.guest.kind()
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
