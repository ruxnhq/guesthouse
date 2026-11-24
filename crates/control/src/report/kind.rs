//! What sort of problem the tester says it is. It never takes anything out of a report. It only adds
//! what fits that sort of problem, and it goes into the summary so reports can be sorted. The words
//! are the Linux host's, so one server sorts both.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProblemKind {
    WontStart,
    Crashed,
    WrongPicture,
    Slow,
    InstallFailed,
    #[default]
    SomethingElse,
}

impl ProblemKind {
    pub const ALL: [Self; 6] = [
        Self::WontStart,
        Self::Crashed,
        Self::WrongPicture,
        Self::Slow,
        Self::InstallFailed,
        Self::SomethingElse,
    ];

    /// The word for it in the summary.
    pub fn word(self) -> &'static str {
        match self {
            Self::WontStart => "wont-start",
            Self::Crashed => "crashed",
            Self::WrongPicture => "picture",
            Self::Slow => "slow",
            Self::InstallFailed => "install-failed",
            Self::SomethingElse => "other",
        }
    }

    /// What the tester sees.
    pub fn label(self) -> &'static str {
        match self {
            Self::WontStart => "Won't start",
            Self::Crashed => "Crashed",
            Self::WrongPicture => "Picture is wrong",
            Self::Slow => "Slow",
            Self::InstallFailed => "Install failed",
            Self::SomethingElse => "Something else",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.word() == word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_reads_back_from_its_word() {
        for kind in ProblemKind::ALL {
            assert_eq!(ProblemKind::from_word(kind.word()), Some(kind));
        }
        assert_eq!(ProblemKind::from_word("broken"), None);
    }

    #[test]
    fn the_words_and_labels_are_the_linux_hosts() {
        let linux = include_str!("../../../app-core/src/report/kind.rs");
        for kind in ProblemKind::ALL {
            assert!(linux.contains(&format!("=> \"{}\",", kind.word())), "{}", kind.word());
            assert!(linux.contains(&format!("=> \"{}\",", kind.label())), "{}", kind.label());
        }
    }

    #[test]
    fn the_default_is_something_else() {
        assert_eq!(ProblemKind::default(), ProblemKind::SomethingElse);
    }
}
