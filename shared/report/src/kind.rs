//! What sort of problem the tester says it is. It never takes anything out of a report. It only adds
//! what fits that sort of problem, and it goes into the summary so reports can be sorted. Every host
//! uses these words, so one server sorts them all.

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

    /// The word for it in the summary, and on the Linux host's command line.
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

    /// The guest's install log, which is only worth its room when the install is what failed. The
    /// Linux host's gathering asks this.
    pub fn wants_install_log(self) -> bool {
        self == Self::InstallFailed
    }

    /// Crash records of the programs that run a machine too, and from further back, beyond the
    /// app's own. The Linux host's gathering asks this.
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

    #[test]
    fn the_words_and_labels_are_the_linux_hosts() {
        // Pinned, as the hosts' reports are sorted by these on one server.
        let words: Vec<&str> = ProblemKind::ALL.iter().map(|kind| kind.word()).collect();
        assert_eq!(words, ["wont-start", "crashed", "picture", "slow", "install-failed", "other"]);
        let labels: Vec<&str> = ProblemKind::ALL.iter().map(|kind| kind.label()).collect();
        assert_eq!(
            labels,
            ["Won't start", "Crashed", "Picture is wrong", "Slow", "Install failed", "Something else"]
        );
    }

    #[test]
    fn only_a_failed_install_wants_the_install_log_and_only_a_crash_every_crash() {
        for kind in ProblemKind::ALL {
            assert_eq!(kind.wants_install_log(), kind == ProblemKind::InstallFailed);
            assert_eq!(kind.wants_every_crash(), kind == ProblemKind::Crashed);
        }
    }

    #[test]
    fn the_default_is_something_else() {
        assert_eq!(ProblemKind::default(), ProblemKind::SomethingElse);
    }
}
