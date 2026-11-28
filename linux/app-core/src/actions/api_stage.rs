//! The API check's last part, written by the app: for each API, whether this start's stage runs its
//! work on the card or withholds it, and so whether how far it got is a failure. The guest's script
//! can't know the stage; the daemon's log says it in its `intel stage=` line.

use guesthouse_backend_intel::stage::{COMPUTE_CONTEXT_TYPES, Stage};

/// How the guest's script starts each line of its summary, and the word before the furthest layer.
const SUMMARY_START: &str = "summary, the furthest layer each API reached:";
const FURTHEST: &str = ": furthest ";
/// The daemon's lines that give the stage and the software desktop.
const STAGE_MARK: &str = "intel stage=";
const SOFTWARE_DESKTOP_ON: &str = "intel software-desktop on";
/// What the script says of an API that got nowhere on Intel's card.
const NOTHING: [&str; 2] = ["nothing", "no device on "];
/// Probes whose work has a context type the daemon knows; the others run only where everything does.
const OPENCL_PROBE: &str = "opencl";
const DIRECT3D_PROBE: &str = "d3d";
/// What the software desktop does to the Direct3D probe.
const DIRECT3D_REFUSED: &str = "Direct3D 9 to 12 refused by the software desktop";

/// The message of a line of the daemon's log: what follows its level word and time, where it has
/// them. Only a line whose message starts with a mark counts, as a message may quote the guest.
fn message_of(line: &str) -> &str {
    const LEVELS: [&str; 5] = ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"];
    let line = line.trim_start();
    let line = match line.split_once(' ') {
        Some((level, rest)) if LEVELS.contains(&level) => rest.trim_start(),
        _ => line,
    };
    match line.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
        Some((_, rest)) => rest.trim_start(),
        None => line,
    }
}

/// The stage named in the daemon's log, if it names one.
fn stage_in(daemon_log: &str) -> Option<Stage> {
    let line = daemon_log.lines().find(|line| message_of(line).starts_with(STAGE_MARK))?;
    let name = line.split(STAGE_MARK).nth(1)?;
    let name: String = name.chars().take_while(char::is_ascii_alphabetic).collect();
    match name.as_str() {
        "Listen" => Some(Stage::Listen),
        "Compute" => Some(Stage::Compute),
        "Everything" => Some(Stage::Everything),
        _ => None,
    }
}

/// Whether the probe's work runs on the card at `stage`.
fn runs(probe: &str, stage: Stage) -> bool {
    let context_type = (probe == OPENCL_PROBE).then_some(COMPUTE_CONTEXT_TYPES[0]);
    stage.runs(context_type)
}

/// One API's line: its work run or withheld, and whether how far it got is a failure.
fn verdict(probe: &str, furthest: &str, stage: Stage, software_desktop: bool) -> String {
    let got_nowhere = NOTHING.iter().any(|nothing| furthest.starts_with(nothing));
    let direct3d_refused = probe == DIRECT3D_PROBE && software_desktop;
    if direct3d_refused && got_nowhere {
        return format!("{DIRECT3D_REFUSED}, so no device on the card is as far as it can get");
    }
    let mut said = if runs(probe, stage) {
        if got_nowhere {
            format!("FAILED: its work runs on the card at stage={stage:?}, and it reached nothing there")
        } else {
            format!("its work runs on the card at stage={stage:?}")
        }
    } else if got_nowhere {
        format!(
            "FAILED: stage={stage:?} withholds its work (finished unrun), not its devices, so reaching \
             nothing isn't the stage's doing"
        )
    } else {
        format!(
            "withheld by stage={stage:?}: its devices are made and its work is finished unrun, so this \
             is as far as it can get"
        )
    };
    if direct3d_refused {
        said.push_str(&format!("; {DIRECT3D_REFUSED}"));
    }
    said
}

