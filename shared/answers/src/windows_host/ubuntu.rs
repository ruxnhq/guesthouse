//! What an Ubuntu guest's install is given on the Windows host beyond its static answers.

/// The line `install.sh` reads from the files disk, `user password`, and makes the guest's
/// administrator from. The script deletes it once read.
pub fn account(user: &str, password: &str) -> String {
    format!("{user} {password}\n")
}
