//! What a guest on an Intel card gets. Every part of it follows one switch, [`Desktop`]: the drivers
//! each program is given, the compositor's, and whether the Guesthouse display driver may be in the
//! guest. A real card's guest is given the hardware desktop unless someone asks otherwise; where the
//! stage doesn't run everything it is given the software desktop instead, with the reason
//! ([`Desktop::at_stage`]), and the app falls back by itself when the hardware desktop doesn't come
//! up. A recorded card keeps the software desktop.
//!
//! In the software desktop, every program is refused Intel's Direct3D drivers, 9 to 12, and draws in
//! software. Refused 11 alone, Windows runs a program's Direct3D 11 device on Intel's Direct3D 12
//! driver instead, so the shell's programs (the sign-in screen, Explorer, Start) would still draw on
//! the card, for a compositor that has no driver of it and can't show it. On a card whose record
//! lets that through they fail and are started again, over and over, and the desktop never comes
//! up. The compositor itself is refused every driver: refused 11, it takes 12, and as nothing it
//! draws is run, it gives up and crashes over and over until Windows stops the adapter for every
//! program. Compute and Vulkan stay, which is what a run is for.
//!
//! The display driver is kept off then. With it in, a lab guest's compositor crashed in dwmcore.dll
//! at each start, as on a tester's, until Winlogon stopped the session's use of the GPU and both
//! display devices failed. On the basic display alone it draws in software and stays up.

use std::ffi::OsStr;

/// The desktop's compositor, by its program's file name.
const COMPOSITOR: &str = "dwm.exe";

/// `0` asks for the hardware desktop and `1` for the software one. Unset, or anything else, leaves
/// the choice to [`Desktop::default_for`].
pub const DESKTOP_VARIABLE: &str = "GUESTHOUSE_INTEL_SOFTWARE_DESKTOP";
const HARDWARE_DESKTOP_VALUE: &str = "0";
const SOFTWARE_DESKTOP_VALUE: &str = "1";

/// The desktop a guest on a real card gets unless a person asks for the other.
pub const STAGE_DESKTOP: Desktop = Desktop::Hardware;

/// A recorded card stands in for a card in the lab, and nothing can draw on it, so its guests keep
/// the software desktop unless someone asks.
const RECORDED_CARD_DESKTOP: Desktop = Desktop::Software;

/// Where the guest's desktop is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Desktop {
    /// In software, on the basic display, with Intel's card for everything else.
    Software,
    /// By Intel's card, with every driver given and the display driver in.
    Hardware,
}

impl Desktop {
    /// The desktop a person asked for in [`DESKTOP_VARIABLE`]'s value, if they asked.
    pub fn asked(value: Option<&OsStr>) -> Option<Self> {
        match value? {
            value if value == HARDWARE_DESKTOP_VALUE => Some(Self::Hardware),
            value if value == SOFTWARE_DESKTOP_VALUE => Some(Self::Software),
            _ => None,
        }
    }

    /// The desktop a guest gets when nobody asked: the stage's, or a recorded card's.
    pub fn default_for(recorded_card: bool) -> Self {
        if recorded_card { RECORDED_CARD_DESKTOP } else { STAGE_DESKTOP }
    }

    /// The desktop from [`DESKTOP_VARIABLE`]'s value, or the default.
    pub fn from_value(value: Option<&OsStr>, recorded_card: bool) -> Self {
        Self::asked(value).unwrap_or(Self::default_for(recorded_card))
    }

    /// What a person asked for in this process's environment, if anything.
    pub fn asked_in_environment() -> Option<Self> {
        Self::asked(std::env::var_os(DESKTOP_VARIABLE).as_deref())
    }

    /// The desktop this process was started with.
    pub fn from_environment() -> Self {
        let recorded_card = crate::recording::recorded_card_file().is_some();
        Self::from_value(std::env::var_os(DESKTOP_VARIABLE).as_deref(), recorded_card)
    }

