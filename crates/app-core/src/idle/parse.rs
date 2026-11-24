//! What the guest's idle script says, read by our own markers and by layout. Windows words its own
//! output in the PC's language, so no word of it is looked for. A text that doesn't fit what was
//! expected is a reading that couldn't be taken, and never an empty one.

use super::{Input, PowerKind, PowerRequest, Signal, Unread};
use std::collections::BTreeMap;

/// The script's own words, which no Windows language changes.
const SESSIONS_TAG: &str = "sessions";
const POWER_TAG: &str = "power";
const INPUT_TAG: &str = "input";
const OK: &str = "ok";
const NOBODY: &str = "nobody";
/// A session's line, and a line of `powercfg`'s text, as the script prefixes them.
const SESSION_PREFIX: &str = "s|";
const POWER_PREFIX: &str = "p|";
/// What a session's state is when Windows' own numbers are used: a program in it, or a client behind it.
const STATE_ACTIVE: u32 = 0;
const STATE_CONNECTED: u32 = 1;
/// The name of a Remote Desktop session's station, which is a name of the protocol, not a word of a
/// language. A session that was disconnected has no station name any more.
const RDP_STATION: &str = "rdp-tcp#";
/// How much of a program's line is kept to say who asked.
const WHO_CHARS: usize = 80;
/// A section is its name and at least a line under it, which is "none" when nobody asked.
const LEAST_SECTION_LINES: usize = 2;
/// The sections that are told apart by their order, which `PowerKind::IN_ORDER` has.
const LEAST_SECTIONS: usize = PowerKind::IN_ORDER.len();

/// What the idle script said, signal by signal.
#[derive(Debug, Clone, PartialEq)]
pub struct GuestReport {
    pub remote_sessions: Signal<u32>,
    pub power_requests: Signal<Vec<PowerRequest>>,
    pub input: Signal<Input>,
}

impl GuestReport {
    fn unread(why: &str) -> Self {
        Self {
            remote_sessions: Err(Unread::new(why)),
            power_requests: Err(Unread::new(why)),
            input: Err(Unread::new(why)),
        }
    }
}

/// The guest's answer, which holds a part for each signal. A part that is missing or says it failed
/// is a signal that couldn't be read, and so is everything when there was no answer.
pub fn parse_report(text: &str) -> GuestReport {
    if text.trim().is_empty() {
        return GuestReport::unread("the guest's agent did not answer");
    }
    let mut parts: BTreeMap<&str, (String, Vec<&str>)> = BTreeMap::new();
    let mut current = None;
    for line in text.lines().map(|line| line.trim_end_matches('\r')) {
        if line.starts_with(SESSION_PREFIX) || line.starts_with(POWER_PREFIX) {
            if let Some((_, lines)) = current.and_then(|tag| parts.get_mut(tag)) {
                lines.push(line);
            }
            continue;
        }
        let mut words = line.split_whitespace();
        if let (Some(tag @ (SESSIONS_TAG | POWER_TAG | INPUT_TAG)), Some(status)) =
            (words.next(), words.next())
        {
            parts.insert(tag, (status.to_owned(), Vec::new()));
            current = Some(tag);
        }
    }
    if parts.is_empty() {
        return GuestReport::unread("the guest answered something the script does not say");
    }
    let part = |tag: &str, what: &str| -> Result<Vec<&str>, Unread> {
        match parts.get(tag) {
            Some((status, lines)) if status == OK => Ok(lines.clone()),
            Some(_) => Err(Unread::new(format!("the guest could not read {what}"))),
            None => Err(Unread::new(format!("the guest did not say {what}"))),
        }
    };
    GuestReport {
        remote_sessions: part(SESSIONS_TAG, "its sessions").and_then(|lines| parse_sessions(&lines)),
        power_requests: part(POWER_TAG, "its power requests").and_then(|lines| parse_power(&lines)),
        input: parse_input(parts.get(INPUT_TAG)),
    }
}

/// `input SECONDS`, `input nobody`, or `input failed` when the helper in the user's session said
/// nothing.
fn parse_input(part: Option<&(String, Vec<&str>)>) -> Signal<Input> {
    let Some((status, _)) = part else { return Err(Unread::new("the guest did not say its last input")) };
    if status == NOBODY {
        return Ok(Input::NobodyLoggedIn);
    }
    status
        .parse()
        .map(Input::Seconds)
        .map_err(|_| Unread::new("nothing in the user's session said when the last input was"))
}

/// How many Remote Desktop sessions are connected, from lines of `s|ID|STATE|STATION`. Session 0 is
/// Windows' own and is always there, so a text with no line at all is not one that was read.
pub fn parse_sessions(lines: &[&str]) -> Signal<u32> {
    let mut seen = 0;
    let mut remote = 0;
    for line in lines {
        let fields: Vec<&str> = line.strip_prefix(SESSION_PREFIX).unwrap_or(line).split('|').collect();
        let [id, state, station] = fields[..] else {
            return Err(Unread::new("a session was listed in a form that was not expected"));
        };
        let (Ok(_), Ok(state)) = (id.parse::<u32>(), state.parse::<u32>()) else {
            return Err(Unread::new("a session was listed in a form that was not expected"));
        };
        seen += 1;
        let connected = state == STATE_ACTIVE || state == STATE_CONNECTED;
        if connected && station.to_ascii_lowercase().starts_with(RDP_STATION) {
            remote += 1;
        }
    }
    if seen == 0 {
        return Err(Unread::new("no session was listed"));
    }
    Ok(remote)
}

