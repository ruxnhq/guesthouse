// SPDX-License-Identifier: GPL-2.0-or-later
//! The password of a guest's account. The app makes it (random, readable, no letters that look
//! alike), keeps it in the Mac's Keychain and sends it with the install; this only checks that
//! what arrives is safe to write into the answer files, where it is put at generation time and
//! nowhere else. The shared rule for every host's password comes first; the Mac asks for plainer
//! characters on top, since the app's own passwords are always of that kind.

use guesthouse_account_core::common::password_problem;

const LEAST: usize = 8;
const MOST: usize = 64;

/// Letters, digits and dashes only, so no quoting is ever needed in XML, YAML or a shell line.
pub fn check(password: &str) -> Result<(), String> {
    if let Some(problem) = password_problem(password) {
        return Err(problem);
    }
    let fits = (LEAST..=MOST).contains(&password.len())
        && password.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if fits {
        Ok(())
    } else {
        Err(format!("The password must be {LEAST} to {MOST} letters, digits or dashes."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_characters_in_a_sensible_length_are_taken() {
        assert!(check("k7mn-p3xq-ab9d-w4ht").is_ok());
        assert!(check("short").is_err());
        assert!(check("has a space in it").is_err());
        assert!(check("quote'quote-quote").is_err());
        assert!(check(&"a".repeat(65)).is_err());
    }
}
