//! A machine's settings: one JSON file each, and the rules they must meet whatever the host.

use crate::account;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::network::{self, Forward, Mode};
use crate::settings::write_json;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

/// The sizes offered, which the display driver has.
pub const SCREENS: [&str; 3] = ["1920x1080", "2560x1440", "3840x2160"];
/// What the basic display can do. 2560x1440 can't work there: Windows takes the largest mode that fits
/// the VGA memory, whose size QEMU rounds up to a power of two, and in 16 MiB the firmware's 2048x2048
/// fits too and is larger.
const BASIC_SCREENS: [&str; 2] = ["1920x1080", "3840x2160"];
/// The rates offered by name, which are the ones in the display driver's settings.
pub const REFRESH_RATES: [u32; 6] = [60, 90, 120, 144, 165, 240];
/// A slower rate only makes the guest's timer easier, and this is where a desktop stops being usable.
pub const MIN_REFRESH_HZ: u32 = 30;
/// The fastest that has been measured. A saved rate above a lowered limit would stop a machine
/// loading, so this starts low and can rise.
pub const MAX_REFRESH_HZ: u32 = 360;
/// Windows 11's own minimum.
pub const MIN_CORES: u32 = 2;
/// Windows 11's own minimum, and above 2.75 GiB, where q35 changes how it splits RAM around the
/// 4 GiB hole. The daemon relies on the larger-RAM split.
pub const MIN_RAM_GIB: u32 = 4;
/// The AHCI controller has six ports. The disk and two discs have the first three, and the rest
/// are for drives.
pub const MAX_DRIVES: usize = 3;
pub const MACHINE_FILE: &str = "machine.json";
const NETWORK_BY_NAME_FILE: &str = "network-by-name";
const OTHER_MAKER: &str =
    "Windows here was installed for the card it has now. For a card from another maker, make a new machine.";
const THUMBNAIL_FILE: &str = "thumbnail.png";
const TOKEN_FILE: &str = "guest.token";
const PASSWORD_FILE: &str = "windows-password";
const LINUX_PASSWORD_FILE: &str = "seed/desk-password";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Has GPU-PV.
    #[default]
    Windows,
    /// Has no GPU, and boots what it's given.
    Plain,
    /// Ubuntu, installed with no hands, with the host's driver forwarded to it.
    Linux,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Windows => "Windows with GPU-PV",
            Self::Plain => "Plain VM",
            Self::Linux => "Ubuntu",
        }
    }
}

fn default_cores() -> u32 {
    8
}

fn default_ram() -> u32 {
    16
}

fn default_screen() -> String {
    SCREENS[0].to_owned()
}

fn default_refresh() -> u32 {
    144
}

fn is_on(setting: &bool) -> bool {
    *setting
}

/// Whether the maker goes without saying. AMD's is what an address alone means. A machine keeps
/// NVIDIA's only for a card named by its address, as [`GpuMaker::recorded_for`] says, so that is
/// written, as is a later maker's.
fn is_implied_by_address(maker: &Option<GpuMaker>) -> bool {
    maker.is_none_or(|maker| maker == GpuMaker::Amd)
}

fn is_off(setting: &bool) -> bool {
    !setting
}

/// Reads a setting that's no longer used, whatever it holds.
fn any_value<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<(), D::Error> {
    serde::de::IgnoredAny::deserialize(deserializer).map(|_| ())
}

fn default_on() -> bool {
    true
}

fn default_bridge() -> String {
    network::DEFAULT_BRIDGE.to_owned()
}

/// One of the host's drives, given to a machine whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drive {
    /// Its name under `/dev/disk/by-id`, as `sdb` and its like change from one boot to the next.
    pub id: String,
}

