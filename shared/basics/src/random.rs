//! Randomness as a value, so code that needs random bytes (a password, a tag, a file's name) can be
//! handed a source and tested with one that isn't random.
//!
//! Only the trait is here: where the bytes come from is each host's, as the standard library has no
//! source of its own. The Linux host's is its kernel's random device (`random::KernelRandom` in
//! `app-core`).

/// Where random bytes come from.
pub trait RandomSource: Send + Sync {
    /// Fills `bytes` with random ones. A source that can't is a broken system, so it panics rather
    /// than hand back bytes that aren't random.
    fn fill(&self, bytes: &mut [u8]);
}

#[cfg(test)]
mod tests;