    /// The value of [`DESKTOP_VARIABLE`] that asks for this desktop.
    pub fn value(self) -> &'static str {
        match self {
            Self::Hardware => HARDWARE_DESKTOP_VALUE,
            Self::Software => SOFTWARE_DESKTOP_VALUE,
        }
    }

    /// Whether `program` is refused Intel's Direct3D drivers, and why.
    pub fn withheld(self, program: &str) -> Option<Withheld> {
        match self {
            Self::Hardware => None,
            Self::Software if program.eq_ignore_ascii_case(COMPOSITOR) => Some(Withheld::FromTheCompositor),
            Self::Software => Some(Withheld::Direct3d),
        }
    }

    /// Whether `program` is told a driver update is in progress: the compositor, on the software
    /// desktop, so its failed tries on the card don't count against it. See
    /// [`crate::protocol::update_status`].
    pub fn tells_compositor_of_an_update(self, program: &str) -> bool {
        self == Self::Software && program.eq_ignore_ascii_case(COMPOSITOR)
    }

    /// Whether the Guesthouse display driver may be in the guest.
    pub fn has_display_driver(self) -> bool {
        self == Self::Hardware
    }

    /// The desktop a guest is given at `stage`, with why it isn't this one if it isn't. The
    /// hardware desktop needs everything the compositor sends to run: at any stage short of that
    /// nothing it draws reaches the screen, so the guest gets the software desktop in the same start.
    /// `why` is the reason the stage is listening, if it is. `lab_kept` is a lab asking for the
    /// hardware desktop all the same, to see it fail: see [`LAB_HARDWARE_DESKTOP_VARIABLE`].
    pub fn at_stage(self, stage: Stage, why: Option<&Fallback>, lab_kept: bool) -> (Self, Option<String>) {
        if self == Self::Software || stage == Stage::Everything || lab_kept {
            return (self, None);
        }
        let reason = match stage {
            Stage::Compute => format!("stage=Compute: {RUN_VARIABLE}=compute runs compute work only"),
            _ => listening_note(why),
        };
        (Self::Software, Some(reason))
    }

    /// What the daemon tells the app of the desktop it `given` at `stage`: why the hardware desktop
    /// asked wasn't given (`why`, from [`Self::at_stage`]), or, for a software desktop that was asked
    /// for, that nothing runs on the card when the stage is listening. The app keeps the Intel device
    /// on in the guest only for work that runs, so it has to know.
    pub fn note(
        given: Self,
        why: Option<String>,
        stage: Stage,
        listening: Option<&Fallback>,
    ) -> Option<String> {
        why.or_else(|| (given == Self::Software && stage == Stage::Listen).then(|| listening_note(listening)))
    }
}

/// How the daemon's note starts when nothing runs on the card.
pub const LISTENING_NOTE_START: &str = "stage=Listen";

fn listening_note(why: Option<&Fallback>) -> String {
    format!("{LISTENING_NOTE_START} {}", why.cloned().unwrap_or(Fallback::AskedFor))
}

/// `1` keeps the hardware desktop a person asked for on a recorded card, where nothing runs and so
/// nothing can be drawn. For a lab only, to bring about a hardware desktop's failures on purpose;
/// it has no effect on a real card.
pub const LAB_HARDWARE_DESKTOP_VARIABLE: &str = "GUESTHOUSE_INTEL_LAB_HARDWARE_DESKTOP_ON_RECORDED_CARD";

/// Whether a lab asked for [`LAB_HARDWARE_DESKTOP_VARIABLE`]'s exception, for a recorded card.
pub fn lab_keeps_hardware_desktop(value: Option<&OsStr>, recorded_card: bool) -> bool {
    recorded_card && value.is_some_and(|value| value == "1")
}

/// `1` has the app keep the Intel device on in the guest for work asked on a recorded card, as it
/// does on a real one. For a lab whose daemon runs the lab card (a build with the `lab-fake-card`
/// feature); it has no effect on a real card, and a daemon that listens still has the device
/// switched off by its note.
pub const LAB_DEVICE_ON_RECORDED_CARD_VARIABLE: &str = "GUESTHOUSE_INTEL_LAB_DEVICE_ON_RECORDED_CARD";

/// Whether a lab asked for [`LAB_DEVICE_ON_RECORDED_CARD_VARIABLE`]'s exception, for a recorded card.
pub fn lab_keeps_device_on_recorded_card(value: Option<&OsStr>, recorded_card: bool) -> bool {
    recorded_card && value.is_some_and(|value| value == "1")
}

/// Names what a guest's work is run on: `0` or `listen` keeps everything off the card, as the first
/// test builds did, `compute` runs compute work only, and unset runs everything this build can.
pub const RUN_VARIABLE: &str = "GUESTHOUSE_INTEL_RUN_ON_CARD";

