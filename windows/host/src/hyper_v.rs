//! Windows' Hyper-V feature, which a bridged machine needs: where it stands, and turning it on.
//!
//! A machine runs on Virtual Machine Platform alone, but a bridged one is plugged into a
//! `Transparent` network, which is Hyper-V's virtual switch bound to one of this PC's adapters, and
//! the service offers bridging only where Hyper-V's virtual machine management service, `vmms`, is
//! installed (see [`crate::network::bridging_from`]). That service comes with the feature Microsoft
//! names `Microsoft-Hyper-V`, which Pro, Education and Enterprise have and Home doesn't. Turning it
//! on is Microsoft's own instruction for it, `DISM /Online /Enable-Feature /All
//! /FeatureName:Microsoft-Hyper-V`, which turns its parent on too, and never restarts the PC here.
//! Whether a smaller part of it would do for the switch alone hasn't been tried, as bridging hasn't
//! been tried on a real PC yet, so the whole feature, as Microsoft says to turn it on, is asked for.
//!
//! This module is every part of that which isn't a call into Windows, so it's tested anywhere: the
//! arguments `dism.exe` is run with, fixed words that never go through a shell; reading what it
//! prints, which is asked for in English with `/English` whatever the system's language; what its
//! exit codes mean, in words; and which case of [`HyperV`] a reading is. `dism.exe` itself is run by
//! [`windows`](crate::windows), behind [`Dism`], so the tests run it through a fake and the real one
//! is never run by a test.

use guesthouse_control::{HYPER_V_FEATURE, HyperV};
use std::time::{Duration, Instant};

/// DISM's exit code when it worked and Windows has to restart to finish (`ERROR_SUCCESS_REBOOT_REQUIRED`).
pub const RESTART_NEEDED: i32 = 3010;
/// DISM's exit code when it worked and Windows is restarting (`ERROR_SUCCESS_REBOOT_INITIATED`),
/// which `/NoRestart` means it never is here.
pub const RESTARTING: i32 = 1641;
/// DISM's exit code for a feature this Windows doesn't have (`CBS_E_UNKNOWN_UPDATE`), as Home and
/// Hyper-V.
pub const UNKNOWN_FEATURE: u32 = 0x800F_080C;

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|word| (*word).to_owned()).collect()
}

/// Asks how the feature stands. A read, which changes nothing.
pub fn info_args() -> Vec<String> {
    let feature = format!("/FeatureName:{HYPER_V_FEATURE}");
    words(&["/English", "/Online", "/Get-FeatureInfo", &feature])
}

/// Turns the feature on, with its parent (`/All`), and leaves the restart to the person
/// (`/NoRestart`). Without `/Quiet`, DISM prints its progress, which is passed on.
pub fn enable_args() -> Vec<String> {
    let feature = format!("/FeatureName:{HYPER_V_FEATURE}");
    words(&["/English", "/Online", "/Enable-Feature", &feature, "/All", "/NoRestart"])
}

/// How the feature stands, as `/Get-FeatureInfo` says on its `State :` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Enabled,
    Disabled,
    /// Turned on, and waiting for a restart.
    EnablePending,
    /// Turned off, and waiting for a restart.
    DisablePending,
    /// Off, with its files removed from the PC, which Windows Update or the installation media has
    /// to give back as it's turned on.
    PayloadRemoved,
}

/// The state in what `/Get-FeatureInfo /English` printed, if it has one.
pub fn feature_state(text: &str) -> Option<Feature> {
    let state = text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == "State").then(|| value.trim().to_ascii_lowercase())
    })?;
    Some(match state.as_str() {
        "enabled" => Feature::Enabled,
        "disabled" => Feature::Disabled,
        "enable pending" => Feature::EnablePending,
        "disable pending" => Feature::DisablePending,
        "disabled with payload removed" => Feature::PayloadRemoved,
        _ => return None,
    })
}

/// The last percentage DISM printed in `text`, as `[====   45.0%   ]` lines, to a whole number.
pub fn progress_in(text: &str) -> Option<u8> {
    let at = text.rfind('%')?;
    let number: String = text[..at]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    let value: f64 = number.parse().ok()?;
    (0.0..=100.0).contains(&value).then_some(value as u8)
}

/// Where DISM said its log is, in what it printed, if it said.
pub fn log_in(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let at = line.find("can be found at ")?;
        Some(line[at + "can be found at ".len()..].trim().to_owned()).filter(|path| !path.is_empty())
    })
}

