//! Cuts a long log down to its start and its end, which are where a run's story is: how it began, and
//! how it ended.

/// The start's share of what is kept, as a fraction: one part in this many. The end gets the rest, as
/// that's where a failure usually is.
const START_SHARE: usize = 8;

/// The log as it is if it fits in `most` bytes, and otherwise its start and its end, each cut at a line
/// break, with a line between them saying how much is left out.
pub fn trim(text: &str, most: usize) -> String {
    if text.len() <= most {
        return text.to_owned();
    }
    let start_end = line_end_before(text, most / START_SHARE);
    let end_start = line_start_after(text, text.len() - (most - most / START_SHARE));
    let left_out = end_start.saturating_sub(start_end);
    let start = &text[..start_end];
    // The note goes on a line of its own, whether or not the cut fell on a line break.
    let joint = if start.is_empty() || start.ends_with('\n') { "" } else { "\n" };
    format!("{start}{joint}[{left_out} bytes left out here]\n{}", &text[end_start..])
}

/// Where the last whole line before `at` ends, or `at` itself if there's no line break before it.
fn line_end_before(text: &str, at: usize) -> usize {
    let at = floor_boundary(text, at);
    text[..at].rfind('\n').map_or(at, |found| found + 1)
}

/// Where the first whole line after `at` starts, or `at` itself if there's no line break after it.
fn line_start_after(text: &str, at: usize) -> usize {
    let at = ceil_boundary(text, at);
    text[at..].find('\n').map_or(at, |found| at + found + 1)
}

fn floor_boundary(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil_boundary(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(lines: usize) -> String {
        (0..lines).map(|number| format!("line {number:05}\n")).collect()
    }

    #[test]
    fn a_short_log_stays_whole() {
        assert_eq!(trim("one\ntwo\n", 100), "one\ntwo\n");
    }

    #[test]
    fn a_long_log_keeps_its_start_and_its_end() {
        let log = numbered(10_000);
        let trimmed = trim(&log, 8_000);
        assert!(trimmed.len() <= 8_000 + 64, "{} bytes", trimmed.len());
        assert!(trimmed.starts_with("line 00000\n"));
        assert!(trimmed.ends_with("line 09999\n"));
        assert!(trimmed.contains("bytes left out here]"));
    }

    #[test]
    fn the_cuts_fall_on_whole_lines() {
        let trimmed = trim(&numbered(1_000), 1_000);
        for line in trimmed.lines() {
            assert!(line.starts_with("line ") && line.len() == 10 || line.starts_with('['), "{line:?}");
        }
    }

    #[test]
    fn the_end_gets_most_of_the_room() {
        let trimmed = trim(&numbered(10_000), 8_000);
        let (start, end) = trimmed.split_once("left out here]\n").unwrap();
        assert!(end.len() > start.len() * 4);
    }

    #[test]
    fn a_log_with_no_line_breaks_is_still_cut_on_characters() {
        let log = "é".repeat(5_000);
        let trimmed = trim(&log, 1_001);
        assert!(trimmed.len() < 1_100);
        assert!(trimmed.contains("left out here"));
    }
}
