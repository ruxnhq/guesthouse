//! Whether a newer Guesthouse is out. Each host fetches the file its releases publish, and this decides
//! what it says, with no OS or network calls, so all of them share it and it's tested anywhere.

/// Whether `latest` is a newer version than `current`, each three numbers.
pub fn newer(latest: &str, current: &str) -> bool {
    match (numbers(latest), numbers(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// Whether `version` is three numbers, the only versions a release has.
pub fn is_version(version: &str) -> bool {
    numbers(version).is_some()
}

fn numbers(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.trim().split('.').map(|part| {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())).then(|| part.parse().ok()).flatten()
    });
    let found = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_higher_version_is_newer() {
        assert!(newer("0.10.0", "0.9.0"), "by number, not by text");
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.9.0", "0.9.0"));
        assert!(!newer("0.8.5", "0.9.0"));
        assert!(!newer("0.10", "0.9.0"));
        assert!(!newer("0.10.0", "garbled"));
    }

    #[test]
    fn a_version_is_three_numbers() {
        assert!(is_version("0.10.0"));
        assert!(!is_version("0.10"));
        assert!(!is_version("0.10.0-rc1"));
        assert!(!is_version(""));
    }
}
