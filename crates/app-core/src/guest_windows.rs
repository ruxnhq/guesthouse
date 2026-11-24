//! The windows a running Windows guest has open, as its helper in the user's session reports them.
//! Only the helper can see them, as a window belongs to the session's desktop. This is the host's
//! end: it starts the helper through the agent, asks it, and reads the answer. It changes nothing in
//! the guest.

use crate::agent::{QUICK_SECONDS, helper_start, stop_helper};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::Machine;
use crate::text::base64_decode;
use guesthouse_daemon::slot::Slot;
use serde::Deserialize;

/// The helper's file, start script and task share this name. It listens on 8769 in the guest, on
/// loopback only, so no port is forwarded: the agent reaches it, as it reaches the window-size helper.
const WINDOWS_HELPER: &str = "windows";
const START_SECONDS: u64 = 20;
/// Listing takes the helper a few milliseconds. The rest is the agent's own round trip.
const LIST_SECONDS: u64 = QUICK_SECONDS;
const LIST_REQUEST: &str = "list";
/// What the helper's ask script says before the helper's own answer.
const ANSWER_WORD: &str = "answer";
const STARTED_WORD: &str = "started";
const NOBODY_WORD: &str = "nobody is logged in";

/// A rectangle in the guest's screen, in pixels, as Windows gives it: right and bottom are the first
/// pixel outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GuestWindow {
    /// Windows' own handle for it, which is how a later request would name it. A number that is only
    /// good while the window lives.
    pub handle: i64,
    /// The window it belongs to, or 0. A menu, a tooltip or a dialog is owned by the window it came from.
    pub owner: i64,
    pub pid: u32,
    /// The program's file name. Empty when Windows would not let the helper look.
    pub exe: String,
    pub class: String,
    pub title: String,
    /// What the user sees of the window, without the invisible border Windows draws around it. This
    /// is the one to crop the picture by.
    pub frame: Bounds,
    /// What Windows says the window occupies, border included.
    pub rect: Bounds,
    /// 0 is the topmost window.
    pub z: u32,
    pub minimized: bool,
    pub tool_window: bool,
    /// A menu, tooltip or drop-down: a window of its own that belongs to the one in `owner`.
    pub popup: bool,
    pub foreground: bool,
    /// The helper leaves cloaked windows out, so this is only ever set by a list that was not made by
    /// it as it is today. Such a window is one the user can't see, and is dropped.
    #[serde(default)]
    cloaked: bool,
}

/// What the helper answered: its own JSON as it came, and the windows read from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub json: String,
    pub windows: Vec<GuestWindow>,
}

#[derive(Deserialize)]
struct Answer {
    windows: Vec<GuestWindow>,
}

/// The windows in the helper's JSON, topmost first as the helper has them.
pub fn parse_windows(json: &str) -> Result<Vec<GuestWindow>> {
    let answer: Answer = serde_json::from_str(json)
        .map_err(|error| Failure(format!("The guest's list of windows could not be read: {error}.")))?;
    Ok(answer.windows.into_iter().filter(|window| !window.cloaked).collect())
}

/// One line for a window, for a person. A title can hold anything, line breaks included, so it is
/// written the way a program writes a string.
pub fn describe(window: &GuestWindow) -> String {
    let exe = if window.exe.is_empty() { "(unknown program)" } else { &window.exe };
    let frame = &window.frame;
    let mut notes = Vec::new();
    for (set, note) in [
        (window.foreground, "foreground"),
        (window.minimized, "minimized"),
        (window.tool_window, "tool window"),
        (window.popup, "popup"),
    ] {
        if set {
            notes.push(note.to_owned());
        }
    }
    if window.owner != 0 {
        notes.push(format!("owner {:#x}", window.owner));
    }
    let notes = if notes.is_empty() { String::new() } else { format!(" [{}]", notes.join(", ")) };
    format!(
        "{} {:#x} {exe} {:?} {}x{} at {},{}{notes}",
        window.z,
        window.handle,
        window.title,
        frame.width(),
        frame.height(),
        frame.left,
        frame.top,
    )
}

/// What the start said. The helper takes a second or two to compile before it listens.
fn start(context: &Context, machine: &Machine, slot: Option<Slot>) -> Result<()> {
    let script = helper_start(context, WINDOWS_HELPER)?;
    let said = context.ask_guest(machine, &script, START_SECONDS, slot);
    if said.contains(STARTED_WORD) {
        return Ok(());
    }
    let name = &machine.name;
    Err(Failure(if said.contains(NOBODY_WORD) {
        format!("Nobody is logged in to {name}, so it has no windows.")
    } else {
        format!("The helper that lists {name}'s windows did not start.")
    }))
}

