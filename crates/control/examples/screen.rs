//! Opens a Windows guest's screen, as the app does: asks the service for what the viewer needs,
//! writes the viewer's identity into a state folder of the user's, and starts remoteman's viewer
//! with its `connect` command.
//!
//!     cargo run -p guesthouse-control --example screen -- NAME PATH\TO\remoteman.exe [VIEWER OPTIONS]
//!
//! Options after the viewer's program go to it, such as `--size 2560x1440`. The viewer finds
//! FFmpeg's DLLs in its own folder, as remoteman's release lays it out.

use guesthouse_control::client::ask;
use guesthouse_control::screen::connect_command;
use guesthouse_control::{Answer, Request};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [name, viewer, options @ ..] = args.as_slice() else {
        eprintln!("Give the machine's name and the remoteman viewer's program.");
        return ExitCode::FAILURE;
    };
    let answer = match ask(&Request::Screen { name: name.clone() }) {
        Ok(answer) => answer,
        Err(unreached) => {
            eprintln!("{}", unreached.words());
            return ExitCode::FAILURE;
        }
    };
    let Answer::Screen { address, port, host_fingerprint, viewer_certificate, viewer_key } = answer else {
        eprintln!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return ExitCode::FAILURE;
    };
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let state = base.join("Guesthouse").join("screen").join(name);
    let command =
        match connect_command(&state, &address, port, &host_fingerprint, &viewer_certificate, &viewer_key) {
            Ok(command) => command,
            Err(error) => {
                eprintln!("{}: {error}", state.display());
                return ExitCode::FAILURE;
            }
        };
    println!("Opening {address}:{port}, expecting {host_fingerprint}, with {}", state.display());
    let mut viewer_command = Command::new(viewer);
    viewer_command.args(command).args(options);
    if let Some(folder) = Path::new(viewer).parent().filter(|folder| !folder.as_os_str().is_empty()) {
        viewer_command.current_dir(folder);
    }
    match viewer_command.spawn() {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{viewer}: {error}");
            ExitCode::FAILURE
        }
    }
}
