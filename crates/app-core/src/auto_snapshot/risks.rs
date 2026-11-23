//! The changes a start can make that may leave Windows unable to come up, found from the machine's own
//! files before QEMU starts, with no word to the guest. Each is something the start does to a booted
//! guest later, so at this moment it is only known to be due. A change made to a guest that is
//! already running, with no start after it, is not here: there is no moment to snapshot a machine
//! that is on.

use crate::actions::{nvme_pending, will_stage};
use crate::context::Context;
use crate::display::{DriverPackage, read_state as read_display};
use crate::gpu_identity::{machine_maker, read_state as read_identity};
use crate::gpu_maker::GpuMaker;
use crate::machine::Machine;
use crate::settings::read_setup;
use crate::setup::amd::amd_driver_version;
use crate::setup::intel::intel_driver_version;
use crate::updates::VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// The card maker's vendor ID: a driver package Windows is to write, or a first boot that shows it.
    VendorId,
    /// The disk's move to NVMe, or the first boot on it.
    Nvme,
    /// The driver's files and the app's scripts and wrapper, as an update or a new host driver has them.
    GuestFiles,
    /// A new build of the display driver going into Windows.
    DisplayDriver,
}

impl Risk {
    /// For a snapshot's name: "Automatic: before the vendor ID".
    pub fn words(self) -> &'static str {
        match self {
            Self::VendorId => "the vendor ID",
            Self::Nvme => "the NVMe move",
            Self::GuestFiles => "new guest files",
            Self::DisplayDriver => "the display driver",
        }
    }
}

/// One change that is due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub risk: Risk,
    /// Says which change this is, so that a start that has seen it through is not asked for another
    /// snapshot by it. A new build or a new driver is a new change.
    pub signature: String,
}

fn due(risk: Risk, signature: String) -> Pending {
    Pending { risk, signature }
}

/// The version of the driver this machine's card maker has on this PC, whose files a start puts in
/// the guest. A new one is a new change, as a new build of the app is.
fn host_driver_version(context: &Context, machine: &Machine) -> Option<String> {
    match machine_maker(machine) {
        GpuMaker::Nvidia => {
            read_setup(context).get("driver_version").and_then(|version| version.as_str().map(str::to_owned))
        }
        GpuMaker::Amd => amd_driver_version(context),
        GpuMaker::Intel => intel_driver_version(context),
    }
}

/// What this machine's next start will change in Windows. A machine with nothing due costs a few
/// small reads.
pub fn pending(context: &Context, machine: &Machine) -> Vec<Pending> {
    let mut found = Vec::new();
    let identity = read_identity(context, machine);
    if identity.restarts_windows() {
        found.push(due(Risk::VendorId, "vendor-id:restart".to_owned()));
    } else if let Some(script) = will_stage(context, machine) {
        found.push(due(Risk::VendorId, format!("vendor-id:stage:{script}")));
    }
    if let Some(step) = nvme_pending(context, machine) {
        found.push(due(Risk::Nvme, format!("nvme:{step}")));
    }
    if let Some(package) = DriverPackage::find(context) {
        let state = read_display(context, machine);
        if state.package != package.id && state.failed_package != package.id {
            found.push(due(Risk::DisplayDriver, format!("display:{}", package.id)));
        }
    }
    let driver = host_driver_version(context, machine).unwrap_or_default();
    found.push(due(Risk::GuestFiles, format!("files:{VERSION}:{driver}")));
    found
}
