use guesthouse_app_core::actions::run_task;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::vm_host::LocalHost;
use std::env;
use std::process::ExitCode;

pub fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
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
