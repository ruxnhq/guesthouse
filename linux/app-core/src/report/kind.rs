//! What sort of problem the tester says it is. It never takes anything out of a report. It only adds
//! what fits that sort of problem, and it goes into the summary so reports can be sorted. Every host
//! has the same kinds, so they're `guesthouse-report-core`'s.

pub use guesthouse_report_core::kind::ProblemKind;

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
