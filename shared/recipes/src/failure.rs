//! What reading a recipe, or keeping what a machine was made with, says when it can't: a sentence a
//! person can be told, as the Linux host's own errors are. A host turns it into its own error with the
//! same words.

use std::fmt;
use std::io;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(pub String);

pub type Result<T> = std::result::Result<T, Failure>;

impl Failure {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failure {}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}
