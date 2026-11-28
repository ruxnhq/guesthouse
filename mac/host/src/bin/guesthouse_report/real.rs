// SPDX-License-Identifier: GPL-2.0-or-later
//! `guesthouse-report`: gathers a bug report about one machine and this Mac into one file, shows
//! what is in it, and sends it to the developer if the person types yes. A program of its own, so it
//! works when the app will not open.

pub fn main() -> std::process::ExitCode {
    guesthouse_core::report::cli::main()
}
