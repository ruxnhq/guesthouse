//! Whether Windows really got its GPU. A guest whose GPU failed to start boots all the same, on the
//! basic display, so nothing else in a start would notice.

use super::Task;
use super::identity::after_failed_gpu;
use crate::daemon;
use crate::failure::{Failure, Result};
use crate::gpu_identity::{MICROSOFT_RENDER, identity_of};
use crate::gpu_maker::GpuMaker;
use crate::log::write_log;
use std::fs;
use std::time::Duration;

const WORKING_MARK: &str = "gpu working ";
/// A device that's still starting gets a moment. One that has failed stays that way.
const CHECK_ATTEMPTS: u32 = 10;
const CHECK_POLL: Duration = Duration::from_secs(3);
const CHECK_ANSWER_SECONDS: u64 = 15;
const NO_GPU: &str = "Windows started without its GPU, so the VM was turned off.";
const KEPT_WITHOUT_GPU: &str = "Windows started without its GPU. The VM was left running on Windows' basic display, as \
                                Intel's support is experimental, so dxdiag can run and a bug report can be sent from it.";
const DAEMON_ERROR_MARK: &str = "ERROR ";
const STATE_ANSWER_SECONDS: u64 = 20;
const STATE_MARK: &str = "render device ";

/// The virtual render device, under any identity the QEMU patch can give it.
pub(super) fn is_render_device() -> String {
    let identities = std::iter::once(MICROSOFT_RENDER).chain(GpuMaker::ALL.map(identity_of));
    let patterns =
        identities.map(|identity| format!("$_.PNPDeviceID -like '{}'", identity.windows_pattern()));
    patterns.collect::<Vec<_>>().join(" -or ")
}

fn working_devices(task: &Task) -> Option<u32> {
    let script = format!(
        "$found = @(Get-CimInstance Win32_VideoController | \
         Where-Object {{ ({}) -and $_.ConfigManagerErrorCode -eq 0 }}); \
         \"{WORKING_MARK}$($found.Count)\"",
        is_render_device()
    );
    let said = task.ask_guest(&script, CHECK_ANSWER_SECONDS);
    said.split(WORKING_MARK).nth(1)?.trim().parse().ok()
}

/// What Windows says about the render device, for the log of a check that failed: the device as
/// Plug and Play has it, with its problem code, and each display adapter Windows shows. A check
/// that fails says only that no adapter was working, and the reason is in these.
fn log_device_state(task: &Task) {
    let script = format!(
        "Get-CimInstance Win32_PnPEntity | Where-Object {{ {} }} | ForEach-Object {{ \
         \"{STATE_MARK}$($_.PNPDeviceID) status $($_.Status) problem $($_.ConfigManagerErrorCode)\" }}; \
         Get-CimInstance Win32_VideoController | ForEach-Object {{ \
         \"adapter $($_.Name) at $($_.PNPDeviceID) problem $($_.ConfigManagerErrorCode)\" }}; \
         \"up $([int]((Get-Date) - (Get-CimInstance Win32_OperatingSystem).LastBootUpTime).TotalSeconds) s\"",
        is_render_device()
    );
    let said = task.ask_guest(&script, STATE_ANSWER_SECONDS);
    let said = if said.trim().is_empty() { "Windows did not answer" } else { said.trim() };
    let _ = write_log(task.context, &format!("gpu check: no working GPU. {said}"));
}

/// The daemon's last error, which is the most likely reason.
fn daemons_reason(task: &Task) -> Option<String> {
    let said = fs::read_to_string(task.run_file(daemon::OUTPUT_FILE)).ok()?;
    let last = said.lines().rev().find_map(|line| line.strip_prefix(DAEMON_ERROR_MARK))?;
    let last = without_time(last);
    Some(last.trim().to_owned())
}

/// A verbose log line has its time in brackets after the level. The reason reads the same without it.
fn without_time(message: &str) -> &str {
    let Some(rest) = message.strip_prefix('[') else { return message };
    match rest.split_once("] ") {
        Some((time, after))
            if !time.is_empty()
                && time.bytes().all(|byte| byte.is_ascii_digit() || byte == b' ' || byte == b'.')
                && time.matches('.').count() == 1 =>
        {
            after
        }
        _ => message,
    }
}

/// `said`, with the daemon's reason after it if it gave one.
fn with_daemons_reason(task: &Task, said: &str) -> String {
    match daemons_reason(task) {
        Some(reason) => format!("{said} The daemon said: {reason}"),
        None => said.to_owned(),
    }
}

pub(super) fn gpu_found(task: &Task) -> bool {
    task.step("Checking the GPU");
    (0..CHECK_ATTEMPTS).any(|attempt| {
        if attempt > 0 {
            task.sleep(CHECK_POLL);
        }
        working_devices(task).is_some_and(|count| count > 0)
    })
}

/// Whether Windows got its GPU, for a start that goes on either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Gpu {
    Working,
    /// Windows is up without it, and the machine stays running: see [`runs_without_gpu`].
    Missing,
    /// The desktop failed on an Intel card, and a start on the software desktop has done the rest.
    StartedAgain,
}

/// Whether a machine whose Windows started without its GPU is left running, by its card's maker.
/// Without the GPU, a start on NVIDIA or AMD has failed, so the VM is turned off. Intel is at a test
/// stage where nothing runs on the card yet, and what a run is for is the reports: Windows on its
/// basic display can still run dxdiag and answer a bug report, so the VM stays up for them.
pub(super) fn runs_without_gpu(maker: Option<GpuMaker>) -> bool {
    match maker {
        Some(GpuMaker::Intel) => true,
        Some(GpuMaker::Nvidia | GpuMaker::Amd) | None => false,
    }
}

/// Checks the GPU after a boot. Without it, the next start falls back to Microsoft's vendor ID as
/// always, and then the start fails, or for a maker that runs without its GPU, goes on and says why.
pub(super) fn confirm_gpu(task: &mut Task) -> Result<Gpu> {
    if gpu_found(task) {
        return Ok(Gpu::Working);
    }
    log_device_state(task);
    if !runs_without_gpu(task.machine.windows_maker()) {
        return Err(after_failed_gpu(task, Failure(with_daemons_reason(task, NO_GPU))));
    }
    let said = after_failed_gpu(task, Failure(with_daemons_reason(task, KEPT_WITHOUT_GPU))).0;
    write_log(task.context, &said)?;
    task.error = said;
    Ok(Gpu::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_identity::{AMD_RENDER, INTEL_RENDER, NVIDIA_RENDER};

    #[test]
    fn the_check_accepts_the_render_device_under_every_identity_a_vm_can_show() {
        let test = is_render_device();
        for identity in [MICROSOFT_RENDER, NVIDIA_RENDER, AMD_RENDER, INTEL_RENDER] {
            assert!(test.contains(&identity.windows_pattern()), "{identity:?}: {test}");
        }
    }
}
