// SPDX-License-Identifier: GPL-2.0-or-later
//! The size a machine's screen should be, as the app works it out from the machine's window.
//!
//! This is the app's half of "the screen follows the window". When a resize of the window ends
//! the app says how many points the window is, how many pixels the screen should have and the
//! scaling to set (200 for Sharp, 100 for Standard). The last answer for each machine is
//! recorded. For a running machine on the guarded path it is also sent to the graphics process
//! as its one display mode (`modes WxH@60` on the control socket), which offers it to the guest;
//! a guest whose driver follows the device's modes (our Linux driver does) takes it. At most one
//! a second for each machine, the last one winning. QEMU's own device takes no size from the
//! app. The scaling is not sent: a Linux guest is given its scale when it starts
//! (`launch::SCALE_FILE`).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// What the app asks the machine's screen to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    pub width_px: u32,
    pub height_px: u32,
    pub scaling_percent: u32,
}

/// The scalings that mean something to a guest.
pub const SCALINGS: [u32; 2] = [100, 200];

fn recorded() -> &'static Mutex<HashMap<String, Wanted>> {
    static RECORDED: OnceLock<Mutex<HashMap<String, Wanted>>> = OnceLock::new();
    RECORDED.get_or_init(Default::default)
}

/// Records what machine `id`'s screen should be, if it is something a screen can be.
pub fn record(id: &str, wanted: Wanted) -> Result<(), String> {
    if wanted.width_px == 0 || wanted.height_px == 0 {
        return Err("A screen needs a width and a height.".into());
    }
    if !SCALINGS.contains(&wanted.scaling_percent) {
        return Err(format!("{}% is not a scaling a guest takes (100 or 200).", wanted.scaling_percent));
    }
    recorded().lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), wanted);
    Ok(())
}

/// The last size recorded for machine `id`.
pub fn last(id: &str) -> Option<Wanted> {
    recorded().lock().unwrap_or_else(|e| e.into_inner()).get(id).copied()
}

/// The least time between two sizes sent to one machine.
pub const PACE: Duration = Duration::from_secs(1);

/// The largest screen sent to a graphics process: the largest its own window offers a guest.
pub const MOST_PIXELS: (u32, u32) = (5120, 2880);

/// How long the graphics process gets to answer `modes`: it answers once the device has the
/// new list, which is quick, but it serves its guest first.
const ANSWER_WAIT: Duration = Duration::from_secs(5);

/// What became of a size sent to a machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Told {
    /// The graphics process said `ok` to this mode.
    Applied(String),
    /// A newer size came before this one's turn, and is sent in its place.
    Overtaken,
    /// Not sent, and why.
    NotSent(String),
}

/// The display mode sent for `wanted`: the width down to a multiple of 8, as the graphics
/// process's own window offers, and no larger than `MOST_PIXELS`.
pub fn mode_for(wanted: Wanted) -> String {
    let width = wanted.width_px.min(MOST_PIXELS.0) & !7;
    let height = wanted.height_px.min(MOST_PIXELS.1);
    format!("{width}x{height}@60")
}

/// For each machine: when a size was last sent, and the number of the newest one asked for.
#[derive(Default)]
struct Pace {
    sent: Option<Instant>,
    newest: u64,
}

fn paces() -> &'static Mutex<HashMap<String, Pace>> {
    static PACES: OnceLock<Mutex<HashMap<String, Pace>>> = OnceLock::new();
    PACES.get_or_init(Default::default)
}

/// Sends `wanted` to machine `id`'s graphics process on its `control` socket, once its turn
/// comes: a second after the last one sent, unless a newer size has come by then. Waits for
/// that and for the answer, so the app calls it off its main thread.
pub fn send(id: &str, control: &Path, wanted: Wanted) -> Result<Told, String> {
    let ticket = {
        let mut paces = paces().lock().unwrap_or_else(|e| e.into_inner());
        let pace = paces.entry(id.to_string()).or_default();
        pace.newest += 1;
        pace.newest
    };
    loop {
        let wait = {
            let mut paces = paces().lock().unwrap_or_else(|e| e.into_inner());
            let pace = paces.entry(id.to_string()).or_default();
            if pace.newest != ticket {
                return Ok(Told::Overtaken);
            }
            let now = Instant::now();
            let wait = pace.sent.map_or(Duration::ZERO, |sent| (sent + PACE).saturating_duration_since(now));
            if wait.is_zero() {
                pace.sent = Some(now);
            }
            wait
        };
        if wait.is_zero() {
            break;
        }
        thread::sleep(wait);
    }
    let mode = mode_for(wanted);
    let Ok(stream) = UnixStream::connect(control) else {
        return Ok(Told::NotSent("the machine's graphics process is not running".into()));
    };
    let answer = ask(&stream, &format!("modes {mode}"))
        .map_err(|error| format!("the graphics process did not answer: {error}"))?;
    if answer == "ok" || answer.starts_with("ok ") {
        Ok(Told::Applied(mode))
    } else {
        Err(format!("the graphics process said: {answer}"))
    }
}