/// Fields beyond the first three can be missing: older files don't have them, and each default keeps
/// such a machine as it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    #[serde(skip)]
    pub id: String,
    pub name: String,
    pub disk: String,
    pub vars: String,
    #[serde(default = "default_cores")]
    pub cores: u32,
    #[serde(default = "default_ram")]
    pub ram_gib: u32,
    #[serde(default = "default_screen")]
    pub screen: String,
    #[serde(default = "default_refresh")]
    pub refresh_hz: u32,
    /// Wanted, that is: Windows only goes to HDR on a screen that can show it.
    #[serde(default)]
    pub hdr: bool,
    #[serde(default)]
    pub vcache_pin: bool,
    #[serde(default)]
    pub kind: Kind,
    /// What is in the drive of a plain machine.
    #[serde(default)]
    pub iso: String,
    /// Windows hasn't finished installing here.
    #[serde(default)]
    pub installing: bool,
    /// A privacy choice, so it's the user's to turn on.
    #[serde(default)]
    pub clipboard: bool,
    /// The host folder the guest sees as a drive, if any.
    #[serde(default)]
    pub share: String,
    /// The most video memory it may use. 0 is no cap.
    #[serde(default)]
    pub vram_gib: u32,
    /// A virtual TPM and Secure Boot, from when it was installed. Older machines stay without.
    #[serde(default)]
    pub tpm: bool,
    /// The performance overlay in the viewer.
    #[serde(default)]
    pub overlay: bool,
    /// The viewer draws the guest's frames into its window, rather than handing them to the
    /// compositor as they are. The user's choice when the picture goes wrong. Left out of the file
    /// when off, so an older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub direct_frames_off: bool,
    /// The same setting from when it was off unless turned on. Old files have it, and it isn't
    /// used: the frames are handed over unless the setting above says not to.
    #[serde(default, rename = "direct_frames", skip_serializing, deserialize_with = "any_value")]
    pub was_direct_frames: (),
    /// The viewer's bar stays over a window's screen, rather than hiding until the pointer rests at
    /// the top edge. On unless the user hides it. Older builds only wrote it when on, so a file
    /// without it is a machine nobody told to hide its bar. Left out of the file when on.
    #[serde(default = "default_on", skip_serializing_if = "is_on")]
    pub bar_stays: bool,
    /// The app starts this machine when a login starts the app. Left out of the file when off, so
    /// an older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub start_with_pc: bool,
    /// The viewer goes fullscreen when it opens on this machine. Left out of the file when off, so an
    /// older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub opens_fullscreen: bool,
    /// The guest's screen keeps the size the machine's settings give it, rather than taking the size
    /// of the viewer's window. Left out of the file when off, so an older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub window_size_off: bool,
    #[serde(default)]
    pub network_mode: Mode,
    /// NAT through passt, if it's installed. It gives the guest a second adapter.
    #[serde(default)]
    pub passt: bool,
    /// The first NIC's. Empty is QEMU's default, which older machines keep until told otherwise.
    #[serde(default)]
    pub mac: String,
    #[serde(default = "default_bridge")]
    pub bridge: String,
    /// NAT only: the name of one host interface, a VPN's, that all of the guest's traffic goes out
    /// through and nothing else. Empty is off. Left out of the file when empty, so an older build can
    /// still read it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub only_through: String,
    /// The DNS server the guest's names are asked of while `only_through` is on, as an IPv4 address.
    /// Empty finds one. Left out of the file when empty, so an older build can still read it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dns: String,
    #[serde(default)]
    pub forwards: Vec<Forward>,
    /// Ubuntu's GPU, which is a preview. Off, the guest gets the plain screen.
    #[serde(default = "default_on")]
    pub gpu_preview: bool,
    /// A GPU machine may start with no jail on a PC that can't give it one. The user's choice, and
    /// off unless they make it. Left out of the file when off, so an older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub start_without_jail: bool,
    /// The forwarded GPU for a plain machine that holds a Linux the user installed. A preview, and
    /// off unless they turn it on. Left out of the file when off, so an older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub linux_gpu: bool,
    /// Sound for a GPU machine, through PipeWire. Off, the guest's sound card plays nothing and
    /// QEMU has no socket to PipeWire, so its jail can refuse it any socket of its own. Left out of
    /// the file when on, so an older build can still read it.
    #[serde(default = "default_on", skip_serializing_if = "is_on")]
    pub sound: bool,
    /// The PCI address of the card the guest's GPU runs on, which for a Linux guest is an AMD card.
    /// Empty is the NVIDIA card: setup's for a Windows guest, forwarded for a Linux one. A Windows
    /// guest can also be on another NVIDIA card, by its address. Left out of the file when empty, so
    /// an older build can still read it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub amd_gpu: String,
    /// The maker of a Windows guest's card, when its address can't say. An empty `amd_gpu` is setup's
    /// NVIDIA card and an address is AMD's, which is how every machine from before other makers is
    /// read, so those two are never written. An NVIDIA card named by its address is, and so is a maker
    /// that came later, with the card's address in `amd_gpu`. An app from before this field refuses a
    /// file with it, so it can't take such a machine for an AMD one.
    #[serde(default, skip_serializing_if = "is_implied_by_address")]
    pub gpu_maker: Option<GpuMaker>,
    /// The name of the account the install makes, if the user chose one. Empty is the name the guest
    /// scripts have always used, which every machine from before this setting has. Left out of the
    /// file when empty, so an older build can still read it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    /// Drives of this PC the guest has beside its disk. Left out of the file when there are none, so
    /// an older build can still read it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drives: Vec<Drive>,
    /// Windows shows every program that draws in a window without a copy of each frame, not only
    /// the ones it counts as games. The user turns it off when a program's window goes wrong. Left
    /// out of the file until then, so an older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub window_speed_off: bool,
    /// Files can be dropped on its screen and dragged out of it. The user's to turn off. Left out
    /// of the file until then, so an older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub file_drop_off: bool,
    /// An NVIDIA Windows guest keeps NVIDIA's own NVAPI in System32 rather than Guesthouse's wrapper,
    /// which DLSS needs but some games' anti-cheat refuses, as it isn't signed. The user turns it
    /// off for such a game. Left out of the file until then, so an older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub nvapi_wrapper_off: bool,
    /// The folder made for its disk on another drive, which goes when the machine is deleted. Empty
    /// is a disk in the machine's own folder. Left out of the file when empty, so an older build
    /// can still read it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub disk_folder: String,
    /// The disk is on an NVMe controller rather than the emulated SATA one every machine had before.
    /// Windows has its own driver for it, and it's faster. A new Windows machine is made on it, and an
    /// older one moves once Windows has been readied for it. Left out of the file when off, so an
    /// older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub nvme: bool,
    /// The OpenCore image of a plain machine that holds macOS, which it boots through. Its GPU is
    /// reims-vgpu's device. Left out of the file when empty, so an older build can still read it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub opencore: String,
    /// A snapshot before a start that changes Windows: see [`crate::auto_snapshot`]. Nothing here is
    /// what [`crate::auto_snapshot::AUTO_SNAPSHOT_DEFAULT_ON`] says, and the user's choice is written.
    /// Left out of the file until then, so an older build can still read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_snapshot: Option<bool>,
    /// Every start begins from the disk, firmware settings and TPM as they are now, and what a run
    /// writes to them is gone once it ends: see [`crate::throwaway`]. Left out of the file when off.
    /// An app from before this setting refuses a file with it, so it can't write to the clean disk.
    #[serde(default, skip_serializing_if = "is_off")]
    pub throwaway: bool,
}