/// How much of a guest's work runs on the card. The desktop's compositor is [`Desktop`]'s business:
/// in the software desktop it is given no driver, so it sends nothing to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Nothing is made on the card and nothing runs: memory is names, and every submission is
    /// finished unrun. This is all a recorded card can do.
    Listen,
    /// Real memory, address spaces and queues, and compute work runs: the submissions whose header
    /// says OpenCL. The rest are finished unrun.
    Compute,
    /// Everything a guest submits runs.
    Everything,
}

/// The furthest stage this build has the code for, which is what a guest gets on a card that can
/// take it unless a person asks for less.
pub const FURTHEST_STAGE: Stage = Stage::Everything;

/// Why a guest is left at [`Stage::Listen`] when more was asked for, by the layer that refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fallback {
    /// A person asked for it with [`RUN_VARIABLE`].
    AskedFor,
    /// [`RUN_VARIABLE`] holds a word this build doesn't know.
    UnknownWord(String),
    /// The card is a recording, so there is nothing to run on.
    RecordedCard,
    /// The card's kernel driver isn't xe.
    NotXe(String),
    /// The render node couldn't be opened for work.
    NodeNotOpened(String),
    /// The host's own test of xe failed these steps, which the guest's work needs.
    SelfTestFailed(Vec<String>),
    /// The card's memory regions give nowhere to put a buffer.
    NoMemoryRegion,
    /// The host's own desktop leaves the guest this many bytes of the card, which is too few.
    TooLittleMemory(u64),
}

impl std::fmt::Display for Fallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AskedFor => write!(f, "layer=switch: {RUN_VARIABLE} asks for it"),
            Self::UnknownWord(word) => {
                write!(f, "layer=switch: {RUN_VARIABLE} is \"{word}\", which this build doesn't know")
            }
            Self::RecordedCard => f.write_str("layer=card: it is a recording, with no card behind it"),
            Self::NotXe(driver) => {
                write!(f, "layer=host-driver: the card is on {driver}, and work runs through xe only")
            }
            Self::NodeNotOpened(error) => write!(f, "layer=render-node: opening it for work failed: {error}"),
            Self::SelfTestFailed(steps) => {
                write!(
                    f,
                    "layer=host-self-test: its steps {} failed on this card, and work needs them (intel-probe --run shows them)",
                    steps.join(", ")
                )
            }
            Self::NoMemoryRegion => f.write_str("layer=card-memory: xe named no system memory region"),
            Self::TooLittleMemory(left) => write!(
                f,
                "layer=card-memory: only {} MiB of the card is free beside the host's own use and reserve",
                left >> 20
            ),
        }
    }
}

impl Stage {
    /// The stage a person asked for with [`RUN_VARIABLE`]'s value, and why it is less than the
    /// furthest if it is. A word this build doesn't know is never read as "run".
    pub fn asked(value: Option<&OsStr>) -> (Self, Option<Fallback>) {
        let Some(value) = value else { return (FURTHEST_STAGE, None) };
        match value.to_string_lossy().trim().to_ascii_lowercase().as_str() {
            "0" | "listen" => (Self::Listen, Some(Fallback::AskedFor)),
            "compute" => (Self::Compute, None),
            "1" | "everything" => (FURTHEST_STAGE, None),
            unknown => (Self::Listen, Some(Fallback::UnknownWord(unknown.to_string()))),
        }
    }

    /// The stage this process was started with.
    pub fn from_environment() -> (Self, Option<Fallback>) {
        Self::asked(std::env::var_os(RUN_VARIABLE).as_deref())
    }

    /// Whether memory, address spaces and queues are real at this stage.
    pub fn is_real(self) -> bool {
        self != Self::Listen
    }

    /// Whether a submission whose header gives this context type runs on the card. One whose
    /// header can't be read has none, and runs only where everything does.
    pub fn runs(self, context_type: Option<u32>) -> bool {
        match self {
            Self::Listen => false,
            Self::Compute => context_type.is_some_and(|kind| COMPUTE_CONTEXT_TYPES.contains(&kind)),
            Self::Everything => true,
        }
    }
}

/// The header context types that are compute work: 5 is OpenCL. Level Zero's isn't known yet, so
/// at [`Stage::Compute`] its work is finished unrun.
pub const COMPUTE_CONTEXT_TYPES: [u32; 1] = [5];

/// Why a driver is withheld.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Withheld {
    /// Every Direct3D version, from every program.
    Direct3d,
    /// Any version, from the compositor.
    FromTheCompositor,
}