/// The requests in `powercfg /requests`, from its lines, with or without our prefix on each. The sections
/// are told apart by their order and the requests in them by their layout: a section with a request
/// has bracketed lines or more than one line, and one with none has a single line that every such
/// section has alike.
pub fn parse_power(lines: &[&str]) -> Signal<Vec<PowerRequest>> {
    let layout = || Unread::new("the power requests were not laid out as expected");
    let sections = sections(lines);
    if sections.len() < LEAST_SECTIONS || sections.iter().any(|body| body.is_empty()) {
        return Err(layout());
    }
    let none = what_none_says(&sections);
    let mut found = Vec::new();
    for (index, section) in sections.iter().enumerate() {
        let single = section.len() == 1 && !section[0].starts_with('[');
        if single && Some(section[0].as_str()) == none.as_deref() {
            continue;
        }
        if single && none.is_none() {
            // Nothing to say what "none" looks like in this language, and this isn't a request by its layout.
            return Err(layout());
        }
        let Some(kind) = PowerKind::IN_ORDER.get(index).copied() else { continue };
        let entries: Vec<&String> = section.iter().filter(|line| line.starts_with('[')).collect();
        let whos = if entries.is_empty() { vec![&section[0]] } else { entries };
        found.extend(
            whos.into_iter().map(|who| PowerRequest { kind, who: who.chars().take(WHO_CHARS).collect() }),
        );
    }
    Ok(found)
}

/// The lines under each section's name, which is its first line, with the blank lines between the
/// sections taken out.
fn sections(lines: &[&str]) -> Vec<Vec<String>> {
    let mut sections: Vec<Vec<String>> = Vec::new();
    let mut block: Vec<String> = Vec::new();
    for line in lines.iter().chain(std::iter::once(&"")) {
        let line = line.strip_prefix(POWER_PREFIX).unwrap_or(line).trim();
        if line.is_empty() {
            if !block.is_empty() {
                sections.push(std::mem::take(&mut block));
            }
        } else {
            block.push(line.to_owned());
        }
    }
    // Under its name, or the section was only a name.
    sections
        .into_iter()
        .map(|block| if block.len() >= LEAST_SECTION_LINES { block[1..].to_vec() } else { Vec::new() })
        .collect()
}

