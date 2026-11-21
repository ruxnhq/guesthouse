//! Opens a Windows guest's screen, as the app will: asks the service for what the viewer needs,
//! writes it into a state folder of the user's, and starts remoteman's viewer with it.
//!
//!     cargo run -p guesthouse-control --example screen -- NAME PATH\TO\remoteman-app.exe [VIEWER OPTIONS]
//!
//! Options after the viewer's program go to it, such as `--size 2560x1440`. The viewer needs
//! FFmpeg's DLLs beside it or on PATH.

use guesthouse_control::client::ask;
use guesthouse_control::screen::viewer_state;
use guesthouse_control::{Answer, Request};
use std::path::PathBuf;
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
    let target =
        match viewer_state(&state, &address, port, &host_fingerprint, &viewer_certificate, &viewer_key) {
            Ok(target) => target,
            Err(error) => {
                eprintln!("{}: {error}", state.display());
                return ExitCode::FAILURE;
            }
        };
    println!("Opening {target}, expecting {host_fingerprint}, with {}", state.display());
    match Command::new(viewer).arg(&target).arg("--state").arg(&state).args(options).spawn() {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{viewer}: {error}");
            ExitCode::FAILURE
        }
    }
}
