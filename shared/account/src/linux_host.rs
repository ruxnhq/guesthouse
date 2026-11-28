//! The Linux host's account: the common rules in [`crate::common`], which are its own, by the names
//! it has always had here, and its default names. The machine's kind picks the rule in `app-core`'s
//! `account::name_problem`.

pub use crate::common::{
    LINUX_RESERVED, MOST_LINUX_NAME_CHARS, MOST_PASSWORD_CHARS, MOST_WINDOWS_NAME_CHARS, WINDOWS_RESERVED,
    linux_name_problem, password_problem, windows_name_problem,
};

/// The Windows account, which signs in by itself.
pub const WINDOWS_USER: &str = "gpupv";
/// Ubuntu's desktop user.
pub const LINUX_USER: &str = "desk";

#[cfg(test)]
mod tests;
