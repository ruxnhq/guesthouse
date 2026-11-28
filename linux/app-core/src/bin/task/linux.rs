use guesthouse_app_core::actions::run_task;
use guesthouse_app_core::capped_log;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::vm_host::LocalHost;
use std::env;
use std::process::ExitCode;

pub fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if let [word, file, most] = &arguments[..]
        && word == capped_log::KEEP_LOG
    {
        return keep_log(file, most);
    }
    let [action, machine, rest @ ..] = &arguments[..] else {
        eprintln!("usage: guesthouse-task ACTION MACHINE [ARGUMENT...]");
        return ExitCode::from(2);
    };
    let context = match Context::from_process() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    context.keep_private();
    match run_task(&context, &LocalHost::new(&context), action, machine, rest) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// QEMU's output or a guest's serial port, from standard input into the file, held to its size.
fn keep_log(file: &str, most: &str) -> ExitCode {
    let Ok(most) = most.parse::<u64>() else {
        eprintln!("usage: guesthouse-task keep-log FILE MOST_BYTES");
        return ExitCode::from(2);
    };
    match capped_log::keep(std::io::stdin().lock(), std::path::Path::new(file), most) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{file}: {error}");
            ExitCode::FAILURE
        }
    }
}
