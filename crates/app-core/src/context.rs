//! What the code needs from the PC, in one value that a test can fill with stand-ins.

use crate::agent::{GuestAgent, TcpAgent};
use crate::bridge::BridgeHost;
use crate::clock::{Clock, RealClock};
use crate::guest_memory::GuestMemory;
use crate::jailing::Offer;
use crate::monitor::{Monitor, QmpMonitor};
use crate::paths::Paths;
use crate::vm_args::VmPlan;
use guesthouse_daemon::slot::Slot;
use guesthouse_jail::Support;
use rustix::fs::{Access, access};
use std::fs::{self, DirBuilder, Permissions};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PRIVATE_FOLDER: u32 = 0o700;
/// Where pci.ids is kept: hwdata on Fedora and Arch, misc on Debian and Ubuntu, and on NixOS in the
/// system's hwdata, which Guesthouse's module adds.
const PCI_NAMES: [&str; 3] =
    ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/run/current-system/sw/share/hwdata/pci.ids"];

/// Whether this user can open a device to read and write it.
fn may_open_here(device: &Path) -> bool {
    access(device, Access::READ_OK | Access::WRITE_OK).is_ok()
}

/// The first of these that is there, or the first if none is.
fn first_present(paths: &[&str]) -> PathBuf {
    let found = paths.iter().find(|path| Path::new(path).exists());
    PathBuf::from(found.or(paths.first()).copied().unwrap_or_default())
}

/// A folder that already exists is made private too, as the first run could have made it otherwise.
fn make_private_folder(folder: &Path) -> std::io::Result<()> {
    DirBuilder::new().recursive(true).mode(PRIVATE_FOLDER).create(folder)?;
    fs::set_permissions(folder, Permissions::from_mode(PRIVATE_FOLDER))
}

/// The variable that names the slot of a task.
pub const SLOT_VARIABLE: &str = guesthouse_daemon::slot::VARIABLE;

#[derive(Clone)]
pub struct Context {
    pub paths: Paths,
    /// This process's slot. A task has its VM's. The window has the first, and names others.
    pub slot: Slot,
    pub processes: PathBuf,
    pub cpus: PathBuf,
    pub usb_devices: PathBuf,
    pub usb_nodes: PathBuf,
    /// The kernel's block devices, one folder for each disk with its partitions inside.
    pub block_devices: PathBuf,
    /// The names that stay with a drive, as links to wherever the kernel put it this time.
    pub drive_ids: PathBuf,
    pub mount_table: PathBuf,
    pub swaps: PathBuf,
    /// What udev knows of each block device, a file for each by its major and minor numbers.
    pub udev_data: PathBuf,
    pub pci_devices: PathBuf,
    pub pci_names: PathBuf,
    /// The NVIDIA driver's own list of its cards, a folder for each by PCI address.
    pub nvidia_cards: PathBuf,
    /// The DRM class in sysfs, where each render node links to its PCI device.
    pub drm_class: PathBuf,
    pub kvm: PathBuf,
    /// Makes a dma-buf of a guest's memory. An AMD card takes the memory faster that way.
    pub udmabuf: PathBuf,
    /// The largest dma-buf udmabuf makes, in MiB.
    pub udmabuf_limit: PathBuf,
    /// The kernel's security modules that are on, Landlock among them or not.
    pub security_modules: PathBuf,
    pub driver_version_file: PathBuf,
    pub meminfo: PathBuf,
    /// Makes a GPU guest's memory and claims its RAM. A test has no gigabytes to spare.
    pub reserve_memory: fn(&Context, Slot, u64) -> crate::Result<GuestMemory>,
    /// What this PC offers a GPU machine's jail. A test's pretend programs can't be jailed.
    pub jail_offer: fn(&VmPlan) -> Offer,
    /// What this kernel has of the jail's parts, which setup warns about before any start.
    pub jail_support: fn() -> Support,
    /// Whether this user can open a card's device. A test's cards have none.
    pub may_open: fn(&Path) -> bool,
    /// What NVIDIA's driver says of each card's address and memory, as `nvidia-smi`'s lines. A test
    /// says what its cards have.
    pub card_memories: fn() -> String,
    /// Stands in for what this PC can give a GPU machine, which depends on everything else running.
    pub ram_limit: Option<u64>,
    pub bridge: BridgeHost,
    pub monitor: Arc<dyn Monitor>,
    pub agent: Arc<dyn GuestAgent>,
    pub clock: Arc<dyn Clock>,
    /// Replaces the room an install needs, for a test on a small disk.
    pub install_room: Option<u64>,
    /// A recording of an Intel card that stands in for one, for a lab run on a PC without it. Only
    /// a process started with the variable that names it has one: see [`Context::from_process`].
    pub recorded_intel_card: Option<PathBuf>,
}

