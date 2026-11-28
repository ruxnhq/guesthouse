//! Which PCI identity the virtual render device shows Windows. Windows' own virtual render driver
//! only binds to Microsoft's, so every machine starts with that one. Browsers and streaming programs
//! look at the vendor number before they use a GPU, and stay on the CPU when it's Microsoft's. So once
//! a guest holds a driver package for its card maker's number, NVIDIA's, AMD's or Intel's, written there from
//! its own copy of Windows' driver, every VM after that shows that number. The choice is kept in the
//! machine's folder, as it has to outlive a VM: a guest shown a number without its package would
//! start with no GPU.

use crate::context::Context;
use crate::failure::Result;
use crate::gpu_maker::GpuMaker;
use crate::machine::Machine;
use crate::settings::write_json;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const STATE_FILE: &str = "gpu-identity.json";

/// A PCI vendor and device number, as the guest's bus shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciIdentity {
    pub vendor: u16,
    pub device: u16,
}

/// Microsoft's virtual render device, which Windows' own driver binds to. QEMU gives this one by itself.
pub const MICROSOFT_RENDER: PciIdentity = PciIdentity { vendor: 0x1414, device: 0x008E };
/// NVIDIA's vendor number with a device number NVIDIA has never shipped, so NVIDIA's own driver can't
/// claim the device.
pub const NVIDIA_RENDER: PciIdentity = PciIdentity { vendor: 0x10DE, device: 0x2BFE };
/// AMD's vendor number with a device number AMD has never shipped, far from every range it uses.
pub const AMD_RENDER: PciIdentity = PciIdentity { vendor: 0x1002, device: 0x7FFE };

/// Intel's vendor number with a device number Intel has never shipped, in a block it has never used.
pub const INTEL_RENDER: PciIdentity = PciIdentity { vendor: 0x8086, device: 0xCFFE };

/// The number a guest on a card of this maker shows once it holds the package.
pub fn identity_of(maker: GpuMaker) -> PciIdentity {
    match maker {
        GpuMaker::Nvidia => NVIDIA_RENDER,
        GpuMaker::Amd => AMD_RENDER,
        GpuMaker::Intel => INTEL_RENDER,
    }
}

/// The maker of the machine's card. Only a Windows machine has one that matters here, and any other
/// has always been counted as NVIDIA's.
pub(crate) fn machine_maker(machine: &Machine) -> GpuMaker {
    machine.windows_maker().unwrap_or(GpuMaker::Nvidia)
}

/// The number a machine shows once its guest holds the package: its card maker's. A Windows machine
/// stays with one maker, so this never changes under a guest.
pub fn maker_identity(machine: &Machine) -> PciIdentity {
    identity_of(machine_maker(machine))
}

/// The card maker's name, for what the user hears.
pub fn maker_name(machine: &Machine) -> &'static str {
    machine_maker(machine).name()
}

impl PciIdentity {
    /// The start of the hardware id Windows gives the device, with the rest left open.
    pub fn windows_pattern(self) -> String {
        format!(r"PCI\VEN_{:04X}&DEV_{:04X}*", self.vendor, self.device)
    }

    /// What the render device's QEMU line needs after its other properties. Nothing for the identity
    /// QEMU gives by itself, so a QEMU from before the properties still starts.
    pub fn qemu_properties(self) -> String {
        if self == MICROSOFT_RENDER {
            return String::new();
        }
        format!(",vendor-id={:#06x},device-id={:#06x}", self.vendor, self.device)
    }
}

/// Which identity a machine's next start shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shown {
    #[default]
    Microsoft,
    /// The card maker's, [`maker_identity`]. Written as "nvidia" before AMD cards came.
    #[serde(alias = "nvidia")]
    Maker,
}

impl Shown {
    pub fn identity(self, machine: &Machine) -> PciIdentity {
        match self {
            Self::Microsoft => MICROSOFT_RENDER,
            Self::Maker => maker_identity(machine),
        }
    }
}

