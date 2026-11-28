//! A machine's settings: one JSON file each, and the rules they must meet whatever the host.

use crate::account;
use crate::context::Context;
use crate::disk_image::backing::BackingFile;
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
pub const SHARE_KEY_FILE: &str = "share.key";
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
    /// The guest may read the shared folder and not change it. Left out of the file when off, so an
    /// older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub share_read_only: bool,
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
    /// The files the disk is built on, nearest first, which a start checks the disk's header against.
    /// None for a disk that stands alone, which is every disk the app makes, and every machine from
    /// before this setting: a disk built on a file by hand has it recorded with `guesthouse-vm set`.
    /// Left out of the file when there are none, so an older build can still read it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backing_files: Vec<BackingFile>,
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
    /// An NVIDIA machine's card holds its memory clock up while a viewer streams the machine's screen
    /// to another device, which takes each picture's encoding from tens of milliseconds to under ten,
    /// at the card's idle power. The user turns it off to save that. Left out of the file until then,
    /// so an older build can still read it.
    #[serde(default, skip_serializing_if = "is_off")]
    pub stream_clocks_off: bool,
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
    /// The data disk of a plain machine that holds Android for VMs, the second of its two. Its system
    /// disk is the machine's own. Left out of the file when empty, so an older build can still read it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub android_data: String,
    /// The card an Android guest draws on, by its PCI address. Empty is the first NVIDIA card, or else
    /// the first AMD or Intel one. Left out of the file when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub android_gpu: String,
    /// Play services through microG, for an Android machine. Its image has microG, off unless this is.
    /// Left out of the file when off.
    #[serde(default, skip_serializing_if = "is_off")]
    pub android_play: bool,
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
            share_read_only: false,
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
            backing_files: Vec::new(),
            window_speed_off: false,
            file_drop_off: false,
            nvapi_wrapper_off: false,
            stream_clocks_off: false,
            disk_folder: String::new(),
            nvme: false,
            opencore: String::new(),
            android_data: String::new(),
            android_gpu: String::new(),
            android_play: false,
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

    /// The key the shared folder is served under, which is part of the address the guest is given.
    pub fn share_key_file(&self, context: &Context) -> PathBuf {
        self.folder(context).join(SHARE_KEY_FILE)
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

    /// Whether the card's memory clock is held up while a viewer streams the screen. Only an NVIDIA
    /// card's can be.
    pub fn stream_clocks(&self) -> bool {
        self.windows_maker() == Some(GpuMaker::Nvidia) && !self.stream_clocks_off
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

    /// Whether this plain machine holds Android.
    pub fn android(&self) -> bool {
        self.kind == Kind::Plain && !self.android_data.is_empty()
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
        if self.android() {
            "Android"
        } else if self.macos() {
            "macOS"
        } else {
            self.kind.name()
        }
    }

    /// The summary, with what's known of the screen.
    pub fn summary_showing(&self, screen: &str) -> String {
        let (kind, cores, ram) = (self.kind.name(), self.cores, self.ram_gib);
        let summary = match self.kind {
            Kind::Plain if self.macos() => {
                format!("macOS, {cores} cores, {ram} GiB, {}, GPU preview", self.screen)
            }
            Kind::Plain if self.android() => {
                format!("Android, {cores} cores, {ram} GiB, {}, GPU preview", self.screen)
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

/// Fields a machine has from how it's made, which settings chosen before it's made don't replace.
const MADE_FIELDS: [&str; 10] =
    ["name", "disk", "vars", "iso", "installing", "kind", "opencore", "android_data", "disk_folder", "user"];

/// What changed between two versions of a machine's settings, field by field: a new value, or `None` for
/// a field gone back to its default, which the file leaves out. Fields a machine gets from how it's made
/// aren't among them.
pub type SettingsChanges = Vec<(String, Option<serde_json::Value>)>;

/// The settings changed from `before` to `after`, as New Machine's draft of a machine's settings gives
/// them, to be applied once the machine is made.
pub fn settings_changes(before: &Machine, after: &Machine) -> SettingsChanges {
    let as_map = |machine: &Machine| match serde_json::to_value(machine) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let (before, after) = (as_map(before), as_map(after));
    let mut changes: SettingsChanges = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, value)| (key.clone(), Some(value.clone())))
        .collect();
    changes.extend(before.keys().filter(|key| !after.contains_key(*key)).map(|key| (key.clone(), None)));
    changes.retain(|(key, _)| !MADE_FIELDS.contains(&key.as_str()));
    changes
}

impl Machine {
    /// This machine with `changes` applied: a field set, or taken back to its default.
    pub fn with_changes(&self, changes: &SettingsChanges) -> Result<Machine> {
        let mut map = match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => return Err(Failure::new("The machine's settings could not be read.")),
        };
        for (key, value) in changes {
            match value {
                Some(value) => map.insert(key.clone(), value.clone()),
                None => map.remove(key),
            };
        }
        let mut changed: Machine = serde_json::from_value(serde_json::Value::Object(map))
            .map_err(|error| Failure(format!("The settings chosen could not be applied: {error}")))?;
        changed.id.clone_from(&self.id);
        Ok(changed)
    }
}

#[cfg(test)]
mod tests;
