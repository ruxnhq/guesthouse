//! `guesthouse --app NAME PROGRAM [--start-menu]`: one step from a launcher entry or a terminal to a
//! program's window. It starts the machine if it is off, waits for the helper in the guest, starts the
//! program, and makes sure one viewer (`guesthouse --app-windows`) is running for the machine. It needs
//! no display itself, so what it has to say goes to the terminal and to the app's log, as a launcher
//! entry has no terminal to show it in.
//!
//! The viewer is another process of this program, started detached so that it outlives this one. One
//! viewer per machine holds because the viewer takes a lock of the machine's (see
//! `guesthouse_app_core::app_windows::viewer_lock`) and one that finds it taken ends at once, so that
//! two launches that race for it leave exactly one.

use gtk::glib;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::failure::Failure;
use guesthouse_app_core::headless::{Opening, ViewerProcess, open_program};
use guesthouse_app_core::log::{log_panics, write_log};
use guesthouse_app_core::machine::Machine;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

pub const ARGUMENT: &str = "--app";
const START_MENU: &str = "--start-menu";
const USAGE: &str = "Usage: guesthouse --app NAME PROGRAM [--start-menu]

Opens PROGRAM of the Windows machine NAME as a window of its own on this desktop. The machine is
started if it is off. PROGRAM is a path or name in the guest, such as C:\\Windows\\notepad.exe, or with
--start-menu the id of a Start menu program (guesthouse-vm apps NAME lists them). App windows has to
be on for the machine (guesthouse-vm set NAME app-windows on).";

#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub machine: String,
    pub program: String,
    pub start_menu: bool,
}

/// The request in these arguments, the program's name left out. `None` if they don't ask for this at
/// all, and the usage if they ask for it wrongly.
pub fn request(arguments: &[String]) -> Option<Result<Request, String>> {
    let (first, rest) = arguments.split_first()?;
    if first != ARGUMENT {
        return None;
    }
    let start_menu = rest.iter().any(|word| word == START_MENU);
    let words: Vec<&String> = rest.iter().filter(|word| *word != START_MENU).collect();
    Some(match words[..] {
        [machine, program] if !machine.starts_with("--") && !program.starts_with("--") => {
            Ok(Request { machine: machine.clone(), program: program.clone(), start_menu })
        }
        _ => Err(USAGE.to_owned()),
    })
}

/// A viewer this process started.
struct Started(Child);

impl ViewerProcess for Started {
    fn ended(&mut self) -> Option<String> {
        match self.0.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => None,
            Err(error) => Some(error.to_string()),
        }
    }
}

/// Starts this program again as the machine's viewer, in a process group of its own and with no
/// terminal, so that it goes on when this one ends and when the terminal closes.
pub fn start_viewer(machine: &Machine) -> Result<Box<dyn ViewerProcess>, Failure> {
    let program = std::env::current_exe()?;
    let child = Command::new(program)
        .arg(super::ARGUMENT)
        .arg(&machine.id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| Failure(format!("The app windows viewer could not be started: {error}.")))?;
    Ok(Box::new(Started(child)))
}

pub fn run(request: Request) -> glib::ExitCode {
    let context = match Context::from_process() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("{error}");
            return glib::ExitCode::FAILURE;
        }
    };
    log_panics(&context);
    let opening =
        Opening { machine: &request.machine, program: &request.program, start_menu: request.start_menu };
    let mut out = std::io::stdout();
    match open_program(&context, &opening, &mut start_viewer, &mut out) {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("{failure}");
            let _ = write_log(&context, &format!("guesthouse --app {}: {failure}", request.machine));
            glib::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn only_its_own_argument_asks_for_an_app() {
        assert_eq!(request(&arguments("--app-windows Win11")), None);
        assert_eq!(request(&[]), None);
        assert_eq!(
            request(&arguments("--app Win11 notepad.exe")),
            Some(Ok(Request { machine: "Win11".into(), program: "notepad.exe".into(), start_menu: false }))
        );
        assert_eq!(
            request(&arguments("--app win11 --start-menu Accessories/Paint")),
            Some(Ok(Request {
                machine: "win11".into(),
                program: "Accessories/Paint".into(),
                start_menu: true
            }))
        );
    }

    #[test]
    fn a_request_that_is_wrong_says_so() {
        for wrong in ["--app", "--app a", "--app a b c", "--app a --other"] {
            assert!(
                matches!(request(&arguments(wrong)), Some(Err(text)) if text.starts_with("Usage: guesthouse --app"))
            );
        }
    }
}
