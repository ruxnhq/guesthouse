use guesthouse_jail::launch::{Guard, Launch};
use std::env;
use std::ffi::OsString;
use std::process::ExitCode;

pub fn main() -> ExitCode {
    let words: Vec<OsString> = env::args_os().skip(1).collect();
    if let Some(guard) = Guard::parse(&words) {
        return match guard.and_then(|guard| guard.run().map_err(|error| error.to_string())) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("guesthouse-jail: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let error = match Launch::parse(words) {
        Ok(launch) => launch.run().to_string(),
        Err(error) => error,
    };
    eprintln!("guesthouse-jail: {error}");
    ExitCode::FAILURE
}