/// What a section with no request says: the one line that more than one section has alike, and no
/// other line does as often.
fn what_none_says(sections: &[Vec<String>]) -> Option<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for section in sections.iter().filter(|section| section.len() == 1 && !section[0].starts_with('[')) {
        *counts.entry(section[0].as_str()).or_default() += 1;
    }
    let most = counts.values().copied().max().filter(|most| *most >= 2)?;
    let mut top = counts.iter().filter(|(_, count)| **count == most);
    let (text, _) = top.next()?;
    top.next().is_none().then(|| (*text).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As `powercfg /requests` lays out a PC where nothing asks, from memory of its format and not a capture.
    const NOTHING_ENGLISH: &str = "DISPLAY:\nNone.\n\nSYSTEM:\nNone.\n\nAWAYMODE:\nNone.\n\nEXECUTION:\nNone.\n\nPERFBOOST:\nNone.\n\nACTIVELOCKSCREEN:\nNone.\n\n";
    /// The same with every word in another language, and none of them in ours.
    const NOTHING_GERMAN: &str = "ANZEIGE:\nKeine.\n\nSYSTEM:\nKeine.\n\nABWESENHEITSMODUS:\nKeine.\n\nAUSF\u{dc}HRUNG:\nKeine.\n\nPERFBOOST:\nKeine.\n\nACTIVELOCKSCREEN:\nKeine.\n\n";
    const ASKING: &str = "DISPLAY:\nNone.\n\nSYSTEM:\n[PROCESS] \\Device\\HarddiskVolume3\\Program Files\\VideoLAN\\vlc.exe\nPlaying video\n\nAWAYMODE:\nNone.\n\nEXECUTION:\n[SERVICE] BITS\n\nPERFBOOST:\nNone.\n\nACTIVELOCKSCREEN:\nNone.\n";

    fn lines(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    #[test]
    fn a_pc_where_nothing_asks_has_no_requests_in_any_language() {
        assert_eq!(parse_power(&lines(NOTHING_ENGLISH)), Ok(Vec::new()));
        assert_eq!(parse_power(&lines(NOTHING_GERMAN)), Ok(Vec::new()));
    }

    #[test]
    fn requests_are_found_by_their_section_and_their_brackets() {
        let found = parse_power(&lines(ASKING)).unwrap();
        let summary: Vec<(PowerKind, bool)> =
            found.iter().map(|request| (request.kind, request.who.starts_with('['))).collect();
        assert_eq!(summary, [(PowerKind::System, true), (PowerKind::Execution, true)]);
        assert!(found[0].who.contains("vlc.exe"));
    }

    #[test]
    fn a_request_whose_tag_has_no_brackets_is_still_one_by_its_section() {
        let text = NOTHING_GERMAN.replace("SYSTEM:\nKeine.", "SYSTEM:\nProzess vlc.exe\nVideo");
        let found = parse_power(&lines(&text)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, PowerKind::System);
        assert_eq!(found[0].who, "Prozess vlc.exe");
    }

    #[test]
    fn requests_after_the_first_four_sections_are_not_counted() {
        let text = NOTHING_ENGLISH.replace("PERFBOOST:\nNone.", "PERFBOOST:\n[PROCESS] game.exe");
        assert_eq!(parse_power(&lines(&text)), Ok(Vec::new()));
    }

    #[test]
    fn a_text_that_is_not_laid_out_like_the_sections_is_not_read() {
        for text in [
            "",
            "Access is denied.\n",
            "Run this as administrator\nto see the requests\n",
            // Four sections that say four different things: nothing says what "none" looks like.
            "A:\none\n\nB:\ntwo\n\nC:\nthree\n\nD:\nfour\n",
            // A section that is only its name.
            "A:\nNone.\n\nB:\n\nC:\nNone.\n\nD:\nNone.\n",
        ] {
            assert!(parse_power(&lines(text)).is_err(), "{text:?}");
        }
    }

    #[test]
    fn the_scripts_prefix_on_each_line_is_taken_off() {
        let prefixed: Vec<String> = ASKING.lines().map(|line| format!("p|{line}")).collect();
        let lines: Vec<&str> = prefixed.iter().map(String::as_str).collect();
        assert_eq!(parse_power(&lines).unwrap().len(), 2);
    }

    #[test]
    fn only_connected_remote_desktop_sessions_count() {
        let sessions = |text: &str| parse_sessions(&lines(text));
        assert_eq!(sessions("s|0|4|Services\ns|1|1|Console\ns|65536|4|"), Ok(0));
        assert_eq!(sessions("s|0|4|Services\ns|1|1|Console\ns|2|0|RDP-Tcp#3"), Ok(1));
        assert_eq!(sessions("s|1|0|Console\ns|2|4|\ns|3|1|rdp-tcp#5\ns|4|0|RDP-Tcp#6"), Ok(2));
        assert_eq!(sessions("s|2|4|"), Ok(0), "a disconnected one has no station and does not count");
        assert_eq!(sessions("s|2|4|RDP-Tcp#3"), Ok(0), "nor does one left by its state");
    }

    #[test]
    fn sessions_in_a_form_that_is_not_expected_are_not_read() {
        for text in ["", "s|1|1", "s|x|1|Console", "s|1|y|Console", "s|1|1|a|b"] {
            assert!(parse_sessions(&lines(text)).is_err(), "{text:?}");
        }
    }

    fn answer(sessions: &str, power: &str, input: &str) -> String {
        let prefixed = |prefix: &str, text: &str| -> String {
            text.lines().map(|line| format!("{prefix}{line}\n")).collect()
        };
        format!(
            "{sessions}\n{}{power}\n{}{input}\n",
            prefixed("s|", "0|4|Services\n1|1|Console"),
            prefixed("p|", NOTHING_ENGLISH)
        )
    }

    #[test]
    fn a_whole_answer_gives_each_signal() {
        let report = parse_report(&answer("sessions ok", "power ok", "input 125"));
        assert_eq!(report.remote_sessions, Ok(0));
        assert_eq!(report.power_requests, Ok(Vec::new()));
        assert_eq!(report.input, Ok(Input::Seconds(125)));
        let nobody = parse_report(&answer("sessions ok", "power ok", "input nobody"));
        assert_eq!(nobody.input, Ok(Input::NobodyLoggedIn));
    }

    #[test]
    fn a_part_the_guest_could_not_read_is_unread_and_the_others_still_are() {
        let report = parse_report(&answer("sessions failed", "power ok", "input failed"));
        assert!(report.remote_sessions.is_err());
        assert!(report.input.is_err());
        assert_eq!(report.power_requests, Ok(Vec::new()));
        // A part's lines under a part that failed are not read.
        let report = parse_report(&answer("sessions ok", "power failed", "input 5"));
        assert!(report.power_requests.is_err());
        assert_eq!(report.input, Ok(Input::Seconds(5)));
    }

    #[test]
    fn no_answer_and_an_answer_that_is_not_ours_read_nothing() {
        for text in ["", "  \n", "ERROR: the term is not recognized", "sessions\ninput\n"] {
            let report = parse_report(text);
            assert!(report.remote_sessions.is_err() && report.power_requests.is_err(), "{text:?}");
            assert!(report.input.is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_missing_part_is_unread() {
        let report = parse_report("sessions ok\ns|0|4|Services\ns|1|1|Console\n");
        assert_eq!(report.remote_sessions, Ok(0));
        assert!(report.power_requests.is_err() && report.input.is_err());
    }
}