impl Withheld {
    pub fn why(self) -> &'static str {
        match self {
            Self::Direct3d => "held back by the software desktop switch",
            Self::FromTheCompositor => "the compositor gets no Intel driver while the desktop is in software",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_software_desktop_withholds_direct3d_from_every_program_and_the_compositor_has_its_own_reason() {
        let software = Desktop::Software;
        assert_eq!(software.withheld("DWM.EXE"), Some(Withheld::FromTheCompositor));
        assert_eq!(software.withheld("dwm.exe"), Some(Withheld::FromTheCompositor));
        for program in ["explorer.exe", "LogonUI.exe", "game.exe", ""] {
            assert_eq!(software.withheld(program), Some(Withheld::Direct3d), "{program}");
        }
        assert_eq!(Desktop::Hardware.withheld("dwm.exe"), None, "all of Intel's drivers for everyone");
        assert_eq!(Desktop::Hardware.withheld("explorer.exe"), None);
    }

    #[test]
    fn only_the_compositor_on_the_software_desktop_is_told_of_an_update() {
        assert!(Desktop::Software.tells_compositor_of_an_update("DWM.EXE"));
        assert!(!Desktop::Software.tells_compositor_of_an_update("explorer.exe"));
        assert!(!Desktop::Hardware.tells_compositor_of_an_update("dwm.exe"));
    }

    #[test]
    fn the_display_driver_comes_with_the_hardware_desktop_only() {
        assert!(!Desktop::Software.has_display_driver());
        assert!(Desktop::Hardware.has_display_driver());
    }

    #[test]
    fn each_value_asks_for_its_own_desktop_whatever_the_default() {
        for recorded_card in [false, true] {
            let from = |value: &str| Desktop::from_value(Some(OsStr::new(value)), recorded_card);
            assert_eq!(from("0"), Desktop::Hardware);
            assert_eq!(from("1"), Desktop::Software, "as a lab exported it");
            assert_eq!(from("yes"), Desktop::default_for(recorded_card));
            assert_eq!(Desktop::from_value(None, recorded_card), Desktop::default_for(recorded_card));
        }
        for desktop in [Desktop::Software, Desktop::Hardware] {
            assert_eq!(Desktop::asked(Some(OsStr::new(desktop.value()))), Some(desktop));
        }
    }

    #[test]
    fn a_real_card_gets_the_stages_desktop_and_a_recorded_one_the_software_desktop() {
        assert_eq!(Desktop::default_for(false), STAGE_DESKTOP);
        assert_eq!(STAGE_DESKTOP, Desktop::Hardware, "the hardware desktop is tried by default");
        assert_eq!(Desktop::default_for(true), Desktop::Software);
    }
    fn every_fallback() -> [Fallback; 8] {
        [
            Fallback::AskedFor,
            Fallback::UnknownWord("x".into()),
            Fallback::RecordedCard,
            Fallback::NotXe("i915".into()),
            Fallback::NodeNotOpened("denied".into()),
            Fallback::SelfTestFailed(vec!["vm".into(), "binds".into()]),
            Fallback::NoMemoryRegion,
            Fallback::TooLittleMemory(1 << 20),
        ]
    }

    #[test]
    fn the_hardware_desktop_is_given_where_everything_runs() {
        assert_eq!(Desktop::Hardware.at_stage(Stage::Everything, None, false), (Desktop::Hardware, None));
    }

    #[test]
    fn a_hardware_desktop_at_the_compute_stage_is_the_software_desktop() {
        let (given, why) = Desktop::Hardware.at_stage(Stage::Compute, None, false);
        assert_eq!(given, Desktop::Software);
        assert!(why.unwrap().contains("stage=Compute"));
    }

    #[test]
    fn a_hardware_desktop_left_listening_is_the_software_desktop_with_the_layer_that_refused() {
        for fallback in every_fallback() {
            let (given, why) = Desktop::Hardware.at_stage(Stage::Listen, Some(&fallback), false);
            assert_eq!(given, Desktop::Software, "{fallback}");
            assert_eq!(why.unwrap(), format!("stage=Listen {fallback}"));
        }
        let (_, why) = Desktop::Hardware.at_stage(Stage::Listen, None, false);
        assert!(why.unwrap().contains("layer=switch"), "listening with no reason is the switch's");
    }

    #[test]
    fn the_software_desktop_stays_at_every_stage() {
        for stage in [Stage::Listen, Stage::Compute, Stage::Everything] {
            assert_eq!(Desktop::Software.at_stage(stage, None, false), (Desktop::Software, None));
            assert_eq!(Desktop::Software.at_stage(stage, None, true), (Desktop::Software, None));
        }
    }

    #[test]
    fn the_note_says_when_nothing_runs_on_a_software_desktop_that_was_asked_for() {
        let failed = Fallback::SelfTestFailed(vec!["binds".into()]);
        let note = Desktop::note(Desktop::Software, None, Stage::Listen, Some(&failed)).unwrap();
        assert_eq!(note, format!("{LISTENING_NOTE_START} {failed}"));
        assert_eq!(Desktop::note(Desktop::Software, None, Stage::Compute, None), None, "compute runs");
        assert_eq!(Desktop::note(Desktop::Software, None, Stage::Everything, None), None);
        assert_eq!(Desktop::note(Desktop::Hardware, None, Stage::Listen, None), None, "a lab's kept desktop");
        let (given, why) = Desktop::Hardware.at_stage(Stage::Listen, Some(&failed), false);
        let at_stage = Desktop::note(given, why.clone(), Stage::Listen, Some(&failed));
        assert_eq!(at_stage, why, "the hardware desktop's reason is the note");
        assert!(at_stage.unwrap().starts_with(LISTENING_NOTE_START));
    }

    #[test]
    fn a_lab_can_keep_the_hardware_desktop_on_a_recorded_card_only() {
        let kept = Desktop::Hardware.at_stage(Stage::Listen, Some(&Fallback::RecordedCard), true);
        assert_eq!(kept, (Desktop::Hardware, None));
        let one = Some(OsStr::new("1"));
        assert!(lab_keeps_hardware_desktop(one, true));
        assert!(!lab_keeps_hardware_desktop(one, false), "never on a real card");
        assert!(!lab_keeps_hardware_desktop(Some(OsStr::new("yes")), true));
        assert!(!lab_keeps_hardware_desktop(None, true));
    }

    #[test]
    fn a_lab_can_keep_the_device_on_for_a_recorded_card_only() {
        let one = Some(OsStr::new("1"));
        assert!(lab_keeps_device_on_recorded_card(one, true));
        assert!(!lab_keeps_device_on_recorded_card(one, false), "never on a real card");
        assert!(!lab_keeps_device_on_recorded_card(Some(OsStr::new("yes")), true));
        assert!(!lab_keeps_device_on_recorded_card(None, true));
    }

    #[test]
    fn with_nothing_asked_a_guest_gets_the_furthest_stage_and_one_word_turns_it_off() {
        assert_eq!(Stage::asked(None), (FURTHEST_STAGE, None));
        assert!(FURTHEST_STAGE.is_real());
        assert_eq!(Stage::asked(Some(OsStr::new("0"))), (Stage::Listen, Some(Fallback::AskedFor)));
        assert_eq!(Stage::asked(Some(OsStr::new("listen"))), (Stage::Listen, Some(Fallback::AskedFor)));
        assert_eq!(Stage::asked(Some(OsStr::new("compute"))), (Stage::Compute, None));
        assert_eq!(Stage::asked(Some(OsStr::new("1"))), (FURTHEST_STAGE, None));
    }

    #[test]
    fn a_word_this_build_does_not_know_is_never_read_as_run() {
        for word in ["", "yes", "desktop-2", "on"] {
            let (stage, why) = Stage::asked(Some(OsStr::new(word)));
            assert_eq!(stage, Stage::Listen, "{word}");
            assert_eq!(why, Some(Fallback::UnknownWord(word.into())));
        }
    }

    #[test]
    fn each_stage_runs_what_its_name_says() {
        let (opencl, direct3d_12) = (Some(5), Some(7));
        assert!(!Stage::Listen.runs(opencl) && !Stage::Listen.runs(None));
        assert!(Stage::Compute.runs(opencl));
        assert!(!Stage::Compute.runs(direct3d_12) && !Stage::Compute.runs(None));
        assert!(Stage::Everything.runs(opencl) && Stage::Everything.runs(direct3d_12));
        assert!(Stage::Everything.runs(None));
        assert!(!Stage::Listen.is_real() && Stage::Compute.is_real());
    }

    #[test]
    fn every_fallback_names_the_layer_that_refused() {
        for fallback in every_fallback() {
            assert!(fallback.to_string().starts_with("layer="), "{fallback}");
        }
        assert!(Fallback::NotXe("i915".into()).to_string().contains("i915"));
    }
}