impl Context {
    pub fn new(paths: Paths, slot: Slot) -> Self {
        Self {
            paths,
            slot,
            processes: "/proc".into(),
            cpus: "/sys/devices/system/cpu".into(),
            usb_devices: "/sys/bus/usb/devices".into(),
            usb_nodes: "/dev/bus/usb".into(),
            block_devices: "/sys/class/block".into(),
            drive_ids: "/dev/disk/by-id".into(),
            mount_table: "/proc/self/mountinfo".into(),
            swaps: "/proc/swaps".into(),
            udev_data: "/run/udev/data".into(),
            pci_devices: "/sys/bus/pci/devices".into(),
            pci_names: first_present(&PCI_NAMES),
            nvidia_cards: "/proc/driver/nvidia/gpus".into(),
            drm_class: "/sys/class/drm".into(),
            kvm: "/dev/kvm".into(),
            udmabuf: "/dev/udmabuf".into(),
            udmabuf_limit: "/sys/module/udmabuf/parameters/size_limit_mb".into(),
            security_modules: "/sys/kernel/security/lsm".into(),
            driver_version_file: "/sys/module/nvidia/version".into(),
            meminfo: "/proc/meminfo".into(),
            reserve_memory: GuestMemory::claim,
            jail_offer: Offer::here,
            jail_support: Support::probe,
            may_open: may_open_here,
            card_memories: crate::setup::cards::ask_card_memories,
            ram_limit: None,
            bridge: BridgeHost::default(),
            monitor: Arc::new(QmpMonitor),
            agent: Arc::new(TcpAgent),
            clock: Arc::new(RealClock::default()),
            install_room: None,
            recorded_intel_card: None,
        }
    }

    /// For this process. A slot that can't be read stops it, as a guess could reach another VM's files.
    pub fn from_process() -> crate::Result<Self> {
        let slot = Slot::from_environment().map_err(|error| crate::Failure::new(error.to_string()))?;
        let recorded_intel_card = guesthouse_backend_intel::recording::recorded_card_file();
        Ok(Self { recorded_intel_card, ..Self::new(Paths::from_process(), slot) })
    }

    pub fn second_instance(&self) -> bool {
        self.paths.second_instance()
    }

    pub fn run_folder(&self, slot: Option<Slot>) -> PathBuf {
        self.paths.run_folder(slot.unwrap_or(self.slot))
    }

    /// Makes a run folder that only this user can open, as its sockets give the run of a VM.
    pub fn make_run_folder(&self, slot: Option<Slot>) -> std::io::Result<()> {
        make_private_folder(&self.paths.runtime)?;
        make_private_folder(&self.run_folder(slot))
    }

    pub fn agent_port(&self, slot: Option<Slot>) -> u16 {
        self.paths.agent_port(slot.unwrap_or(self.slot))
    }

    pub fn install_room_gib(&self) -> u64 {
        self.install_room.unwrap_or(crate::library::INSTALL_ROOM_GIB)
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{Scratch, scratch_context};
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;

    const OTHERS: u32 = 0o077;

    #[test]
    fn a_run_folder_is_private_even_if_it_was_there_already() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fs::create_dir_all(context.run_folder(None)).unwrap();
        fs::set_permissions(&context.paths.runtime, Permissions::from_mode(0o755)).unwrap();
        context.make_run_folder(None).unwrap();
        for folder in [context.paths.runtime.clone(), context.run_folder(None)] {
            assert_eq!(fs::metadata(folder).unwrap().permissions().mode() & OTHERS, 0);
        }
    }

    #[test]
    fn pci_ids_is_found_where_the_distribution_keeps_it() {
        let scratch = Scratch::new();
        let fedora = scratch.path("hwdata/pci.ids");
        let debian = scratch.write("misc/pci.ids", "");
        let places = [fedora.to_str().unwrap(), debian.to_str().unwrap()];
        assert_eq!(super::first_present(&places), debian);
        assert_eq!(super::first_present(&places[..1]), fedora, "none there is the first");
    }
}