/// The lines to put after the guest's report, from its summary and the daemon's log. `device_off`
/// is the app having switched the Intel device off in the guest.
pub fn stage_section(report: &str, daemon_log: Option<&str>, device_off: bool) -> String {
    let mut out = String::from("\neach API at this start's stage, from daemon.out:\n");
    if device_off {
        out.push_str(
            "  the Intel device is switched off in the guest (intel-device-off), so no API has the card\n",
        );
        return out;
    }
    let Some(stage) = daemon_log.and_then(stage_in) else {
        out.push_str("  daemon.out names no stage, so whether a failure is the stage's can't be said\n");
        return out;
    };
    let software_desktop = daemon_log
        .is_some_and(|log| log.lines().any(|line| message_of(line).starts_with(SOFTWARE_DESKTOP_ON)));
    let summary = report.lines().skip_while(|line| !line.starts_with(SUMMARY_START)).skip(1);
    for line in summary {
        let Some((probe_bits, rest)) = line.trim().split_once(FURTHEST) else { break };
        let probe = probe_bits.split_whitespace().next().unwrap_or_default();
        out.push_str(&format!("  {probe_bits}: {}\n", verdict(probe, rest, stage, software_desktop)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = "-- opencl --\n  reached: nothing\n\nsummary, the furthest layer each API reached:\n\
        \x20 d3d 64-bit: furthest D3D12 device, feature level 12_0, on vendor 0x8086 (exit code 0 (0x00000000))\n\
        \x20 opencl 64-bit: furthest kernel enqueued (TIMED OUT after 60 seconds, process killed)\n\
        \x20 levelzero 64-bit: furthest context (exit code 0 (0x00000000))\n\
        \x20 vulkan 64-bit: furthest nothing (exit code 0 (0x00000000))\n\
        \x20 opencl 32-bit: furthest nothing (exit code 0 (0x00000000))\n\napi-check done\n";
    const COMPUTE: &str = "INFO  [    0.003] intel stage=Compute: the guest's work runs on the card\n\
        INFO  [    0.003] intel software-desktop on: every Direct3D driver refused, 9 to 12\n";

    fn line<'a>(section: &'a str, start: &str) -> &'a str {
        section.lines().find(|line| line.trim_start().starts_with(start)).unwrap()
    }

    #[test]
    fn at_the_compute_stage_opencl_runs_and_the_others_are_withheld_or_failed() {
        let section = stage_section(REPORT, Some(COMPUTE), false);
        assert_eq!(
            line(&section, "opencl 64-bit"),
            "  opencl 64-bit: its work runs on the card at stage=Compute"
        );
        assert!(line(&section, "opencl 32-bit").contains("FAILED: its work runs on the card"));
        assert!(line(&section, "levelzero").contains("withheld by stage=Compute"));
        let d3d = line(&section, "d3d 64-bit");
        assert!(d3d.contains("withheld by stage=Compute") && d3d.contains("refused by the software desktop"));
        assert!(line(&section, "vulkan").contains("FAILED: stage=Compute withholds its work"));
    }

    #[test]
    fn only_a_line_that_starts_with_a_mark_counts() {
        let quoted =
            "INFO  [    0.004] process 0x40 is guest pid 7, intel software-desktop on intel stage=Listen\n";
        let log = format!("{}{quoted}", COMPUTE.lines().next().unwrap());
        assert_eq!(stage_in(&log), Some(Stage::Compute));
        let section = stage_section(REPORT, Some(&log), false);
        assert!(!line(&section, "d3d 64-bit").contains("software desktop"));
    }

    #[test]
    fn direct3d_reaching_no_device_on_the_software_desktop_is_no_failure() {
        let report = REPORT.replace("furthest D3D12 device, feature level 12_0,", "furthest no device");
        let log = format!("intel stage=Everything: runs\n{}", COMPUTE.lines().nth(1).unwrap());
        let section = stage_section(&report, Some(&log), false);
        let d3d = line(&section, "d3d 64-bit");
        assert!(!d3d.contains("FAILED") && d3d.contains("refused by the software desktop"), "{d3d}");
        let without = stage_section(&report, Some("intel stage=Everything: runs\n"), false);
        assert!(line(&without, "d3d 64-bit").contains("FAILED"), "the hardware desktop gives Direct3D");
    }

    #[test]
    fn everything_runs_at_the_furthest_stage_and_nothing_at_listen() {
        let everything = stage_section(REPORT, Some("intel stage=Everything: runs\n"), false);
        assert!(line(&everything, "levelzero").contains("its work runs on the card at stage=Everything"));
        assert!(!line(&everything, "d3d").contains("software desktop"));
        let listen =
            stage_section(REPORT, Some("WARN intel stage=Listen layer=card: it is a recording\n"), false);
        assert!(line(&listen, "opencl 64-bit").contains("withheld by stage=Listen"));
    }

    #[test]
    fn a_device_switched_off_or_a_log_without_a_stage_is_said_instead() {
        assert!(stage_section(REPORT, Some(COMPUTE), true).contains("switched off in the guest"));
        assert!(stage_section(REPORT, None, false).contains("names no stage"));
        assert!(stage_section(REPORT, Some("nothing here\n"), false).contains("names no stage"));
    }
}
