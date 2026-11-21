use guesthouse_jail::launch::Launch;
use std::env;
use std::process::ExitCode;

pub fn main() -> ExitCode {
    let error = match Launch::parse(env::args_os().skip(1)) {
        Ok(launch) => launch.run().to_string(),
        Err(error) => error,
    };
    eprintln!("guesthouse-jail: {error}");
    ExitCode::FAILURE
}
