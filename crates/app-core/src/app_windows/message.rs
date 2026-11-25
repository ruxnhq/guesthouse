//! The lines the guest's app-windows helper and the host send each other: one JSON object a line,
//! each with a `t` that names its type. Which fields there are, and what they mean, is the contract in
//! the design notes. Reading is strict about every field the contract names, as a line that lacks one
//! would be acted on with a guess, and tolerant of fields it does not name, so a later helper can add
//! some without this build refusing its lines.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;

/// The version `hello` must carry. Another one is another build's helper.
pub const PROTOCOL_VERSION: u64 = 1;
/// No line is longer than this, except an answer to `apps`, which carries icons.
pub const MOST_LINE_BYTES: usize = 1 << 20;
pub const MOST_APPS_LINE_BYTES: usize = 16 << 20;

/// A rectangle on the guest's screen, in the guest's pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Shown,
    Minimized,
    /// Hidden under an overflow cell: there is nothing to show.
    Covered,
}

/// The strip a drag moves the host window by: the top `h` rows, less `buttons_w` at the right. An `h`
/// of 0 means the helper does not know where the title bar is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caption {
    pub h: i32,
    pub buttons_w: i32,
}

/// One window or popup of the guest, as the helper lays it out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    /// The guest's window handle as a decimal string.
    pub id: String,
    pub pid: u32,
    pub exe: String,
    pub cls: String,
    pub title: String,
    /// Where the picture is on the guest's screen, which is what the host crops.
    pub cell: Rect,
    /// Where the real window's visible frame is, which is where input must go.
    pub frame: Rect,
    pub popup: bool,
    /// For a popup, the cell it belongs to. Empty for a window.
    pub parent: String,
    /// For a popup, its frame's origin less its parent's frame's origin.
    pub rel: Point,
    pub state: State,
    pub overflow: bool,
    pub focus: bool,
    pub caption: Caption,
    pub resizable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// Different for each start of the helper, so a restart can be told from a reconnect.
    pub session: String,
    pub screen: Size,
    /// Windows' percentage for its one monitor.
    pub scale: u32,
}

/// The whole state each time, never a difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub seq: u64,
    pub screen: Size,
    pub cells: Vec<Cell>,
}

/// Windows' screen as the helper sees it now, whole each time: sent after the first layout and again
/// whenever any of it changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    /// Windows' percentage for its one monitor, as it is now (not as it was at sign-in).
    pub scale: u32,
    /// The percentages Windows offers for the monitor at its resolution now, from the smallest.
    #[serde(default)]
    pub steps: Vec<u32>,
    /// The smallest screen that holds every window's cell with none overflowing, the taskbar
    /// included. `None` when the helper cannot say.
    #[serde(default)]
    pub need: Option<Size>,
}

/// A program from the Start menu.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct App {
    /// Stable, so a launcher entry can name it.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub target: String,
    /// A PNG of 48 pixels, in Base64. Empty when the request did not ask for icons.
    #[serde(default)]
    pub icon: String,
    /// An uninstaller or a help file: marked, not left out.
    #[serde(default)]
    pub minor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The `n` of the request it answers.
    pub re: u64,
    pub ok: bool,
    /// In words, when `ok` is false. Empty otherwise.
    pub error: String,
    /// Only an answer to `apps` has them.
    pub apps: Option<Vec<App>>,
}

/// Which desktop takes the input. While it is not the normal one, the host shows the whole frame in
/// one window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Desktop {
    Normal,
    /// UAC's.
    Secure,
    Locked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestMessage {
    Hello(Hello),
    Layout(Layout),
    Answer(Answer),
    Desktop(Desktop),
    Display(Display),
    /// A type this build does not know, which a later helper may send.
    Other(String),
}

/// Why a line was not taken, in words a log can hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineProblem {
    TooLong {
        length: usize,
        most: usize,
    },
    NotJson(String),
    NoType,
    /// A message of this type that lacks a field, or has one of the wrong kind.
    Malformed {
        kind: String,
        why: String,
    },
    /// The helper speaks another version of the contract.
    Version(u64),
}

impl fmt::Display for LineProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { length, most } => write!(f, "a line of {length} bytes, where {most} is the most"),
            Self::NotJson(why) => write!(f, "a line that is not JSON: {why}"),
            Self::NoType => f.write_str("a line with no type"),
            Self::Malformed { kind, why } => write!(f, "a {kind} message that cannot be read: {why}"),
            Self::Version(found) => {
                write!(f, "version {found} of the messages, where {PROTOCOL_VERSION} is read")
            }
        }
    }
}

impl std::error::Error for LineProblem {}

#[derive(Deserialize)]
struct RawHello {
    v: u64,
    session: String,
    screen: Size,
    scale: u32,
}

#[derive(Deserialize)]
struct RawLayout {
    seq: u64,
    screen: Size,
    cells: Vec<Cell>,
}

#[derive(Deserialize)]
struct RawAnswer {
    re: u64,
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    apps: Option<Vec<App>>,
}

#[derive(Deserialize)]
struct RawDesktop {
    kind: Desktop,
}

fn read<T: serde::de::DeserializeOwned>(kind: &str, value: Value) -> Result<T, LineProblem> {
    serde_json::from_value(value)
        .map_err(|error| LineProblem::Malformed { kind: kind.to_owned(), why: error.to_string() })
}

