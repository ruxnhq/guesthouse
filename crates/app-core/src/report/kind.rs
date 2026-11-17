//! What sort of problem the tester says it is. It never takes anything out of a report. It only adds
//! what fits that sort of problem, and it goes into the summary so reports can be sorted.

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

    /// The word for it on the command line and in the summary.
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

    /// The guest's install log, which is only worth its room when the install is what failed.
    pub fn wants_install_log(self) -> bool {
        self == Self::InstallFailed
    }

    /// Crash records of QEMU and swtpm too, and from further back, beyond the app's own programs.
    pub fn wants_every_crash(self) -> bool {
        self == Self::Crashed
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
}