/// What a state from before AMD cards holds a package for.
fn nvidia_vendor() -> u16 {
    NVIDIA_RENDER.vendor
}

/// How a machine stands. Tasks write it in the machine's folder, and the app reads it from there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityState {
    pub shown: Shown,
    /// The guest script that failed to write the package, which a start doesn't run again.
    #[serde(default)]
    pub failed_script: String,
    /// Why Windows stays on Microsoft's number.
    #[serde(default)]
    pub reason: String,
    /// What the user should hear about a machine that's up.
    #[serde(default)]
    pub note: String,
    /// The next start is Windows' first on its maker's number, which ends with Windows restarting once.
    #[serde(default)]
    pub restart_due: bool,
    /// The vendor number of the package Windows holds.
    #[serde(default = "nvidia_vendor")]
    pub package_vendor: u16,
    /// How many starts on the maker's number in a row have ended with Windows lacking its GPU. One
    /// that gets its GPU there clears it.
    #[serde(default)]
    pub failed_checks: u32,
}

impl Default for IdentityState {
    fn default() -> Self {
        Self {
            shown: Shown::default(),
            failed_script: String::new(),
            reason: String::new(),
            note: String::new(),
            restart_due: false,
            package_vendor: nvidia_vendor(),
            failed_checks: 0,
        }
    }
}

impl IdentityState {
    /// Whether this start, the first on its maker's number, has to restart Windows before it's ready.
    pub fn restarts_windows(&self) -> bool {
        self.shown == Shown::Maker && self.restart_due
    }

    /// What the user should hear, if anything. A note is about the machine as it runs now.
    pub fn problem(&self, up: bool) -> &str {
        if self.reason.is_empty() && up { &self.note } else { &self.reason }
    }

    /// Whether the maker's number has failed its GPU check as often as it's tried by itself.
    pub fn out_of_tries(&self) -> bool {
        self.failed_checks >= CHECK_TRIES
    }

    /// Whether the machine is on Microsoft's number because a start on its maker's had no GPU.
    pub fn fell_back(&self) -> bool {
        self.shown == Shown::Microsoft && self.reason == FELL_BACK
    }

    /// What the machine's row says. A machine that stays on Microsoft's number for a reason is also
    /// told what that costs, as the game that won't start says nothing about a vendor number.
    pub fn row_text(&self, up: bool, machine: &Machine) -> String {
        let said = self.problem(up);
        if self.shown != Shown::Microsoft || self.reason.is_empty() {
            return said.to_owned();
        }
        let maker = maker_name(machine);
        if !self.fell_back() {
            return format!("{said} {NO_GAMES}");
        }
        if self.out_of_tries() {
            return format!(
                "Windows started without its GPU on {maker}'s vendor ID {} times, so it stays on \
                 Microsoft's. {NO_GAMES} To try {maker}'s again: guesthouse-vm set {} vendor-id retry",
                self.failed_checks, machine.id
            );
        }
        format!(
            "Windows started without its GPU on {maker}'s vendor ID, so it sees Microsoft's for now. \
             The next start tries {maker}'s again. {NO_GAMES}"
        )
    }

    /// For the user's own word to try the maker's number again: whatever kept the machine on
    /// Microsoft's is forgotten, and the next start has Windows take the package afresh. It says
    /// whether there was anything to forget.
    pub fn try_again(&mut self) -> bool {
        if self.shown == Shown::Maker {
            return false;
        }
        let kept = !self.failed_script.is_empty() || self.failed_checks > 0 || !self.reason.is_empty();
        self.failed_script.clear();
        self.failed_checks = 0;
        self.reason.clear();
        kept
    }
}

/// Where this machine's choice of identity is kept.
pub fn state_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(STATE_FILE)
}