/// One line to the graphics process and its one line back.
fn ask(mut stream: &UnixStream, line: &str) -> std::io::Result<String> {
    stream.set_read_timeout(Some(ANSWER_WAIT))?;
    stream.set_write_timeout(Some(ANSWER_WAIT))?;
    writeln!(stream, "{line}")?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    Ok(answer.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_size_asked_for_is_the_one_kept() {
        let first = Wanted { width_px: 2360, height_px: 1392, scaling_percent: 200 };
        let second = Wanted { width_px: 1180, height_px: 696, scaling_percent: 100 };
        assert_eq!(last("screen-size-test"), None);
        record("screen-size-test", first).unwrap();
        record("screen-size-test", second).unwrap();
        assert_eq!(last("screen-size-test"), Some(second));
    }

    #[test]
    fn the_mode_sent_is_a_multiple_of_8_wide_and_no_larger_than_the_window_offers() {
        let wanted = |width_px, height_px| Wanted { width_px, height_px, scaling_percent: 200 };
        assert_eq!(mode_for(wanted(2560, 1440)), "2560x1440@60");
        assert_eq!(mode_for(wanted(2362, 1392)), "2360x1392@60");
        assert_eq!(mode_for(wanted(7680, 4320)), "5120x2880@60");
    }

    /// A stand-in graphics process on a socket of its own: answers every line it is given with
    /// `answer` and writes the line down.
    fn graphics(name: &str, answer: &'static str) -> (std::path::PathBuf, std::sync::mpsc::Receiver<String>) {
        let path = std::env::temp_dir().join(format!("gh-size-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let (heard, lines) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let _ = heard.send(line.trim().to_string());
                writeln!(&stream, "{answer}").unwrap();
            }
        });
        (path, lines)
    }

    fn sized(width_px: u32) -> Wanted {
        Wanted { width_px, height_px: 800, scaling_percent: 100 }
    }

    #[test]
    fn a_size_is_applied_only_when_the_graphics_process_says_ok() {
        let (good, heard) = graphics("ok", "ok 1 modes");
        assert_eq!(send("size-ok", &good, sized(1280)), Ok(Told::Applied("1280x800@60".into())));
        assert_eq!(heard.recv().unwrap(), "modes 1280x800@60");
        let (bad, _) = graphics("bad", "error the device refused the modes");
        assert!(send("size-bad", &bad, sized(1280)).unwrap_err().contains("refused"));
        let nobody = std::env::temp_dir().join("gh-size-nobody");
        assert!(matches!(send("size-none", &nobody, sized(1280)), Ok(Told::NotSent(_))));
        let _ = (std::fs::remove_file(good), std::fs::remove_file(bad));
    }

    #[test]
    fn at_most_one_size_a_second_goes_and_the_last_one_wins() {
        let (path, heard) = graphics("pace", "ok 1 modes");
        let started = Instant::now();
        assert!(matches!(send("size-pace", &path, sized(1024)), Ok(Told::Applied(_))));
        // Three more at once: only the last is sent, a second after the first.
        let later: Vec<_> = [1280u32, 1440, 1600]
            .into_iter()
            .map(|width| {
                let path = path.clone();
                let told = thread::spawn(move || send("size-pace", &path, sized(width)));
                thread::sleep(Duration::from_millis(50));
                told
            })
            .collect();
        let told: Vec<_> = later.into_iter().map(|t| t.join().unwrap().unwrap()).collect();
        assert_eq!(told, [Told::Overtaken, Told::Overtaken, Told::Applied("1600x800@60".into())]);
        assert!(started.elapsed() >= PACE);
        let lines: Vec<String> = heard.try_iter().collect();
        assert_eq!(lines, ["modes 1024x800@60", "modes 1600x800@60"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn nothing_that_is_not_a_screen_is_kept() {
        let empty = Wanted { width_px: 0, height_px: 10, scaling_percent: 100 };
        assert!(record("screen-size-bad", empty).is_err());
        let odd = Wanted { width_px: 10, height_px: 10, scaling_percent: 150 };
        assert!(record("screen-size-bad", odd).unwrap_err().contains("150%"));
        assert_eq!(last("screen-size-bad"), None);
    }
}
