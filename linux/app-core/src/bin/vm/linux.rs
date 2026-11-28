use guesthouse_app_core::context::Context;
use guesthouse_app_core::headless::{Problem, run};
use std::env;
use std::io;
use std::process::ExitCode;

pub fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let context = match Context::from_process() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    context.keep_private();
    match run(&context, &arguments, &mut io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem @ Problem::Usage(_)) => {
            eprintln!("{problem}");
            ExitCode::from(2)
        }
        Err(problem) => {
            eprintln!("{problem}");
            ExitCode::FAILURE
        }
    }
}
