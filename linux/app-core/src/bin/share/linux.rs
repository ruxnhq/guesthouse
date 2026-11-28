use guesthouse_app_core::share::{Access, MOST_QUIET, Share, limit_quiet};
use std::env;
use std::io::{self, BufReader};
use std::process::ExitCode;

pub fn main() -> ExitCode {
    let Some(folder) = env::var_os("SHARE") else {
        eprintln!("SHARE must name the folder to share");
        return ExitCode::from(2);
    };
    let key = env::var("SHARE_KEY").unwrap_or_default();
    let Some(access) = env::var("SHARE_ACCESS").ok().as_deref().and_then(Access::from_word) else {
        eprintln!("SHARE_ACCESS must be read-write or read-only");
        return ExitCode::from(2);
    };
    if let Err(error) = limit_quiet(io::stdin(), MOST_QUIET) {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }
    let reader = BufReader::new(io::stdin().lock());
    match Share::new(folder.as_ref(), &key, access)
        .and_then(|mut share| share.serve(reader, io::stdout().lock()))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
