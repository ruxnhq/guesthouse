use guesthouse_app_core::context::Context;
use guesthouse_app_core::firmware::format_of;
use guesthouse_app_core::linux::amd::amd_cards;
use guesthouse_app_core::setup::checks::{Check, CheckState, check_host};
use guesthouse_app_core::setup::intel::supported_intel_cards;
use guesthouse_app_core::setup::toolkit::gtk_version;
use std::env;
use std::process::ExitCode;

const FIRMWARE_FLAG: &str = "--firmware";

fn state_word(state: CheckState) -> &'static str {
    match state {
        CheckState::Ok => "ok",
        CheckState::Warning => "warning",
        CheckState::Failed => "failed",
    }
}

/// A tab or a line break in a message would split it into fields of its own.
fn field(text: &str) -> String {
    text.replace(['\t', '\n'], " ")
}

fn line(check: &Check) -> String {
    let state = state_word(check.state);
    format!("check\t{state}\t{}\t{}\t{}", field(&check.title), field(&check.text), field(&check.fix))
}

fn yes_no(answer: bool) -> &'static str {
    if answer { "yes" } else { "no" }
}

/// The lines after the checks: `gtk` only when there is a GTK, then `gpu`, `amd` and `intel`.
fn closing_lines(gtk: Option<&str>, gpu: bool, amd: bool, intel: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(version) = gtk {
        lines.push(format!("gtk\t{version}"));
    }
    lines.push(format!("gpu\t{}", yes_no(gpu)));
    lines.push(format!("amd\t{}", yes_no(amd)));
    lines.push(format!("intel\t{}", yes_no(intel)));
    lines
}

/// The files a plain VM would boot, with their format, so a test can boot what the app would.
fn print_firmware(context: &Context) {
    let firmware = &context.paths.firmware;
    let format = format_of(&firmware.code);
    println!("firmware\t{}\t{}\t{format}", firmware.code.display(), firmware.vars.display());
}

pub fn main() -> ExitCode {
    let context = match Context::from_process() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if env::args().nth(1).as_deref() == Some(FIRMWARE_FLAG) {
        print_firmware(&context);
        return ExitCode::SUCCESS;
    }
    let report = check_host(&context);
    for check in &report.checks {
        println!("{}", line(check));
    }
    // An AMD card can be a Linux guest's GPU, whatever the checks above say of NVIDIA's.
    let amd = !amd_cards(&context).is_empty();
    // Only a card the Intel daemon is written for counts, as nearly every Intel processor has a GPU.
    let intel = !supported_intel_cards(&context).is_empty();
    for line in closing_lines(gtk_version().as_deref(), report.gpu, amd, intel) {
        println!("{line}");
    }
    ExitCode::SUCCESS
}

// install.sh and the Proxmox host check read these lines, and the format is promised stable (see
// docs/check-output.md). These tests spell it out in full, so a change breaks them first.
#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_app_core::setup::checks::Part;

    fn check(state: CheckState, title: &str, text: &str, fix: &str) -> Check {
        Check { title: title.into(), state, text: text.into(), fix: fix.into(), part: Part::Pc }
    }

    #[test]
    fn a_check_line_is_check_state_title_found_fix_split_by_tabs() {
        let ok = check(CheckState::Ok, "KVM", "/dev/kvm is there.", "");
        assert_eq!(line(&ok), "check\tok\tKVM\t/dev/kvm is there.\t");
        let warning = check(CheckState::Warning, "Title", "Found.", "Fix it.");
        assert_eq!(line(&warning), "check\twarning\tTitle\tFound.\tFix it.");
        let failed = check(CheckState::Failed, "Title", "Found.", "Fix it.");
        assert_eq!(line(&failed), "check\tfailed\tTitle\tFound.\tFix it.");
    }

    #[test]
    fn a_tab_or_line_break_in_a_message_becomes_a_space() {
        let odd = check(CheckState::Ok, "A\tB", "C\nD", "E\t\nF");
        assert_eq!(line(&odd), "check\tok\tA B\tC D\tE  F");
        assert_eq!(line(&odd).split('\t').count(), 5);
    }

    #[test]
    fn the_closing_lines_are_gtk_gpu_amd_intel_in_that_order() {
        assert_eq!(
            closing_lines(Some("4.14.5"), true, false, true),
            ["gtk\t4.14.5", "gpu\tyes", "amd\tno", "intel\tyes"]
        );
        assert_eq!(closing_lines(None, false, true, false), ["gpu\tno", "amd\tyes", "intel\tno"]);
    }
}
