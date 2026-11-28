//! Which host made a report, in the two places a report's own text says it: the `host` fact in the
//! summary, and the manifest's first sentence ("A Guesthouse bug report from a Windows host."). A
//! host passes its [`Host`] to [`crate::manifest::manifest_for`] and
//! [`crate::summary::Summary::set_host`], so both say the same host, and a new host names itself with
//! [`Host::new`] rather than a copy of either.
//!
//! Nothing else in a report names its host: the files a bundle holds and their names are each
//! host's own.

use super::summary::HOST_WINDOWS;

/// A host, as a report names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    word: Option<&'static str>,
    phrase: Option<&'static str>,
}

impl Host {
    /// The Linux host's reports name no host: no `host` fact, and "A Guesthouse bug report."
    pub const LINUX: Host = Host { word: None, phrase: None };
    /// The Windows host's: `host` is [`HOST_WINDOWS`], and the report is "from a Windows host".
    pub const WINDOWS: Host = Host { word: Some(HOST_WINDOWS), phrase: Some("a Windows host") };

    /// Another host: `word` is what the summary's `host` says (a lower-case word that sorts
    /// reports), and `phrase` ends the manifest's "A Guesthouse bug report from ..." Neither may be
    /// empty.
    pub const fn new(word: &'static str, phrase: &'static str) -> Self {
        assert!(!word.is_empty() && !phrase.is_empty(), "a host names itself in both places");
        Host { word: Some(word), phrase: Some(phrase) }
    }

    /// What the summary's `host` says, if anything.
    pub const fn word(self) -> Option<&'static str> {
        self.word
    }

    /// Who the manifest says the report is from, if anyone: the words after "from".
    pub const fn phrase(self) -> Option<&'static str> {
        self.phrase
    }
}