/// What DISM's error line said, after `Error: <code>`, if it said anything.
fn said(output: &str) -> Option<String> {
    let mut lines = output.lines().map(str::trim).skip_while(|line| !line.starts_with("Error:")).skip(1);
    lines.find(|line| !line.is_empty()).map(str::to_owned)
}

/// What a failed DISM's exit `code` means, in words for the person, with what it said itself when
/// the code is one without words of its own.
pub fn exit_words(code: i32, output: &str) -> String {
    let known = match code as u32 {
        740 => Some("DISM needs an administrator, and wasn't run as one (error 740)."),
        87 => Some("DISM didn't take the options it was given (error 87)."),
        UNKNOWN_FEATURE => Some("This edition of Windows doesn't have Hyper-V (0x800F080C)."),
        0x800F_081F => Some(
            "Windows couldn't find the files Hyper-V needs (0x800F081F). They come from Windows Update, or from \
             the installation media.",
        ),
        0x800F_0906 => Some(
            "Windows couldn't download the files Hyper-V needs (0x800F0906). Check that this PC reaches Windows \
             Update.",
        ),
        0x800F_0922 => Some("Windows couldn't finish setting Hyper-V up (0x800F0922)."),
        0x8007_0005 => Some("Windows refused DISM access (0x80070005)."),
        0x8007_0070 => Some("There isn't room enough on the system drive (0x80070070)."),
        0x8007_0422 => Some(
            "A Windows service DISM needs is turned off (0x80070422), such as Windows Modules Installer or \
             Windows Update.",
        ),
        _ => None,
    };
    match (known, said(output)) {
        (Some(words), _) => words.to_owned(),
        (None, Some(line)) => format!("DISM ended with error 0x{:08X}: {line}", code as u32),
        (None, None) => format!("DISM ended with error 0x{:08X}.", code as u32),
    }
}

/// What a run of `dism.exe` gave: its exit code, and everything it printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub code: i32,
    pub output: String,
}

/// Runs `dism.exe` from System32.
pub trait Dism: Send + Sync {
    /// Runs it with `args`, handing `seen` the end of what it has printed so far each time it
    /// prints more, and gives how it ended, or why it couldn't be run.
    fn run(&self, args: &[String], seen: &mut dyn FnMut(&str)) -> Result<Ran, String>;
    /// Where DISM keeps its log.
    fn log(&self) -> String;
}

/// What a read of the feature found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    Feature(Feature),
    /// DISM doesn't know the feature: this edition doesn't have it.
    NotInEdition,
    /// It couldn't be read, and why.
    Failed(String),
}

/// Reads how the feature stands.
pub fn read(dism: &dyn Dism) -> Read {
    match dism.run(&info_args(), &mut |_| {}) {
        Ok(Ran { code: 0, output }) => match feature_state(&output) {
            Some(feature) => Read::Feature(feature),
            None => Read::Failed("DISM's answer didn't say how Hyper-V stands.".to_owned()),
        },
        Ok(Ran { code, .. }) if code as u32 == UNKNOWN_FEATURE => Read::NotInEdition,
        Ok(Ran { code, output }) => Read::Failed(exit_words(code, &output)),
        Err(why) => Read::Failed(why),
    }
}

/// What the service finds on this PC, which [`hyper_v_from`] makes a case of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// Whether Hyper-V's virtual machine management service, `vmms`, is installed.
    pub service: bool,
    /// How the feature stands, when it was read: it isn't while the service is there.
    pub feature: Option<Read>,
    /// Whether a hypervisor is running, as the processor says, if that could be asked.
    pub hypervisor: Option<bool>,
    /// Whether the processor says virtualization is on in the firmware, if that could be asked.
    pub firmware: Option<bool>,
}