impl Machine {
    /// A machine with the defaults, as a Windows VM from the scripts would have them.
    pub fn new(id: &str, name: &str, disk: &str, vars: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            disk: disk.to_owned(),
            vars: vars.to_owned(),
            cores: default_cores(),
            ram_gib: default_ram(),
            screen: default_screen(),
            refresh_hz: default_refresh(),
            hdr: false,
            vcache_pin: false,
            kind: Kind::Windows,
            iso: String::new(),
            installing: false,
            clipboard: false,
            share: String::new(),
            vram_gib: 0,
            tpm: false,
            overlay: false,
            direct_frames_off: false,
            was_direct_frames: (),
            bar_stays: true,
            start_with_pc: false,
            opens_fullscreen: false,
            window_size_off: false,
            network_mode: Mode::Nat,
            passt: false,
            mac: String::new(),
            bridge: default_bridge(),
            only_through: String::new(),
            dns: String::new(),
            forwards: Vec::new(),
            gpu_preview: true,
            start_without_jail: false,
            linux_gpu: false,
            sound: true,
            amd_gpu: String::new(),
            gpu_maker: None,
            user: String::new(),
            drives: Vec::new(),
            window_speed_off: false,
            file_drop_off: false,
            nvapi_wrapper_off: false,
            disk_folder: String::new(),
            nvme: false,
            opencore: String::new(),
            auto_snapshot: None,
            throwaway: false,
        }
    }

    pub fn folder(&self, context: &Context) -> PathBuf {
        context.paths.machines.join(&self.id)
    }

    /// Written when an Ubuntu install finishes, which finds the network card by its name. A guest from
    /// before that has its netplan pinned to QEMU's default address, until its scripts have put that right.
    pub fn network_by_name_marker(&self, context: &Context) -> PathBuf {
        self.folder(context).join(NETWORK_BY_NAME_FILE)
    }

    pub fn tpm_folder(&self, context: &Context) -> PathBuf {
        self.folder(context).join("tpm")
    }

    pub fn thumbnail(&self, context: &Context) -> PathBuf {
        self.folder(context).join(THUMBNAIL_FILE)
    }

    /// The token the guest's agent wants. Each machine has its own.
    pub fn token_file(&self, context: &Context) -> PathBuf {
        self.folder(context).join(TOKEN_FILE)
    }

    /// The guest account's password, which the install reads and the app otherwise keeps for whoever
    /// wants it. Each kind of guest has kept its own file since before the user could choose one.
    pub fn password_file(&self, context: &Context) -> PathBuf {
        match self.kind {
            Kind::Linux => self.folder(context).join(LINUX_PASSWORD_FILE),
            Kind::Windows | Kind::Plain => self.folder(context).join(PASSWORD_FILE),
        }
    }

    /// The name of the account the install makes.
    pub fn account(&self) -> &str {
        if self.user.is_empty() { account::default_user(self.kind) } else { &self.user }
    }

    /// Whether the viewer hands the guest's frames to the compositor as they are.
    pub fn direct_frames(&self) -> bool {
        !self.direct_frames_off
    }

    /// Whether the viewer's bar hides while a window shows the screen.
    pub fn bar_hides(&self) -> bool {
        !self.bar_stays
    }

    /// Whether the guest's screen takes the size of the viewer's window. Only Windows on our display
    /// driver can be given a size it didn't start with.
    pub fn sizes_to_window(&self) -> bool {
        self.has_gpu() && !self.window_size_off
    }

    /// Whether the viewer takes dropped files to the guest and brings dragged ones out, which
    /// only Windows' helper can do the guest's half of.
    pub fn file_drop(&self) -> bool {
        self.has_gpu() && !self.file_drop_off
    }

    /// Whether Windows is told to show every program in a window the fast way.
    pub fn window_speed(&self) -> bool {
        !self.window_speed_off
    }

    /// Whether an NVIDIA guest gets the NVAPI wrapper in place of NVIDIA's NVAPI.
    pub fn nvapi_wrapper(&self) -> bool {
        !self.nvapi_wrapper_off
    }

    /// Whether a start of this machine takes a snapshot first when it has a risky change to make.
    pub fn auto_snapshot_on(&self) -> bool {
        self.auto_snapshot.unwrap_or(crate::auto_snapshot::AUTO_SNAPSHOT_DEFAULT_ON)
    }

    pub fn has_gpu(&self) -> bool {
        self.kind == Kind::Windows && !self.installing
    }

    /// Whether this machine should get the forwarded GPU, if the host can offer it.
    pub fn wants_forwarded_gpu(&self) -> bool {
        self.kind == Kind::Linux && !self.installing && self.gpu_preview
    }

    /// Whether this plain machine should get the forwarded GPU, if the host can offer it. Its desktop
    /// goes on the GPU where the guest can put it there, and stays on QEMU's display where it can't.
    pub fn own_linux_gpu(&self) -> bool {
        self.kind == Kind::Plain && self.linux_gpu
    }

    /// Whether this plain machine holds macOS.
    pub fn macos(&self) -> bool {
        self.kind == Kind::Plain && !self.opencore.is_empty()
    }

    /// The AMD card a Linux guest's GPU is to be, by its PCI address, if it has the GPU and it's AMD's.
    pub fn amd_card(&self) -> Option<&str> {
        let linux_gpu = self.wants_forwarded_gpu() || self.own_linux_gpu();
        Some(self.amd_gpu.as_str()).filter(|pci| linux_gpu && !pci.is_empty())
    }

    /// The maker of the card a Windows guest's GPU-PV runs on. None for any other kind of machine.
    pub fn windows_maker(&self) -> Option<GpuMaker> {
        if self.kind != Kind::Windows {
            return None;
        }
        Some(self.gpu_maker.unwrap_or(if self.amd_gpu.is_empty() { GpuMaker::Nvidia } else { GpuMaker::Amd }))
    }

    /// The PCI address of the card a Windows guest's GPU-PV runs on, if it was given one. With none it
    /// runs on the NVIDIA card setup chose.
    pub fn windows_card_address(&self) -> Option<&str> {
        Some(self.amd_gpu.as_str()).filter(|pci| self.kind == Kind::Windows && !pci.is_empty())
    }

    /// Puts a Windows guest on the card at `pci`, which is `maker`'s. Empty is setup's NVIDIA card.
    pub fn use_windows_card(&mut self, maker: GpuMaker, pci: &str) {
        self.amd_gpu = pci.to_owned();
        self.gpu_maker = maker.recorded_for(pci);
    }

    /// Why this machine can't move to a card made by `to`. A Windows guest was installed with its card
    /// maker's driver, so it stays with that maker.
    pub fn card_change_problem(&self, to: GpuMaker) -> Option<&'static str> {
        let same_maker = self.windows_maker().is_none_or(|own| own == to);
        (!same_maker).then_some(OTHER_MAKER)
    }

    /// Whether its disk and firmware file are the app's to delete: both in its folder, or the disk
    /// in the folder made for it on another drive.
    pub fn owns_files(&self, context: &Context) -> bool {
        let own = self.folder(context);
        let disk_home = if self.disk_folder.is_empty() { &own } else { Path::new(&self.disk_folder) };
        // Ubuntu boots with no firmware file of its own.
        is_inside(&self.disk, disk_home) && (self.vars.is_empty() || is_inside(&self.vars, &own))
    }

    /// The size the VM's VGA gets, for the firmware and for Windows if it's on the basic display.
    pub fn basic_screen(&self) -> &str {
        if BASIC_SCREENS.contains(&self.screen.as_str()) { &self.screen } else { BASIC_SCREENS[0] }
    }

    /// What was asked for, which is what's true of a machine that's off.
    fn screen_wanted(&self) -> String {
        let hdr = if self.hdr { ", HDR" } else { "" };
        format!("{}, {} Hz{hdr}", self.screen, self.refresh_hz)
    }

    pub fn summary(&self) -> String {
        self.summary_showing(&self.screen_wanted())
    }

    /// The summary with the screen as known, and a word for a throwaway machine.
    fn with_throwaway(&self, summary: String) -> String {
        if self.throwaway { format!("{summary}, throwaway") } else { summary }
    }

    /// What sort of guest it is, such as "Windows with GPU-PV", which says nothing of whose it is.
    pub fn kind_name(&self) -> &'static str {
        self.kind.name()
    }

    /// The summary, with what's known of the screen.
    pub fn summary_showing(&self, screen: &str) -> String {
        let (kind, cores, ram) = (self.kind.name(), self.cores, self.ram_gib);
        let summary = match self.kind {
            Kind::Plain if self.macos() => {
                format!("macOS, {cores} cores, {ram} GiB, {}, GPU preview", self.screen)
            }
            Kind::Plain if self.linux_gpu => format!("{kind}, {cores} cores, {ram} GiB, Linux GPU preview"),
            Kind::Plain => format!("{kind}, {cores} cores, {ram} GiB, no GPU acceleration"),
            Kind::Linux if self.gpu_preview => format!("{kind}, {cores} cores, {ram} GiB, GPU preview"),
            Kind::Linux => format!("{kind}, {cores} cores, {ram} GiB, no GPU"),
            Kind::Windows => format!("{kind}, {cores} cores, {ram} GiB, {screen}"),
        };
        self.with_throwaway(summary)
    }

    /// Says what is wrong with these settings, whatever the host. `max_vram_gib` is what the card allows.
    pub fn problems(&self, max_vram_gib: u32) -> Vec<String> {
        let mut found = Vec::new();
        if !valid_id(&self.id) {
            found.push("The machine's id can only hold small letters, digits and dashes.".to_owned());
        }
        if self.name.trim().is_empty() {
            found.push("The machine needs a name.".to_owned());
        }
        found.extend(account::name_problem(self.kind, &self.user));
        // Windows has its limits, and the daemon needs its share. A plain machine only needs to boot.
        let (least_cores, least_ram) =
            if self.kind == Kind::Windows { (MIN_CORES, MIN_RAM_GIB) } else { (1, 1) };
        if self.cores < least_cores {
            found.push(format!("Cores must be a whole number, at least {least_cores}."));
        }
        if self.ram_gib < least_ram {
            found.push(format!("RAM must be a whole number of GiB, at least {least_ram}."));
        }
        if !SCREENS.contains(&self.screen.as_str()) {
            found.push(format!("The screen size must be one of {}.", SCREENS.join(", ")));
        }
        if !(MIN_REFRESH_HZ..=MAX_REFRESH_HZ).contains(&self.refresh_hz) {
            found.push(refresh_limits());
        }
        for path in [&self.disk, &self.vars, &self.iso, &self.share] {
            if !path.is_empty() && !Path::new(path).is_absolute() {
                found.push(format!("{path} must be a full path on this PC."));
            }
        }
        found.extend(self.network_problems());
        found.extend(self.drive_problems());
        if self.vram_gib > max_vram_gib {
            found.push(format!("The video memory cap must be a whole number up to {max_vram_gib} GiB."));
        }
        found
    }

    /// What is wrong with the list of drives, whatever the host has.
    fn drive_problems(&self) -> Vec<String> {
        let mut found = Vec::new();
        // Ubuntu's machine line has no AHCI controller to put them on.
        if self.kind == Kind::Linux && !self.drives.is_empty() {
            found.push("Drives are for Windows and plain machines.".to_owned());
        }
        if self.drives.len() > MAX_DRIVES {
            found.push(format!("A machine can have {MAX_DRIVES} drives at most."));
        }
        for (at, drive) in self.drives.iter().enumerate() {
            // The name is joined to a folder, so it must not lead out of it.
            if matches!(drive.id.as_str(), "" | "." | "..") || drive.id.contains('/') {
                found.push("A drive must be one of the names under /dev/disk/by-id.".to_owned());
            } else if self.drives[..at].iter().any(|before| before.id == drive.id) {
                found.push(format!("{} is given twice.", drive.id));
            }
        }
        found
    }

    pub fn network_problems(&self) -> Vec<String> {
        let mut found = network::forward_list_problems(&self.forwards);
        if let Some(problem) = network::bridge_name_problem(&self.bridge) {
            found.push(problem);
        } else if let Some(problem) =
            Some(&self.mac).filter(|mac| !mac.is_empty()).and_then(|mac| network::mac_problem(mac))
        {
            found.push(problem);
        } else if self.network_mode == Mode::Bridged && self.mac.is_empty() {
            found.push("A bridged machine needs its own MAC address.".to_owned());
        }
        found.extend(network::interface_name_problem(&self.only_through));
        found.extend(network::dns_problem(&self.dns));
        found
    }

    /// Replaces the file in one step, so a crash can't leave half of it. Which controller the disk is
    /// on is kept as the file has it: only a start moves the disk, with [`Machine::save_disk_place`],
    /// and the app holds its own copy of each machine from when it opened, which a setting changed
    /// there saves.
    pub fn save(&self, context: &Context) -> Result<()> {
        let mut saving = self.clone();
        if let Some(nvme) = self.saved_nvme(context) {
            saving.nvme = nvme;
        }
        saving.write(context)
    }

    /// Saves the machine with its disk on the controller this copy says.
    pub fn save_disk_place(&self, context: &Context) -> Result<()> {
        self.write(context)
    }

    /// Whether the saved file has the disk on NVMe, or nothing if there's no file to say.
    fn saved_nvme(&self, context: &Context) -> Option<bool> {
        let text = fs::read_to_string(self.folder(context).join(MACHINE_FILE)).ok()?;
        let saved: serde_json::Value = serde_json::from_str(&text).ok()?;
        Some(saved.get("nvme").and_then(serde_json::Value::as_bool).unwrap_or(false))
    }

    fn write(&self, context: &Context) -> Result<()> {
        if let Some(problem) =
            self.problems(crate::settings::max_vram_gib_for(context, self)).into_iter().next()
        {
            return Err(Failure(problem));
        }
        write_json(&self.folder(context).join(MACHINE_FILE), self)
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

pub fn refresh_limits() -> String {
    format!("The refresh rate must be a whole number from {MIN_REFRESH_HZ} to {MAX_REFRESH_HZ} Hz.")
}

/// The rate the user typed.
pub fn read_refresh(text: &str) -> Result<u32> {
    let text = text.trim();
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    text.parse()
        .ok()
        .filter(|hz| digits && (MIN_REFRESH_HZ..=MAX_REFRESH_HZ).contains(hz))
        .ok_or_else(|| Failure(refresh_limits()))
}

/// Whether this file is in this folder, going by where both really are.
fn is_inside(file: &str, folder: &Path) -> bool {
    let Ok(folder) = fs::canonicalize(folder) else { return false };
    let inside = format!("{}{MAIN_SEPARATOR}", folder.display());
    fs::canonicalize(file).is_ok_and(|real| real.to_string_lossy().starts_with(&inside))
}

/// The machine a person named: its id, or the path of its folder in the machines folder, which a
/// person copies from a file manager. Any other path is refused, as the id is the folder's name.
pub fn id_from_argument(context: &Context, given: &str) -> Result<String> {
    if !given.contains('/') {
        return Ok(given.to_owned());
    }
    let path = Path::new(given.trim_end_matches('/'));
    let in_machines = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(context.paths.machines.canonicalize().ok())
        .is_some_and(|(parent, machines)| parent == machines);
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) if in_machines && valid_id(name) => Ok(name.to_owned()),
        _ => Err(Failure(format!(
            "Give the machine's folder name, the last part of {}/NAME, not a path elsewhere.",
            context.paths.machines.display()
        ))),
    }
}

