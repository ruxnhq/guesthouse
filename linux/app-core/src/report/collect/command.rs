//! Runs a system program for its output, with a time limit, so a hung tool can't hold up a report.

use std::process::{Command, Stdio};

/// What a program wrote, or why there's nothing: a sentence for the report's manifest.
pub fn output_of(program: &str, arguments: &[&str], seconds: u32) -> Result<String, String> {
    let seconds = seconds.to_string();
    let result = Command::new("timeout")
        .arg(&seconds)
        .arg(program)
        .args(arguments)
        // coredumpctl and journalctl would page their output, and a debugger may look for symbols online.
        .env("SYSTEMD_PAGER", "")
        .env("DEBUGINFOD_URLS", "")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run timeout: {error}"))?;
    let said = String::from_utf8_lossy(&result.stdout).into_owned();
    match result.status.code() {
        Some(0) => Ok(said),
        // What timeout says when it can't find the program, and when it ran out of time.
        Some(127) => Err(format!("{program} isn't installed")),
        Some(124) => Err(format!("{program} gave no answer in {seconds} seconds")),
        _ => {
            let complaint = String::from_utf8_lossy(&result.stderr);
            let complaint =
                complaint.lines().find(|line| !line.trim().is_empty()).unwrap_or("no reason given");
            Err(format!("{program} failed: {}", complaint.trim()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_says_so() {
        assert_eq!(
            output_of("guesthouse-no-such-program", &[], 5),
            Err("guesthouse-no-such-program isn't installed".into())
        );
    }

    #[test]
    fn output_comes_back() {
        assert_eq!(output_of("echo", &["hello"], 5).as_deref(), Ok("hello\n"));
    }

    #[test]
    fn a_failure_gives_its_first_line() {
        let said = output_of("sh", &["-c", "echo 'not allowed' >&2; exit 1"], 5);
        assert_eq!(said, Err("sh failed: not allowed".into()));
    }
}
