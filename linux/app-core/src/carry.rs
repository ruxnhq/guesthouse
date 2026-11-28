//! The file a machine is carried to another PC in. Its format is every host's, in
//! `guesthouse_carry_core`; this keeps it where the export and the import have always found it.

pub use guesthouse_carry_core::{MAGIC, Member, Reader, Writer, safe_path};

/// What the format says when it can't read or write, in the same words.
impl From<guesthouse_carry_core::Failure> for crate::Failure {
    fn from(failure: guesthouse_carry_core::Failure) -> Self {
        Self(failure.0)
    }
}