/// The helper's JSON, or `None` if nothing is listening for it. The agent wraps long lines, and the
/// answer is Base64, which has no spaces, so what is left of the whitespace goes.
fn ask(context: &Context, machine: &Machine, slot: Option<Slot>) -> Result<Option<String>> {
    let script =
        format!("$request = '{LIST_REQUEST}'\n{}", crate::agent::guest_script(context, "windows-ask.ps1")?);
    let answer = context.ask_guest(machine, &script, LIST_SECONDS, slot);
    let Some(said) = answer.trim().strip_prefix(ANSWER_WORD) else { return Ok(None) };
    let encoded: String = said.split_whitespace().collect();
    if encoded.is_empty() {
        // It heard the request and didn't answer, which is what it does with one it can't read.
        return Ok(None);
    }
    let bytes =
        base64_decode(&encoded).ok_or_else(|| Failure::new("The guest's list of windows was cut short."))?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Asks the machine's helper for its windows. A helper that isn't running is started for the one
/// question and ended again, so a look leaves the guest as it was.
pub fn read_guest_windows(context: &Context, machine: &Machine, slot: Option<Slot>) -> Result<Listing> {
    let json = match ask(context, machine, slot)? {
        Some(json) => json,
        None => {
            start(context, machine, slot)?;
            let answered = ask(context, machine, slot);
            stop_helper(context, machine, slot, WINDOWS_HELPER);
            answered?.ok_or_else(|| Failure(format!("{} did not answer for its windows.", machine.name)))?
        }
    };
    let windows = parse_windows(&json)?;
    Ok(Listing { json, windows })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::make_token;
    use crate::agent::stand_in::ScriptedAgent;
    use crate::testing::{Scratch, scratch_context};
    use crate::text::base64_encode;
    use std::sync::Arc;

    /// As the helper writes it, with one window cut from each kind that matters. The last is one a
    /// helper of another build left in, marked cloaked.
    const LIST: &str = r##"{"windows":[
{"handle":65814,"owner":0,"pid":4120,"exe":"notepad.exe","class":"Notepad","title":"Untitled - Notepad",
 "frame":{"left":100,"top":80,"right":986,"bottom":673},"rect":{"left":92,"top":80,"right":994,"bottom":681},
 "z":1,"minimized":false,"tool_window":false,"popup":false,"foreground":true},
{"handle":131100,"owner":65814,"pid":4120,"exe":"notepad.exe","class":"#32768","title":"",
 "frame":{"left":110,"top":120,"right":300,"bottom":400},"rect":{"left":110,"top":120,"right":300,"bottom":400},
 "z":0,"minimized":false,"tool_window":true,"popup":true,"foreground":false},
{"handle":197000,"owner":0,"pid":5200,"exe":"CalculatorApp.exe","class":"ApplicationFrameWindow","title":"Calculator",
 "frame":{"left":-32000,"top":-32000,"right":-31840,"bottom":-31972},"rect":{"left":-32000,"top":-32000,"right":-31840,"bottom":-31972},
 "z":2,"minimized":true,"tool_window":false,"popup":false,"foreground":false},
{"handle":262000,"owner":0,"pid":6001,"exe":"","class":"Chrome_WidgetWin_1",
 "title":"He said \"hi\" \\ ünï — 日本語 😀\ttab\nline",
 "frame":{"left":0,"top":0,"right":1920,"bottom":1040},"rect":{"left":0,"top":0,"right":1920,"bottom":1040},
 "z":3,"minimized":false,"tool_window":false,"popup":false,"foreground":false},
{"handle":300000,"owner":0,"pid":7000,"exe":"Weather.exe","class":"ApplicationFrameWindow","title":"Weather",
 "frame":{"left":0,"top":0,"right":800,"bottom":600},"rect":{"left":0,"top":0,"right":800,"bottom":600},
 "z":4,"minimized":false,"tool_window":false,"popup":false,"foreground":false,"cloaked":true}
]}"##;

    #[test]
    fn the_windows_are_read_with_their_popups_and_minimized_ones_and_without_the_cloaked() {
        let windows = parse_windows(LIST).unwrap();
        let handles: Vec<i64> = windows.iter().map(|window| window.handle).collect();
        assert_eq!(handles, [65814, 131100, 197000, 262000], "the cloaked Weather window is not shown");
        let notepad = &windows[0];
        assert!(notepad.foreground && !notepad.popup && !notepad.minimized);
        assert_eq!(
            (notepad.exe.as_str(), notepad.pid, notepad.class.as_str()),
            ("notepad.exe", 4120, "Notepad")
        );
        assert_eq!((notepad.frame.width(), notepad.frame.height()), (886, 593));
        assert_eq!(
            (notepad.rect.width(), notepad.rect.height()),
            (902, 601),
            "the border is in the rectangle"
        );
        let menu = &windows[1];
        assert!(menu.popup && menu.tool_window);
        assert_eq!((menu.owner, menu.z), (notepad.handle, 0), "a popup keeps its owner and is on top");
        assert!(windows[2].minimized);
    }

    #[test]
    fn a_title_with_quotes_and_other_scripts_comes_through_whole() {
        let windows = parse_windows(LIST).unwrap();
        assert_eq!(windows[3].title, "He said \"hi\" \\ ünï — 日本語 😀\ttab\nline");
        assert_eq!(windows[3].exe, "");
    }

    #[test]
    fn a_window_is_one_line_however_odd_its_title() {
        let lines: Vec<String> = parse_windows(LIST).unwrap().iter().map(describe).collect();
        assert_eq!(lines[0], "1 0x10116 notepad.exe \"Untitled - Notepad\" 886x593 at 100,80 [foreground]");
        assert_eq!(
            lines[1],
            "0 0x2001c notepad.exe \"\" 190x280 at 110,120 [tool window, popup, owner 0x10116]"
        );
        assert!(lines[2].ends_with("[minimized]"), "{}", lines[2]);
        assert!(lines[3].contains("(unknown program)"), "{}", lines[3]);
        assert!(lines[3].contains(r#""He said \"hi\" \\ ünï — 日本語 😀\ttab\nline""#), "{}", lines[3]);
        assert!(lines.iter().all(|line| !line.contains('\n')));
    }

    #[test]
    fn what_is_not_the_helpers_list_is_refused_in_words() {
        for text in ["", "ERROR: no", "{\"windows\":[{\"handle\":1}]}", "[]"] {
            let failure = parse_windows(text).unwrap_err();
            assert!(
                failure.0.starts_with("The guest's list of windows could not be read"),
                "{text:?}: {failure}"
            );
        }
        assert_eq!(parse_windows("{\"windows\":[]}"), Ok(Vec::new()));
    }

    fn rig(scratch: &Scratch, answers: &[&str]) -> (Context, Machine, Arc<ScriptedAgent>) {
        let mut context = scratch_context(scratch);
        scratch.write("program/guest/windows.ps1", "the helper");
        scratch.write("program/guest/windows-start.ps1", "start it");
        scratch.write("program/guest/windows-ask.ps1", "ask it");
        scratch.write("program/guest/helper-stop.ps1", "stop it");
        let agent = Arc::new(ScriptedAgent::answering(answers));
        context.agent = agent.clone();
        let machine = Machine::new("vm", "vm", "/d", "/v");
        machine.save(&context).unwrap();
        make_token(&context, &machine).unwrap();
        (context, machine, agent)
    }

    /// As the agent hands it back: the line ends are Windows', and long lines are cut.
    fn wrapped_answer() -> String {
        let encoded = base64_encode(LIST.as_bytes());
        let lines: Vec<&str> =
            encoded.as_bytes().chunks(220).map(|chunk| std::str::from_utf8(chunk).unwrap()).collect();
        format!("answer {}\r\n", lines.join("\r\n"))
    }

    #[test]
    fn a_running_helper_is_asked_and_left_running() {
        let scratch = Scratch::new();
        let (context, machine, agent) = rig(&scratch, &[&wrapped_answer()]);
        let listing = read_guest_windows(&context, &machine, None).unwrap();
        assert_eq!(listing.json, LIST, "the helper's words as they were");
        assert_eq!(listing.windows.len(), 4);
        let requests = agent.requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "no start, no stop");
        assert_eq!(requests[0].2, "$request = 'list'\nask it");
    }

    #[test]
    fn a_helper_that_is_not_running_is_started_for_the_question_and_ended() {
        let scratch = Scratch::new();
        let (context, machine, agent) = rig(
            &scratch,
            &["Exception calling \".ctor\" with \"2\" argument(s)\r\n", "started\r\n", &wrapped_answer(), ""],
        );
        assert_eq!(read_guest_windows(&context, &machine, None).unwrap().windows.len(), 4);
        let requests = agent.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        let start = requests[1].2.strip_prefix("$helperName = 'windows'\nstop it\n$helper = '").unwrap();
        let (helper, rest) = start.split_once('\'').unwrap();
        assert_eq!(base64_decode(helper).unwrap(), b"the helper");
        assert_eq!(rest, "\nstart it");
        assert!(requests[2].2.starts_with("$request = 'list'"));
        assert_eq!(requests[3].2, "$helperName = 'windows'\nstop it");
    }

    #[test]
    fn a_helper_that_does_not_start_or_does_not_answer_is_told_in_words() {
        let scratch = Scratch::new();
        let (context, machine, agent) = rig(&scratch, &["", "not listening\r\n"]);
        let failure = read_guest_windows(&context, &machine, None).unwrap_err();
        assert_eq!(failure.0, "The helper that lists vm's windows did not start.");
        assert_eq!(agent.requests.lock().unwrap().len(), 2, "a helper that never started is not stopped");

        let (context, machine, _) = rig(&scratch, &["", "nobody is logged in\r\n"]);
        assert_eq!(
            read_guest_windows(&context, &machine, None).unwrap_err().0,
            "Nobody is logged in to vm, so it has no windows."
        );

        let (context, machine, agent) = rig(&scratch, &["", "started\r\n", "answer \r\n", ""]);
        assert_eq!(
            read_guest_windows(&context, &machine, None).unwrap_err().0,
            "vm did not answer for its windows."
        );
        assert_eq!(agent.requests.lock().unwrap().len(), 4, "it is stopped all the same");
    }

    #[test]
    fn an_answer_cut_short_is_not_taken_for_a_list() {
        let scratch = Scratch::new();
        let (context, machine, _) = rig(&scratch, &["answer a*b\r\n"]);
        assert_eq!(
            read_guest_windows(&context, &machine, None).unwrap_err().0,
            "The guest's list of windows was cut short."
        );
    }
}