pub fn load(context: &Context, id: &str) -> Result<Machine> {
    let path = context.paths.machines.join(id).join(MACHINE_FILE);
    let could_not_read =
        |error: &dyn std::fmt::Display| Failure(format!("Could not read {}: {error}", path.display()));
    let text = fs::read_to_string(&path).map_err(|error| could_not_read(&error))?;
    let mut machine: Machine = serde_json::from_str(&text).map_err(|error| could_not_read(&error))?;
    machine.id = id.to_owned();
    match machine.problems(crate::settings::max_vram_gib_for(context, &machine)).first() {
        Some(problem) => Err(Failure(format!("{}: {problem}", path.display()))),
        None => Ok(machine),
    }
}

/// The machines, and what is wrong with any file that couldn't be read.
pub fn load_all(context: &Context) -> (Vec<Machine>, Vec<String>) {
    let (mut machines, mut errors) = (Vec::new(), Vec::new());
    for name in sorted_names(&context.paths.machines) {
        // A hidden folder is the app's own work in progress, such as a copy not yet whole.
        if !name.starts_with('.') && context.paths.machines.join(&name).is_dir() {
            match load(context, &name) {
                Ok(machine) => machines.push(machine),
                Err(error) => errors.push(error.0),
            }
        }
    }
    (machines, errors)
}

