//! `guesthouse-service`: the Guesthouse service on a Windows host. The service manager starts it
//! with the word `service`; `guesthouse-service run` serves the pipe from an elevated prompt
//! instead, until Ctrl+C, to try it without installing it.

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    guesthouse_service::windows::main(std::env::args().skip(1).collect())
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-service runs on a Windows host only.");
    std::process::ExitCode::FAILURE
}