/// The case `reading` is. The service being there is what bridging needs, so that's on whatever
/// else is found. A feature waiting to be turned on at the restart, with no service yet, waits for
/// the restart. With no hypervisor
/// running and the firmware's virtualization off, turning it on wouldn't make it run; with a
/// hypervisor running, as Virtual Machine Platform runs one for the machines, the firmware's flag
/// isn't needed to tell.
pub fn hyper_v_from(reading: &Reading) -> HyperV {
    if reading.service {
        return HyperV::On;
    }
    match &reading.feature {
        None => HyperV::Unknown { reason: "Windows' features are still being read.".to_owned() },
        Some(Read::NotInEdition) => HyperV::NotInEdition,
        Some(Read::Failed(why)) => HyperV::Unknown { reason: why.clone() },
        Some(Read::Feature(Feature::EnablePending)) => HyperV::RestartPending,
        // On with no service is what a restart should have finished, so asking for another isn't
        // said: it isn't known why.
        Some(Read::Feature(Feature::Enabled)) => HyperV::Unknown {
            reason: "Hyper-V is on in Windows' features, but its management service, vmms, isn't installed."
                .to_owned(),
        },
        Some(Read::Feature(Feature::Disabled | Feature::DisablePending | Feature::PayloadRemoved)) => {
            if reading.hypervisor == Some(false) && reading.firmware == Some(false) {
                HyperV::NoVirtualization
            } else {
                HyperV::Off
            }
        }
    }
}

/// How long a read of the feature is kept, as DISM takes seconds. It changes only when the feature
/// is turned on or off, which is read again after, or at a restart.
pub const READ_KEPT: Duration = Duration::from_secs(10 * 60);

/// The last read of the feature, kept so a status never waits on DISM: a read that's due is begun
/// beside it, and the one before is used until it's done. None is begun while the feature is being
/// turned on, as DISM is busy then.
#[derive(Debug, Default)]
pub struct Kept {
    read: Option<(Instant, Read)>,
    reading: bool,
    pub turning: bool,
}

impl Kept {
    /// The read to use at `now`, if there's one yet, and whether a new one is to be begun, which
    /// it's then marked as.
    pub fn take(&mut self, now: Instant) -> (Option<Read>, bool) {
        let due = self.read.as_ref().is_none_or(|(at, _)| now.saturating_duration_since(*at) >= READ_KEPT);
        let begin = due && !self.reading && !self.turning;
        if begin {
            self.reading = true;
        }
        (self.read.as_ref().map(|(_, read)| read.clone()), begin)
    }

    /// Keeps a read made at `now`.
    pub fn put(&mut self, now: Instant, read: Read) {
        self.read = Some((now, read));
        self.reading = false;
    }
}

/// How turning the feature on ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enabled {
    /// On, and Windows has to restart to finish.
    Done,
    /// On already, or waiting for a restart already, so nothing was run.
    Already,
    /// This edition doesn't have it, so nothing was run.
    NotInEdition,
    /// It didn't work, why, and DISM's log.
    Failed { reason: String, log: String },
}

/// Turns the feature on, unless it's on already, handing `progress` how far DISM has got. It reads
/// the feature first, so asking twice runs DISM once, and again after, as DISM's exit code alone
/// doesn't say it's on. It never restarts the PC.
pub fn enable(dism: &dyn Dism, progress: &mut dyn FnMut(u8)) -> Enabled {
    let failed = |reason: String| Enabled::Failed { reason, log: dism.log() };
    match read(dism) {
        Read::Feature(Feature::Enabled | Feature::EnablePending) => return Enabled::Already,
        Read::NotInEdition => return Enabled::NotInEdition,
        Read::Feature(_) => {}
        Read::Failed(why) => {
            return failed(format!("How Hyper-V stands couldn't be read, so it wasn't turned on: {why}"));
        }
    }
    let mut last = None;
    let ran = dism.run(&enable_args(), &mut |text| {
        let now = progress_in(text);
        if now.is_some() && now != last {
            last = now;
            progress(now.unwrap_or_default());
        }
    });
    let Ran { code, output } = match ran {
        Ok(ran) => ran,
        Err(why) => return failed(why),
    };
    if !matches!(code, 0 | RESTART_NEEDED | RESTARTING) {
        let log = log_in(&output).unwrap_or_else(|| dism.log());
        return Enabled::Failed { reason: exit_words(code, &output), log };
    }
    match read(dism) {
        Read::Feature(Feature::Enabled | Feature::EnablePending) => Enabled::Done,
        Read::Feature(other) => failed(format!("DISM said it worked, but Hyper-V reads as {other:?} after.")),
        Read::NotInEdition => Enabled::NotInEdition,
        Read::Failed(why) => {
            failed(format!("DISM said it worked, but Hyper-V couldn't be read after: {why}"))
        }
    }
}

#[cfg(test)]
mod tests;