/// What a machine moved to Microsoft's number after a failed GPU check is told, and keeps as its
/// reason.
pub const FELL_BACK: &str = "The next start shows Windows Microsoft's vendor ID again.";
/// How many starts in a row on the maker's number may end without the GPU before a machine stays on
/// Microsoft's: the first, and one more try. A check can fail once on a guest that takes the number
/// well at its next start, and each failed one costs the user a start, so it's two and no more.
pub const CHECK_TRIES: u32 = 2;
/// What a machine kept on Microsoft's number is told it costs.
pub const NO_GAMES: &str = "Some games will not start while Windows sees Microsoft's vendor ID.";
/// What an Intel machine is told instead.
pub const STAYS_ON_INTEL: &str = "Windows keeps Intel's vendor ID, as Intel's Direct3D 12 driver doesn't open on \
                                  Microsoft's, and nothing would put Intel's back.";

/// Whether a failed GPU check moves a machine on this maker's card to Microsoft's number for its next
/// start. On NVIDIA and AMD, the maker's number is the first suspect and Microsoft's always works.
/// Intel's Direct3D 12 driver doesn't open on Microsoft's number at all, and while Intel is experimental a
/// failed check is what the stage is, not the number, so an Intel machine keeps its maker's.
pub fn falls_back_after_failed_check(maker: GpuMaker) -> bool {
    match maker {
        GpuMaker::Nvidia | GpuMaker::Amd => true,
        GpuMaker::Intel => false,
    }
}

/// An earlier build did move Intel machines to Microsoft's number after a failed check, with Intel's
/// package already in Windows. Such a machine goes back to Intel's number at its next start, as if
/// it had never moved: Windows has the device under Intel's number from before.
fn undo_old_fall_back(state: &mut IdentityState, machine: &Machine) {
    let moved_back = state.shown == Shown::Microsoft && state.reason == FELL_BACK;
    let maker = machine_maker(machine);
    if moved_back
        && !falls_back_after_failed_check(maker)
        && state.package_vendor == identity_of(maker).vendor
    {
        state.shown = Shown::Maker;
        state.failed_script.clear();
        state.reason.clear();
    }
}

/// Builds before the count of failed checks kept a machine on Microsoft's number after its first one,
/// by naming the script as failed, until a build with another script. Such a machine has had one
/// failed check, and gets the try that's left.
fn count_old_fall_back(state: &mut IdentityState) {
    if state.fell_back() && state.failed_checks == 0 && !state.failed_script.is_empty() {
        state.failed_script.clear();
        state.failed_checks = 1;
    }
}

/// A file that's missing or bad says Microsoft's number, which always works. So does a package for
/// another maker's number than this machine's, which the guest gets again for its own maker.
pub fn read_state(context: &Context, machine: &Machine) -> IdentityState {
    let text = fs::read_to_string(state_file(context, machine)).unwrap_or_default();
    let mut state: IdentityState = serde_json::from_str(&text).unwrap_or_default();
    undo_old_fall_back(&mut state, machine);
    count_old_fall_back(&mut state);
    if state.shown == Shown::Maker && state.package_vendor != maker_identity(machine).vendor {
        state.shown = Shown::Microsoft;
        state.restart_due = false;
    }
    state
}

pub fn write_state(context: &Context, machine: &Machine, state: &IdentityState) -> Result<()> {
    write_json(&state_file(context, machine), state)
}

/// For a new Windows on this machine's disk: what the old one had taken in is gone with it.
pub fn forget_state(context: &Context, machine: &Machine) {
    let _ = fs::remove_file(state_file(context, machine));
}

