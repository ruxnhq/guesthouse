// SPDX-License-Identifier: GPL-2.0-or-later
//! `guesthouse-pvg-host`, one process per VM on a macOS host that owns the guest's graphics
//! device: the library's notes say what it does. This is its command line, and nothing else:
//! `--option-rom` or `--screen-scale` as the first argument says one thing and ends; otherwise the
//! guest is served, and a mistake or a failure is one `pvg-host: ` line and exit status 1.

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--option-rom") {
        return match guesthouse_pvg_sys::option_rom() {
            Some(path) => {
                println!("{}", path.display());
                std::process::ExitCode::SUCCESS
            }
            None => {
                eprintln!("pvg-host: the framework names no option ROM");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if std::env::args().nth(1).as_deref() == Some("--screen-scale") {
        println!("{}", guesthouse_pvg_host::screen_scale());
        return std::process::ExitCode::SUCCESS;
    }
    let outcome = guesthouse_pvg_host::options(std::env::args().skip(1))
        .and_then(|(options, taking)| guesthouse_pvg_host::serve(options, taking));
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("pvg-host: {why}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("pvg-host: this is the graphics process of a macOS host, and runs on a Mac only");
    std::process::ExitCode::FAILURE
}