/// The names in a folder in order, which is none if there is no folder.
pub fn sorted_names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .map(|entries| {
            entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{Forward, Protocol};
    use crate::testing::{Scratch, scratch_context};

    fn sample() -> Machine {
        Machine::new("sample", "Sample", "/d", "/v")
    }

    #[test]
    fn a_plain_machine_with_an_opencore_image_is_a_mac_and_says_so() {
        let plain = Machine { kind: Kind::Plain, ..sample() };
        assert!(!plain.macos() && plain.summary().ends_with("no GPU acceleration"));
        let mac = Machine { opencore: "/m/mac/opencore.qcow2".into(), ..plain };
        assert!(mac.macos());
        assert_eq!(mac.summary(), "macOS, 8 cores, 16 GiB, 1920x1080, GPU preview");
        assert!(!Machine { opencore: "/o".into(), ..sample() }.macos(), "only a plain machine");
    }

    #[test]
    fn a_windows_guest_on_an_amd_card_names_it_and_a_linux_one_does_not_count() {
        assert_eq!(
            (sample().windows_maker(), sample().windows_card_address()),
            (Some(GpuMaker::Nvidia), None)
        );
        let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..sample() };
        assert_eq!(on_amd.windows_maker(), Some(GpuMaker::Amd));
        assert_eq!(on_amd.windows_card_address(), Some("0000:7c:00.0"));
        let linux = Machine { kind: Kind::Linux, ..on_amd };
        assert_eq!((linux.windows_maker(), linux.windows_card_address()), (None, None));
    }

    #[test]
    fn a_windows_guest_moves_only_to_a_card_of_the_same_maker() {
        use GpuMaker::{Amd, Intel, Nvidia};
        let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..sample() };
        let on_intel = Machine { gpu_maker: Some(Intel), ..on_amd.clone() };
        for (machine, own) in [(sample(), Nvidia), (on_amd, Amd), (on_intel, Intel)] {
            for card in GpuMaker::ALL {
                let problem = machine.card_change_problem(card);
                assert_eq!(problem.is_none(), card == own, "{own:?} to {card:?}");
                assert!(problem.is_none_or(|text| text.contains("make a new machine")));
            }
        }
        let linux = Machine { kind: Kind::Linux, ..sample() };
        assert_eq!(linux.card_change_problem(Amd), None, "a Linux guest builds its Mesa again");
    }

    #[test]
    fn a_file_from_before_other_makers_says_nvidia_for_no_address_and_amd_for_one() {
        let nvidia: Machine = serde_json::from_str(&without_maker(&sample())).unwrap();
        assert_eq!(nvidia.windows_maker(), Some(GpuMaker::Nvidia));
        let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..sample() };
        let amd: Machine = serde_json::from_str(&without_maker(&on_amd)).unwrap();
        assert_eq!(
            (amd.windows_maker(), amd.windows_card_address()),
            (Some(GpuMaker::Amd), Some("0000:7c:00.0"))
        );
        let linux = Machine { kind: Kind::Linux, ..on_amd };
        assert_eq!(linux.windows_maker(), None, "only Windows has a GPU-PV card");
    }

    fn without_maker(machine: &Machine) -> String {
        let mut file: serde_json::Value = serde_json::to_value(machine).unwrap();
        file.as_object_mut().unwrap().remove("gpu_maker");
        file.to_string()
    }

    #[test]
    fn nvidia_and_amd_machines_are_written_as_the_released_app_reads_them() {
        let mut machine = sample();
        for (maker, pci) in [(GpuMaker::Nvidia, ""), (GpuMaker::Amd, "0000:7c:00.0")] {
            machine.use_windows_card(maker, pci);
            let text = serde_json::to_string(&machine).unwrap();
            assert!(!text.contains("gpu_maker"), "{maker:?}: {text}");
            assert_eq!(text.contains("amd_gpu"), !pci.is_empty());
            let read: Machine = serde_json::from_str(&text).unwrap();
            assert_eq!(read.windows_maker(), Some(maker));
        }
    }

    #[test]
    fn a_machine_given_an_nvidia_card_by_its_address_writes_its_maker_so_it_is_not_taken_for_amd() {
        let mut machine = sample();
        machine.use_windows_card(GpuMaker::Nvidia, "0000:02:00.0");
        let text = serde_json::to_string(&machine).unwrap();
        assert!(text.contains(r#""gpu_maker":"nvidia""#), "{text}");
        let read: Machine = serde_json::from_str(&text).unwrap();
        assert_eq!(
            (read.windows_maker(), read.windows_card_address()),
            (Some(GpuMaker::Nvidia), Some("0000:02:00.0"))
        );
        machine.use_windows_card(GpuMaker::Nvidia, "");
        assert!(!serde_json::to_string(&machine).unwrap().contains("gpu_maker"), "setup's card, as before");
    }

    #[test]
    fn an_intel_machine_keeps_its_maker_through_a_round_trip_and_an_old_file_reader_refuses_it() {
        let mut machine = sample();
        machine.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
        let text = serde_json::to_string(&machine).unwrap();
        assert!(text.contains(r#""gpu_maker":"intel""#), "{text}");
        let read: Machine = serde_json::from_str(&text).unwrap();
        assert_eq!(
            Machine { id: machine.id.clone(), ..read.clone() },
            machine,
            "the id is the folder's name"
        );
        assert_eq!(read.windows_maker(), Some(GpuMaker::Intel));
        // The released app's Machine rejects fields it doesn't know, which is what keeps it from taking
        // this one for an AMD machine. This build rejects an unknown field the same way.
        let mut unknown: serde_json::Value = serde_json::from_str(&text).unwrap();
        unknown["a_later_field"] = true.into();
        assert!(serde_json::from_value::<Machine>(unknown).is_err());
    }

    #[test]
    fn an_older_build_can_read_a_machine_that_keeps_its_jail() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(!text.contains("start_without_jail"), "{text}");
        let allowed = serde_json::to_string(&Machine { start_without_jail: true, ..sample() }).unwrap();
        let read: Machine = serde_json::from_str(&allowed).unwrap();
        assert!(read.start_without_jail);
    }

    #[test]
    fn a_machine_on_sata_is_one_an_older_build_can_read() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(!text.contains("nvme"), "{text}");
        assert!(!serde_json::from_str::<Machine>(&text).unwrap().nvme);
        let on_nvme = serde_json::to_string(&Machine { nvme: true, ..sample() }).unwrap();
        assert!(serde_json::from_str::<Machine>(&on_nvme).unwrap().nvme);
    }

    #[test]
    fn programs_in_a_window_are_sped_up_unless_turned_off() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(!text.contains("window_speed"), "an older build can't read this: {text}");
        assert!(serde_json::from_str::<Machine>(&text).unwrap().window_speed());
        let turned_off = serde_json::to_string(&Machine { window_speed_off: true, ..sample() }).unwrap();
        assert!(!serde_json::from_str::<Machine>(&turned_off).unwrap().window_speed());
    }

    #[test]
    fn files_can_be_dragged_to_and_from_a_windows_guest_unless_turned_off() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(!text.contains("file_drop"), "an older build can't read this: {text}");
        assert!(serde_json::from_str::<Machine>(&text).unwrap().file_drop());
        let turned_off = serde_json::to_string(&Machine { file_drop_off: true, ..sample() }).unwrap();
        assert!(!serde_json::from_str::<Machine>(&turned_off).unwrap().file_drop());
        assert!(!Machine { kind: Kind::Linux, ..sample() }.file_drop());
        assert!(!Machine { installing: true, ..sample() }.file_drop());
    }

    #[test]
    fn the_nvapi_wrapper_goes_in_unless_turned_off() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(!text.contains("nvapi"), "an older build can't read this: {text}");
        assert!(serde_json::from_str::<Machine>(&text).unwrap().nvapi_wrapper());
        let turned_off = serde_json::to_string(&Machine { nvapi_wrapper_off: true, ..sample() }).unwrap();
        assert!(!serde_json::from_str::<Machine>(&turned_off).unwrap().nvapi_wrapper());
    }

    #[test]
    fn direct_frames_are_on_whatever_an_old_file_says_and_off_only_when_turned_off() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(!text.contains("direct_frames"), "an older build reads this as off: {text}");
        let old_file = text.replacen('{', r#"{"direct_frames":false,"#, 1);
        assert!(serde_json::from_str::<Machine>(&old_file).unwrap().direct_frames());
        let turned_off = serde_json::to_string(&Machine { direct_frames_off: true, ..sample() }).unwrap();
        assert!(!serde_json::from_str::<Machine>(&turned_off).unwrap().direct_frames());
    }

    #[test]
    fn the_bar_stays_unless_the_file_says_it_hides() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(!text.contains("bar_stays"), "the default stays out: {text}");
        assert!(!serde_json::from_str::<Machine>(&text).unwrap().bar_hides());
        let older = r#"{"name": "Old", "disk": "/d", "vars": "/v"}"#;
        assert!(!serde_json::from_str::<Machine>(older).unwrap().bar_hides(), "nobody chose");
        let stayed = text.replacen('{', r#"{"bar_stays":true,"#, 1);
        assert!(!serde_json::from_str::<Machine>(&stayed).unwrap().bar_hides());
        let hides = serde_json::to_string(&Machine { bar_stays: false, ..sample() }).unwrap();
        assert!(hides.contains(r#""bar_stays":false"#), "an older build knows the field: {hides}");
        assert!(serde_json::from_str::<Machine>(&hides).unwrap().bar_hides());
    }

    #[test]
    fn the_screen_sizes_to_the_window_unless_the_file_says_not_to() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(!text.contains("window_size"), "an older build refuses a field it doesn't know: {text}");
        assert!(serde_json::from_str::<Machine>(&text).unwrap().sizes_to_window());
        let off = serde_json::to_string(&Machine { window_size_off: true, ..sample() }).unwrap();
        assert!(!serde_json::from_str::<Machine>(&off).unwrap().sizes_to_window());
        assert!(!Machine { kind: Kind::Plain, ..sample() }.sizes_to_window(), "no driver to ask");
    }

    #[test]
    fn the_viewer_opens_in_a_window_unless_the_file_says_fullscreen() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(!text.contains("opens_fullscreen"), "an older build refuses a field it doesn't know: {text}");
        assert!(!serde_json::from_str::<Machine>(&text).unwrap().opens_fullscreen);
        let on = serde_json::to_string(&Machine { opens_fullscreen: true, ..sample() }).unwrap();
        assert!(serde_json::from_str::<Machine>(&on).unwrap().opens_fullscreen);
    }

    #[test]
    fn sound_is_on_unless_the_file_says_off() {
        assert!(!serde_json::to_string(&sample()).unwrap().contains("sound"), "the default stays out");
        let quiet = serde_json::to_string(&Machine { sound: false, ..sample() }).unwrap();
        assert!(!serde_json::from_str::<Machine>(&quiet).unwrap().sound);
        let older = r#"{"name": "Old", "disk": "/d", "vars": "/v"}"#;
        assert!(serde_json::from_str::<Machine>(older).unwrap().sound);
    }

    #[test]
    fn what_is_saved_loads_the_same() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut machine = sample();
        machine.ram_gib = 24;
        machine.screen = "3840x2160".into();
        machine.refresh_hz = 240;
        machine.vcache_pin = true;
        machine.forwards = vec![Forward::new(Protocol::Tcp, 2222, 22)];
        machine.save(&context).unwrap();
        assert_eq!(load(&context, "sample").unwrap(), machine);
        assert!(!machine.folder(&context).join("machine.json.new").exists());
    }

    #[test]
    fn a_save_from_an_older_copy_keeps_the_disk_where_a_start_put_it() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut older = sample();
        older.save(&context).unwrap();
        Machine { nvme: true, ..older.clone() }.save_disk_place(&context).unwrap();
        older.overlay = !older.overlay;
        older.save(&context).unwrap();
        let saved = load(&context, "sample").unwrap();
        assert!(saved.nvme, "the app's copy from before the move doesn't put the disk back on SATA");
        assert_eq!(saved.overlay, older.overlay, "what it changed is saved");
        Machine { nvme: false, ..saved }.save_disk_place(&context).unwrap();
        Machine { nvme: true, ..older }.save(&context).unwrap();
        assert!(!load(&context, "sample").unwrap().nvme, "nor on NVMe");
    }

    #[test]
    fn a_new_machine_is_saved_with_its_disk_where_it_says() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        Machine { nvme: true, ..sample() }.save(&context).unwrap();
        assert!(load(&context, "sample").unwrap().nvme);
    }

    #[test]
    fn bad_settings_are_refused_and_change_nothing() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let good = sample();
        good.save(&context).unwrap();
        type Change = fn(&mut Machine);
        let cases: [(Change, &str); 10] = [
            (|m| m.cores = 1, "Cores"),
            (|m| m.ram_gib = 3, "RAM"),
            (|m| m.screen = "1024x768".into(), "screen"),
            (|m| m.refresh_hz = MIN_REFRESH_HZ - 1, "refresh"),
            (|m| m.refresh_hz = MAX_REFRESH_HZ + 1, "refresh"),
            (|m| m.name = " ".into(), "name"),
            (|m| m.id = "../x".into(), "id"),
            (|m| m.disk = "json:{}".into(), "full path"),
            (|m| m.iso = "-x".into(), "full path"),
            (|m| m.share = "shared".into(), "full path"),
        ];
        for (change, text) in cases {
            let mut bad = good.clone();
            change(&mut bad);
            let error = bad.save(&context).unwrap_err();
            assert!(error.0.contains(text), "{error}");
        }
        assert_eq!(load(&context, "sample").unwrap(), good);
    }

    #[test]
    fn a_size_only_the_driver_has_boots_at_one_the_basic_display_has() {
        let mut machine = sample();
        machine.screen = "2560x1440".into();
        assert_eq!(machine.basic_screen(), "1920x1080");
        machine.screen = "3840x2160".into();
        assert_eq!(machine.basic_screen(), "3840x2160");
    }

    #[test]
    fn a_broken_file_is_reported_and_not_fatal() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut good = sample();
        good.refresh_hz = 240;
        good.save(&context).unwrap();
        let path = good.folder(&context).join("machine.json");
        let text = fs::read_to_string(&path).unwrap();
        for (broken, message) in [
            ("{".to_owned(), "Could not read"),
            (r#"{"name": 1}"#.to_owned(), "Could not read"),
            (text.replace("240", "1000"), "refresh"),
            (text.replace("name", "nom"), "Could not read"),
        ] {
            fs::write(&path, broken).unwrap();
            assert!(load(&context, "sample").unwrap_err().0.contains(message));
            let (machines, errors) = load_all(&context);
            assert!(machines.is_empty() && errors.len() == 1);
        }
        fs::write(&path, text).unwrap();
        assert_eq!(load_all(&context), (vec![good], vec![]));
    }

    #[test]
    fn any_whole_rate_in_the_limits_is_kept_as_given() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut machine = sample();
        for hz in [MIN_REFRESH_HZ, 60, 75, 165, 175, 180, 240, MAX_REFRESH_HZ] {
            machine.refresh_hz = hz;
            machine.save(&context).unwrap();
            assert_eq!(load(&context, "sample").unwrap().refresh_hz, hz);
            assert!(machine.summary().contains(&format!("{hz} Hz")));
        }
    }

    #[test]
    fn a_typed_rate_is_read_strictly() {
        assert_eq!(read_refresh(" 165 "), Ok(165));
        assert_eq!(read_refresh(&MAX_REFRESH_HZ.to_string()), Ok(MAX_REFRESH_HZ));
        let huge = "9".repeat(5000);
        for text in
            ["", " ", "abc", "16.5", "-144", "+144", "1e2", "0x90", "١٤٤", "144 Hz", "29", "361", &huge]
        {
            assert!(read_refresh(text).unwrap_err().0.contains("from 30 to 360 Hz"), "{text}");
        }
    }

    #[test]
    fn a_machine_from_before_these_settings_stays_as_it_was() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        scratch.write(
            "home/.local/share/guesthouse/machines/old/machine.json",
            r#"{"name":"Old","disk":"/d","vars":"/v","cores":8,"ram_gib":16,"screen":"1920x1080","refresh_hz":144,"vcache_pin":false}"#,
        );
        let old = load(&context, "old").unwrap();
        assert!(old.kind == Kind::Windows && old.has_gpu() && !old.tpm && !old.overlay && !old.clipboard);
        assert_eq!((old.network_mode, old.passt, old.mac.as_str()), (Mode::Nat, false, ""));
        assert_eq!((old.only_through.as_str(), old.dns.as_str()), ("", ""));
        assert_eq!(old.vram_gib, 0);
        assert!(old.forwards.is_empty());
    }

    #[test]
    fn a_bridged_machine_needs_its_own_mac() {
        let mut machine = sample();
        machine.network_mode = Mode::Bridged;
        assert!(machine.problems(0).iter().any(|text| text.contains("own MAC")));
    }

    #[test]
    fn the_account_is_the_scripts_name_unless_the_user_chose_one() {
        assert_eq!(sample().account(), "gpupv");
        assert_eq!(Machine { kind: Kind::Linux, ..sample() }.account(), "desk");
        assert_eq!(Machine { user: "ana".into(), ..sample() }.account(), "ana");
        assert!(!serde_json::to_string(&sample()).unwrap().contains("user"), "the default stays out");
        let named = serde_json::to_string(&Machine { user: "ana".into(), ..sample() }).unwrap();
        assert_eq!(serde_json::from_str::<Machine>(&named).unwrap().user, "ana");
        let problems = Machine { user: "ana/b".into(), ..sample() }.problems(32);
        assert!(problems.iter().any(|problem| problem.contains("user name")), "{problems:?}");
    }

    #[test]
    fn each_kind_keeps_the_password_file_it_always_had() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert!(sample().password_file(&context).ends_with("sample/windows-password"));
        let ubuntu = Machine { kind: Kind::Linux, ..sample() };
        assert!(ubuntu.password_file(&context).ends_with("sample/seed/desk-password"));
    }

    #[test]
    fn a_machines_token_is_in_its_own_folder() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert!(sample().token_file(&context).ends_with("machines/sample/guest.token"));
    }

    #[test]
    fn ubuntu_says_its_gpu_is_a_preview_and_a_machine_can_go_without() {
        let mut machine = Machine { kind: Kind::Linux, ..sample() };
        assert!(machine.summary().starts_with("Ubuntu, ") && machine.summary().contains("GPU preview"));
        assert!(machine.wants_forwarded_gpu() && !machine.has_gpu(), "it has no daemon");
        machine.gpu_preview = false;
        assert!(machine.summary().contains("no GPU") && !machine.wants_forwarded_gpu());
    }

    #[test]
    fn an_install_has_no_forwarded_gpu() {
        let machine = Machine { kind: Kind::Linux, installing: true, ..sample() };
        assert!(!machine.wants_forwarded_gpu());
    }

    #[test]
    fn a_plain_machine_can_ask_for_the_linux_gpu_and_an_older_build_still_reads_one_that_does_not() {
        let plain = Machine { kind: Kind::Plain, ..sample() };
        assert!(!plain.own_linux_gpu());
        assert!(!serde_json::to_string(&plain).unwrap().contains("linux_gpu"), "the default stays out");
        let with_gpu = Machine { linux_gpu: true, ..plain };
        let read: Machine = serde_json::from_str(&serde_json::to_string(&with_gpu).unwrap()).unwrap();
        assert!(read.own_linux_gpu() && !read.wants_forwarded_gpu() && !read.has_gpu());
        assert!(read.summary().starts_with("Plain VM, ") && read.summary().contains("Linux GPU preview"));
    }

    #[test]
    fn an_amd_card_is_only_a_linux_guests_gpu_and_an_older_build_still_reads_a_machine_with_none() {
        let ubuntu = Machine { kind: Kind::Linux, ..sample() };
        assert!(!serde_json::to_string(&ubuntu).unwrap().contains("amd_gpu"), "the default stays out");
        let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..ubuntu };
        let read: Machine = serde_json::from_str(&serde_json::to_string(&on_amd).unwrap()).unwrap();
        assert_eq!(read.amd_card(), Some("0000:7c:00.0"));
        assert_eq!(Machine { gpu_preview: false, ..read.clone() }.amd_card(), None);
        assert_eq!(Machine { kind: Kind::Windows, ..read.clone() }.amd_card(), None);
        let plain = Machine { kind: Kind::Plain, ..read };
        assert_eq!(plain.amd_card(), None, "a plain machine has no Linux GPU until it's turned on");
        assert_eq!(Machine { linux_gpu: true, ..plain }.amd_card(), Some("0000:7c:00.0"));
    }

    #[test]
    fn only_a_plain_machine_takes_the_linux_gpu() {
        for kind in [Kind::Windows, Kind::Linux] {
            assert!(!Machine { kind, linux_gpu: true, ..sample() }.own_linux_gpu(), "{kind:?}");
        }
    }

    #[test]
    fn plain_machines_say_they_have_no_gpu() {
        let mut machine = sample();
        assert!(machine.summary().starts_with("Windows with GPU-PV, "));
        machine.kind = Kind::Plain;
        assert!(machine.summary().starts_with("Plain VM, ") && machine.summary().contains("no GPU"));
    }

    fn drive(id: &str) -> Drive {
        Drive { id: id.to_owned() }
    }

    #[test]
    fn drives_stay_out_of_the_file_when_there_are_none_and_an_older_file_has_none() {
        assert!(!serde_json::to_string(&sample()).unwrap().contains("drives"), "the default stays out");
        let older = r#"{"name": "Old", "disk": "/d", "vars": "/v"}"#;
        assert!(serde_json::from_str::<Machine>(older).unwrap().drives.is_empty());
        let drives = vec![drive("ata-A_1"), drive("ata-B_2")];
        let given = Machine { drives, ..sample() };
        let text = serde_json::to_string(&given).unwrap();
        assert!(text.contains(r#""drives":[{"id":"ata-A_1"},{"id":"ata-B_2"}]"#), "{text}");
        let read = serde_json::from_str::<Machine>(&text).unwrap();
        assert_eq!(read, Machine { id: String::new(), ..given });
    }

    #[test]
    fn a_machine_has_three_drives_at_most_and_none_twice() {
        let three = Machine { drives: vec![drive("a"), drive("b"), drive("c")], ..sample() };
        assert!(three.problems(0).is_empty());
        let four = Machine { drives: vec![drive("a"), drive("b"), drive("c"), drive("d")], ..sample() };
        assert!(four.problems(0).iter().any(|text| text.contains("3 drives at most")));
        let twice = Machine { drives: vec![drive("a"), drive("a")], ..sample() };
        assert!(twice.problems(0).iter().any(|text| text.contains("a is given twice")));
    }

    #[test]
    fn a_drive_cannot_be_named_to_reach_outside_the_folder_of_names() {
        for id in ["", ".", "..", "../sda", "by-id/x", "/dev/sda"] {
            let problems = Machine { drives: vec![drive(id)], ..sample() }.problems(0);
            assert!(problems.iter().any(|text| text.contains("/dev/disk/by-id")), "{id:?}");
        }
    }

    #[test]
    fn ubuntu_has_no_drives_to_give() {
        let ubuntu = Machine { kind: Kind::Linux, drives: vec![drive("a")], ..sample() };
        assert!(ubuntu.problems(0).iter().any(|text| text.contains("Windows and plain machines")));
        assert!(Machine { kind: Kind::Plain, drives: vec![drive("a")], ..sample() }.problems(0).is_empty());
    }

    /// A machine.json as the Intel test build 0.4.1-intel.2 wrote it: it has `gpu_maker`, and none of
    /// the fields that came to main after that build was cut.
    const FROM_INTEL_TEST_BUILD: &str = r#"{
  "name": "Arc",
  "disk": "/home/tester/.local/share/guesthouse/machines/arc/disk.qcow2",
  "vars": "/home/tester/.local/share/guesthouse/machines/arc/OVMF_VARS.fd",
  "cores": 8,
  "ram_gib": 16,
  "screen": "1920x1080",
  "refresh_hz": 144,
  "hdr": false,
  "vcache_pin": false,
  "kind": "windows",
  "iso": "/home/tester/Downloads/Win11.iso",
  "installing": false,
  "clipboard": true,
  "share": "",
  "vram_gib": 0,
  "tpm": true,
  "overlay": false,
  "network_mode": "nat",
  "passt": true,
  "mac": "52:54:00:12:34:56",
  "bridge": "br0",
  "forwards": [],
  "gpu_preview": true,
  "amd_gpu": "0000:03:00.0",
  "gpu_maker": "intel"
}"#;

    /// A machine.json as release 0.6.1 wrote it, on an AMD card with the window speed setting off and
    /// a drive of the host's.
    const FROM_RELEASE_0_6_1: &str = r#"{
  "name": "Radeon",
  "disk": "/home/tester/.local/share/guesthouse/machines/radeon/disk.qcow2",
  "vars": "/home/tester/.local/share/guesthouse/machines/radeon/OVMF_VARS.fd",
  "cores": 6,
  "ram_gib": 12,
  "screen": "2560x1440",
  "refresh_hz": 60,
  "hdr": false,
  "vcache_pin": false,
  "kind": "windows",
  "iso": "/home/tester/Downloads/Win11.iso",
  "installing": false,
  "clipboard": false,
  "share": "",
  "vram_gib": 0,
  "tpm": true,
  "overlay": false,
  "network_mode": "nat",
  "passt": false,
  "mac": "52:54:00:65:43:21",
  "bridge": "br0",
  "forwards": [{"protocol": "tcp", "host": 2222, "guest": 22}],
  "gpu_preview": true,
  "amd_gpu": "0000:7c:00.0",
  "user": "ana",
  "drives": [{"id": "ata-SOME_DISK_SERIAL"}],
  "window_speed_off": true
}"#;

    fn load_text(text: &str, id: &str) -> Result<Machine> {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = context.paths.machines.join(id);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(MACHINE_FILE), text).unwrap();
        load(&context, id)
    }

    #[test]
    fn a_machine_from_the_intel_test_build_loads_on_its_intel_card() {
        let machine = load_text(FROM_INTEL_TEST_BUILD, "arc").unwrap();
        assert_eq!(machine.windows_maker(), Some(GpuMaker::Intel));
        assert_eq!(machine.windows_card_address(), Some("0000:03:00.0"));
        assert!(machine.window_speed() && machine.disk_folder.is_empty() && machine.drives.is_empty());
        assert!(machine.only_through.is_empty() && machine.dns.is_empty());
    }

    #[test]
    fn a_machine_from_release_0_6_1_loads_as_it_was() {
        let machine = load_text(FROM_RELEASE_0_6_1, "radeon").unwrap();
        assert_eq!(machine.windows_maker(), Some(GpuMaker::Amd));
        assert_eq!(machine.gpu_maker, None);
        assert!(!machine.window_speed());
        assert_eq!(machine.drives, [Drive { id: "ata-SOME_DISK_SERIAL".into() }]);
        assert_eq!(machine.forwards.len(), 1);
        assert!(machine.only_through.is_empty() && machine.dns.is_empty());
    }

    #[test]
    fn only_through_and_its_dns_server_are_kept_and_left_out_when_empty() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(!text.contains("only_through") && !text.contains("dns"), "{text}");
        let through = Machine { only_through: "wg0".into(), dns: "10.64.0.1".into(), ..sample() };
        let read: Machine = serde_json::from_str(&serde_json::to_string(&through).unwrap()).unwrap();
        assert_eq!((read.only_through.as_str(), read.dns.as_str()), ("wg0", "10.64.0.1"));
        let bad = Machine { only_through: "wg0,x".into(), dns: "nobody".into(), ..sample() };
        assert_eq!(bad.network_problems().len(), 2);
    }

    #[test]
    fn a_task_takes_a_machine_by_its_id_or_its_folders_path_and_no_other_path() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fs::create_dir_all(context.paths.machines.join("windowsarc")).unwrap();
        let folder = context.paths.machines.join("windowsarc");
        assert_eq!(id_from_argument(&context, "windowsarc").unwrap(), "windowsarc");
        assert_eq!(id_from_argument(&context, folder.to_str().unwrap()).unwrap(), "windowsarc");
        assert_eq!(id_from_argument(&context, &format!("{}/", folder.display())).unwrap(), "windowsarc");
        let elsewhere = id_from_argument(&context, "/tmp/windowsarc").unwrap_err();
        assert!(elsewhere.0.contains("folder name"), "{}", elsewhere.0);
    }
}
