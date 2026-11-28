//! Cuts a long log down to its start and its end, which are where a run's story is: how it began, and
//! how it ended. From a daemon's log, the lines between them that aren't debug lines stay too, as
//! they say what happened in the middle.

use guesthouse_daemon::logger::{Keeper, starts_a_record};
use std::collections::VecDeque;
use std::ops::Range;

/// The start's share of what is kept, as a fraction: one part in this many. The end gets the rest, as
/// that's where a failure usually is.
const START_SHARE: usize = 8;
/// The most the middle's lines worth keeping take, as a fraction: one part in this many. They take it
/// from the end, and only what they need.
const MIDDLE_SHARE: usize = 8;

/// The log as it is if it fits in `most` bytes, and otherwise its start and its end, each cut at a line
/// break, with a line between them saying how much is left out. A daemon's log keeps the lines of
/// what's left out that a [`Keeper`] keeps, the newest of them, after that line.
pub fn trim(text: &str, most: usize) -> String {
    if text.len() <= most {
        return text.to_owned();
    }
    let start_end = line_end_before(text, most / START_SHARE);
    let end_room = most - most / START_SHARE;
    let end_from = |room: usize| line_start_after(text, text.len() - room).max(start_end);
    // They are looked for as far as the end would start if they took all their room.
    let room = most / MIDDLE_SHARE;
    let looked_at = start_end..end_from(end_room - room);
    let mut kept = kept_lines(text, looked_at, room);
    let mut end_start = end_from(end_room - kept.iter().map(Range::len).sum::<usize>());
    // Those the end holds anyway stay there.
    kept.retain(|line| line.end <= end_start);
    if kept.is_empty() {
        end_start = end_from(end_room);
    }
    let start = &text[..start_end];
    // The note goes on a line of its own, whether or not the cut fell on a line break.
    let joint = if start.is_empty() || start.ends_with('\n') { "" } else { "\n" };
    let end = &text[end_start..];
    if kept.is_empty() {
        let left_out = end_start.saturating_sub(start_end);
        return format!("{start}{joint}[{left_out} bytes left out here]\n{end}");
    }
    let lines: String = kept.into_iter().map(|line| &text[line]).collect();
    let left_out = end_start.saturating_sub(start_end) - lines.len();
    let which = match lines.matches('\n').count() {
        1 => "1 line below that isn't a debug line or names".to_owned(),
        count => format!("{count} lines below that aren't debug lines or name"),
    };
    format!(
        "{start}{joint}[{left_out} bytes left out here, all but the {which} a guest program]\n{lines}\
         [the log's end follows]\n{end}"
    )
}

/// Where the lines in `looked_at` that a [`Keeper`] keeps lie, the newest that fit in `room` bytes,
/// oldest first. A line with no level goes with the line before it, and they're let go together.
fn kept_lines(text: &str, looked_at: Range<usize>, room: usize) -> Vec<Range<usize>> {
    let mut keeper = Keeper::default();
    let mut records: VecDeque<Range<usize>> = VecDeque::new();
    let (mut held, mut at) = (0, looked_at.start);
    for line in text[looked_at].split_inclusive('\n') {
        let span = at..at + line.len();
        at = span.end;
        if !keeper.keeps(line.as_bytes()) {
            continue;
        }
        held += span.len();
        match records.back_mut() {
            Some(record) if !starts_a_record(line.as_bytes()) && record.end == span.start => {
                record.end = span.end
            }
            _ => records.push_back(span),
        }
        while held > room {
            let Some(oldest) = records.pop_front() else { break };
            held -= oldest.len();
        }
    }
    records.into()
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

    /// A daemon's verbose log of `lines` debug lines, with others put in at the given lines.
    fn verbose(lines: usize, others: &[(usize, &str)]) -> String {
        let mut log = String::new();
        for number in 0..lines {
            match others.iter().find(|(at, _)| *at == number) {
                Some((_, other)) => log.push_str(other),
                None => log.push_str(&format!(
                    "DEBUG [{number:>9}] message Some(QueryAdapter) process=0x400012c0\n"
                )),
            }
        }
        log
    }

    #[test]
    fn a_daemons_log_keeps_the_lines_of_its_middle_that_say_what_happened() {
        let log = verbose(
            10_000,
            &[
                (3_000, "DEBUG [     3000] process 0x40181cc0 is guest pid 11900, Heaven.exe\n"),
                (5_000, "WARN  [     5000] intel work: queue 0x400e93c0 stops: it would run on\n"),
                (5_001, "  the rest of that warning\n"),
                (7_000, "INFO  [     7000] intel context process=0x40181cc0 node=1 node_name=video\n"),
            ],
        );
        let trimmed = trim(&log, 80_000);
        assert!(trimmed.len() <= 80_000 + 200, "{} bytes", trimmed.len());
        assert!(
            trimmed.starts_with("DEBUG [        0]")
                && trimmed.ends_with("[     9999] message Some(QueryAdapter) process=0x400012c0\n")
        );
        let (_, rest) = trimmed.split_once("bytes left out here, all but the 4 lines below").unwrap();
        let (kept, end) = rest.split_once("[the log's end follows]\n").unwrap();
        assert_eq!(
            kept,
            " that aren't debug lines or name a guest program]\n\
             DEBUG [     3000] process 0x40181cc0 is guest pid 11900, Heaven.exe\n\
             WARN  [     5000] intel work: queue 0x400e93c0 stops: it would run on\n  \
             the rest of that warning\n\
             INFO  [     7000] intel context process=0x40181cc0 node=1 node_name=video\n"
        );
        assert!(log.ends_with(end));
        // What was left out is said in bytes, which add up.
        let left_out: usize =
            trimmed.split(" bytes left out").next().unwrap().rsplit('[').next().unwrap().parse().unwrap();
        let start = trimmed.split("\n[").next().unwrap().len() + 1;
        assert_eq!(start + left_out + kept.len() - kept.find('\n').unwrap() - 1 + end.len(), log.len());
    }

    #[test]
    fn a_line_the_end_holds_anyway_is_not_kept_twice() {
        let log = verbose(10_000, &[(9_950, "WARN  [     9950] near the end\n")]);
        let trimmed = trim(&log, 80_000);
        assert_eq!(trimmed.matches("near the end").count(), 1);
        assert!(trimmed.contains("[") && !trimmed.contains("all but the"), "nothing else was kept");
    }

    #[test]
    fn the_middles_lines_take_no_more_than_their_room_and_the_newest_stay() {
        let others: Vec<(usize, String)> = (1_000..9_000)
            .step_by(10)
            .map(|at| (at, format!("INFO  [{at:>9}] a line worth keeping\n")))
            .collect();
        let others: Vec<(usize, &str)> = others.iter().map(|(at, line)| (*at, line.as_str())).collect();
        let log = verbose(10_000, &others);
        let trimmed = trim(&log, 80_000);
        assert!(trimmed.len() <= 80_000 + 200, "{} bytes", trimmed.len());
        let kept: Vec<&str> = trimmed.lines().filter(|line| line.contains("worth keeping")).collect();
        assert!(kept.len() < 800, "{}", kept.len());
        assert!(kept.last().unwrap().contains("[     8990]"), "the newest stay: {:?}", kept.last());
    }
}
