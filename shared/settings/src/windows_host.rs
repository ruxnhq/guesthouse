//! The Windows host's checks of a machine's settings that take plain values: the machine's name,
//! which its folder and its VM take as it is, and the Windows edition named for an install. The app
//! and the service both refuse with them, through `control`.

/// Whether `name` can name a machine: its folder and its VM take it as it is.
pub fn valid_name(name: &str) -> bool {
    const LONGEST: usize = 40;
    !name.is_empty()
        && name.len() <= LONGEST
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `edition` can name an edition: Windows' editions are letters, digits and spaces.
pub fn valid_edition(edition: &str) -> bool {
    const LONGEST: usize = 64;
    edition.len() <= LONGEST && edition.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ')
}

#[cfg(test)]
mod tests;