/// Reads one line from the helper, without its line end. `most` is how long a line may be: the
/// reader knows whether an answer to `apps` is what it waits for.
pub fn parse_guest_line(line: &str, most: usize) -> Result<GuestMessage, LineProblem> {
    if line.len() > most {
        return Err(LineProblem::TooLong { length: line.len(), most });
    }
    let value: Value = serde_json::from_str(line).map_err(|error| LineProblem::NotJson(error.to_string()))?;
    let kind = value.get("t").and_then(Value::as_str).ok_or(LineProblem::NoType)?.to_owned();
    Ok(match kind.as_str() {
        "hello" => {
            let hello: RawHello = read(&kind, value)?;
            if hello.v != PROTOCOL_VERSION {
                return Err(LineProblem::Version(hello.v));
            }
            GuestMessage::Hello(Hello { session: hello.session, screen: hello.screen, scale: hello.scale })
        }
        "layout" => {
            let layout: RawLayout = read(&kind, value)?;
            GuestMessage::Layout(Layout { seq: layout.seq, screen: layout.screen, cells: layout.cells })
        }
        "answer" => {
            let answer: RawAnswer = read(&kind, value)?;
            GuestMessage::Answer(Answer {
                re: answer.re,
                ok: answer.ok,
                error: answer.error,
                apps: answer.apps,
            })
        }
        "desktop" => GuestMessage::Desktop(read::<RawDesktop>(&kind, value)?.kind),
        "display" => GuestMessage::Display(read(&kind, value)?),
        _ => GuestMessage::Other(kind),
    })
}

/// What a launch names: a program of the Start menu, or a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    App(String),
    Path { path: String, args: String },
}

/// What the host asks of the helper. Each goes with a number the answer repeats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// The host window got the focus: make the guest's the foreground window.
    Focus {
        id: String,
    },
    Minimize {
        id: String,
    },
    /// The helper repeats it until Windows has taken it, then answers.
    Restore {
        id: String,
    },
    /// As the title bar's close button does. A program that asks to save keeps its cell.
    Close {
        id: String,
    },
    /// The frame's size wanted. The next layout says what came of it.
    Resize {
        id: String,
        w: i32,
        h: i32,
    },
    Launch(Launch),
    Apps {
        icons: bool,
    },
    /// Send the layout again now.
    Layout,
    /// Set Windows' scale for its monitor to this percentage, one of the display's `steps`. The
    /// answer says whether Windows took it, and a `display` message follows with the scale it is at.
    Scale {
        percent: u32,
    },
}

impl Request {
    /// The contract's name for it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Focus { .. } => "focus",
            Self::Minimize { .. } => "minimize",
            Self::Restore { .. } => "restore",
            Self::Close { .. } => "close",
            Self::Resize { .. } => "resize",
            Self::Launch(_) => "launch",
            Self::Apps { .. } => "apps",
            Self::Layout => "layout",
            Self::Scale { .. } => "scale",
        }
    }

    /// Whether the answer may be a line of up to [`MOST_APPS_LINE_BYTES`].
    pub fn answer_is_long(&self) -> bool {
        matches!(self, Self::Apps { .. })
    }

    /// The line to send, without its line end.
    pub fn line(&self, n: u64) -> String {
        let t = self.kind();
        let value = match self {
            Self::Focus { id } | Self::Minimize { id } | Self::Restore { id } | Self::Close { id } => {
                json!({"t": t, "n": n, "id": id})
            }
            Self::Resize { id, w, h } => json!({"t": t, "n": n, "id": id, "w": w, "h": h}),
            Self::Launch(Launch::App(app)) => json!({"t": t, "n": n, "app": app}),
            Self::Launch(Launch::Path { path, args }) => json!({"t": t, "n": n, "path": path, "args": args}),
            Self::Apps { icons } => json!({"t": t, "n": n, "icons": icons}),
            Self::Layout => json!({"t": t, "n": n}),
            Self::Scale { percent } => json!({"t": t, "n": n, "percent": percent}),
        };
        value.to_string()
    }
}

/// The lines the host sends before any request: the token, which the helper checks.
pub fn token_line(token: &str) -> String {
    format!("{token}\n")
}

/// The first line of a side connection: the token, then a space and the word `side`. A side connection
/// is for a short look or a short job (`layout`, `apps` and `launch`, and the answers to them): the
/// helper does not take it for the host's one driving connection, so it neither replaces that one nor
/// is it sent the layout when that changes. A helper of this version before the word was added
/// refuses the line as a wrong token.
pub fn side_token_line(token: &str) -> String {
    format!("{token} {SIDE_WORD}\n")
}

pub const SIDE_WORD: &str = "side";

/// The lines a helper writes, for the fake one and for a test that wants a message as it is sent.
pub fn hello_line(session: &str, screen: Size, scale: u32) -> String {
    json!({"t": "hello", "v": PROTOCOL_VERSION, "session": session, "screen": screen, "scale": scale})
        .to_string()
}

pub fn layout_line(layout: &Layout) -> String {
    json!({"t": "layout", "seq": layout.seq, "screen": layout.screen, "cells": layout.cells}).to_string()
}

pub fn answer_line(re: u64, ok: bool, error: &str) -> String {
    if ok {
        json!({"t": "answer", "re": re, "ok": true}).to_string()
    } else {
        json!({"t": "answer", "re": re, "ok": false, "error": error}).to_string()
    }
}

pub fn display_line(display: &Display) -> String {
    let mut value = json!({"t": "display", "scale": display.scale, "steps": display.steps});
    if let Some(need) = display.need {
        value["need"] = json!(need);
    }
    value.to_string()
}

pub fn desktop_line(kind: &str) -> String {
    json!({"t": "desktop", "kind": kind}).to_string()
}