/// The identity this machine's next start shows.
pub fn shown_identity(context: &Context, machine: &Machine) -> PciIdentity {
    read_state(context, machine).shown.identity(machine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    #[test]
    fn the_patterns_are_how_windows_writes_a_hardware_id() {
        assert_eq!(MICROSOFT_RENDER.windows_pattern(), r"PCI\VEN_1414&DEV_008E*");
        assert_eq!(NVIDIA_RENDER.windows_pattern(), r"PCI\VEN_10DE&DEV_2BFE*");
        assert_eq!(INTEL_RENDER.windows_pattern(), r"PCI\VEN_8086&DEV_CFFE*");
    }

    #[test]
    fn each_makers_identity_has_its_vendor_number_and_no_two_are_alike() {
        for maker in GpuMaker::ALL {
            assert_eq!(identity_of(maker).vendor, maker.vendor(), "{maker:?}");
            assert_ne!(identity_of(maker), MICROSOFT_RENDER);
        }
        assert_ne!(identity_of(GpuMaker::Amd).device, identity_of(GpuMaker::Intel).device);
    }

    #[test]
    fn an_intel_machine_shows_intels_number_and_says_intel() {
        let mut machine = Machine::new("x", "x", "", "");
        machine.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
        assert_eq!((maker_identity(&machine), maker_name(&machine)), (INTEL_RENDER, "Intel"));
        assert_eq!(INTEL_RENDER.qemu_properties(), ",vendor-id=0x8086,device-id=0xcffe");
        let mut amd = machine;
        amd.use_windows_card(GpuMaker::Amd, "0000:7c:00.0");
        assert_eq!((maker_identity(&amd), maker_name(&amd)), (AMD_RENDER, "AMD"));
    }

    #[test]
    fn only_another_identity_than_qemus_own_needs_properties() {
        assert_eq!(MICROSOFT_RENDER.qemu_properties(), "");
        assert_eq!(NVIDIA_RENDER.qemu_properties(), ",vendor-id=0x10de,device-id=0x2bfe");
    }

    #[test]
    fn a_machine_with_nothing_written_shows_microsofts() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = Machine::new("x", "x", "", "");
        assert_eq!(shown_identity(&context, &machine), MICROSOFT_RENDER);
        let state = IdentityState { shown: Shown::Maker, ..IdentityState::default() };
        write_state(&context, &machine, &state).unwrap();
        assert_eq!(shown_identity(&context, &machine), NVIDIA_RENDER);
        assert_eq!(read_state(&context, &machine), state);
    }

    #[test]
    fn a_reason_is_heard_always_and_a_note_only_while_up() {
        let state = IdentityState { note: "later".into(), ..IdentityState::default() };
        assert_eq!(state.problem(true), "later");
        assert_eq!(state.problem(false), "");
        let state = IdentityState { reason: "why".into(), ..state };
        assert_eq!(state.problem(true), "why");
        assert_eq!(state.problem(false), "why");
    }

    /// The tester's machine as 0.6.2-intel.2 left it after a failed check.
    fn moved_by_an_earlier_build(context: &Context, machine: &Machine) {
        let state = IdentityState {
            shown: Shown::Microsoft,
            failed_script: "0dc59626496a5133".into(),
            reason: FELL_BACK.into(),
            package_vendor: INTEL_RENDER.vendor,
            ..IdentityState::default()
        };
        write_state(context, machine, &state).unwrap();
    }

    #[test]
    fn an_intel_machine_an_earlier_build_moved_to_microsofts_number_goes_back_to_intels() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut machine = Machine::new("arc", "Arc", "/d", "/v");
        machine.use_windows_card(GpuMaker::Intel, "0000:03:00.0");
        fs::create_dir_all(machine.folder(&context)).unwrap();
        moved_by_an_earlier_build(&context, &machine);
        let state = read_state(&context, &machine);
        assert_eq!(
            (state.shown, state.failed_script.as_str(), state.reason.as_str()),
            (Shown::Maker, "", "")
        );
        assert_eq!(shown_identity(&context, &machine), INTEL_RENDER);
    }

    #[test]
    fn an_nvidia_or_amd_machine_that_fell_back_stays_on_microsofts_number() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut amd = Machine::new("radeon", "Radeon", "/d", "/v");
        amd.use_windows_card(GpuMaker::Amd, "0000:7c:00.0");
        fs::create_dir_all(amd.folder(&context)).unwrap();
        let state = IdentityState {
            shown: Shown::Microsoft,
            reason: FELL_BACK.into(),
            package_vendor: AMD_RENDER.vendor,
            ..IdentityState::default()
        };
        write_state(&context, &amd, &state).unwrap();
        assert_eq!(read_state(&context, &amd).shown, Shown::Microsoft);
        assert!(
            falls_back_after_failed_check(GpuMaker::Nvidia)
                && !falls_back_after_failed_check(GpuMaker::Intel)
        );
    }

    /// The file a machine was left with by 0.10.0 after one failed check.
    const PINNED_BY_AN_EARLIER_BUILD: &str = r#"{"shown":"microsoft","failed_script":"0dc59626496a5133",
        "reason":"The next start shows Windows Microsoft's vendor ID again.","note":"","restart_due":false,
        "package_vendor":4318}"#;

    fn nvidia_machine(context: &Context) -> Machine {
        let machine = Machine::new("win", "Win", "/d", "/v");
        fs::create_dir_all(machine.folder(context)).unwrap();
        machine
    }

    #[test]
    fn a_machine_an_earlier_build_pinned_after_one_failed_check_has_one_try_left() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = nvidia_machine(&context);
        fs::write(state_file(&context, &machine), PINNED_BY_AN_EARLIER_BUILD).unwrap();
        let state = read_state(&context, &machine);
        assert_eq!(
            (state.shown, state.failed_script.as_str(), state.failed_checks),
            (Shown::Microsoft, "", 1)
        );
        assert!(state.fell_back() && !state.out_of_tries());
        let said = state.row_text(false, &machine);
        assert!(said.contains("The next start tries NVIDIA's again.") && said.ends_with(NO_GAMES), "{said}");
    }

    #[test]
    fn a_script_that_could_not_make_the_package_stays_named_as_failed() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = nvidia_machine(&context);
        let refused = IdentityState {
            failed_script: "abc".into(),
            reason: "Windows keeps Microsoft's vendor ID. Why.".into(),
            ..IdentityState::default()
        };
        write_state(&context, &machine, &refused).unwrap();
        assert_eq!(read_state(&context, &machine), refused);
        assert_eq!(
            refused.row_text(true, &machine),
            format!("Windows keeps Microsoft's vendor ID. Why. {NO_GAMES}")
        );
    }

    #[test]
    fn a_machine_out_of_tries_is_told_how_to_try_again_and_what_it_costs() {
        let machine = Machine::new("win", "Win", "/d", "/v");
        let stuck = IdentityState {
            failed_script: "abc".into(),
            reason: FELL_BACK.into(),
            failed_checks: CHECK_TRIES,
            ..IdentityState::default()
        };
        assert!(stuck.out_of_tries());
        assert_eq!(
            stuck.row_text(false, &machine),
            format!(
                "Windows started without its GPU on NVIDIA's vendor ID 2 times, so it stays on Microsoft's. \
                 {NO_GAMES} To try NVIDIA's again: guesthouse-vm set win vendor-id retry"
            )
        );
    }

    #[test]
    fn a_machine_on_its_makers_number_or_with_nothing_wrong_is_told_nothing_about_games() {
        let machine = Machine::new("win", "Win", "/d", "/v");
        let fine = IdentityState { shown: Shown::Maker, note: "later".into(), ..IdentityState::default() };
        assert_eq!(
            (fine.row_text(true, &machine), fine.row_text(false, &machine)),
            ("later".into(), String::new())
        );
        let waiting = IdentityState { note: "next start".into(), ..IdentityState::default() };
        assert_eq!(waiting.row_text(true, &machine), "next start", "about to move, with no reason to stay");
        assert_eq!(IdentityState::default().row_text(true, &machine), "");
    }

    #[test]
    fn trying_again_forgets_what_kept_the_machine_on_microsofts_number() {
        let mut stuck = IdentityState {
            failed_script: "abc".into(),
            reason: FELL_BACK.into(),
            failed_checks: CHECK_TRIES,
            ..IdentityState::default()
        };
        assert!(stuck.try_again());
        assert_eq!(stuck, IdentityState::default());
        assert!(!stuck.try_again(), "nothing is left to forget");
        let mut settled = IdentityState { shown: Shown::Maker, ..IdentityState::default() };
        assert!(!settled.try_again());
        assert_eq!(settled.shown, Shown::Maker);
    }
}
