//! What this PC is: its system, its hardware, its GPU drivers, and the programs Guesthouse runs on.
//! Everything is read from files the kernel and the system keep, or asked of a program's version.

use super::command::output_of;
use super::{Collection, Request};
use crate::context::Context;
use crate::gpu_maker::GpuMaker;
use crate::host::total_ram;
use crate::machine::Machine;
use crate::render_cards::{render_card, render_cards};
use crate::report::summary;
use crate::setup::cards::{Card, Support, card_in_use, find_cards, nvidia_card_at};
use crate::setup::toolkit::gtk_version;
use guesthouse_card_check::host::parse_taint_as_open;
use std::env;
use std::fs;
use std::path::Path;

const OS_RELEASE_FILES: [&str; 2] = ["/etc/os-release", "/usr/lib/os-release"];
const KERNEL_FILE: &str = "/proc/sys/kernel/osrelease";
const BOOT_OPTIONS_FILE: &str = "/proc/cmdline";
/// The boot options that bear on a GPU guest, by their start. The rest (which disk to boot, where the
/// swap is, keys and names) are the user's own and are left out.
const BOOT_OPTION_STARTS: [&str; 22] = [
    "quiet",
    "splash",
    "nomodeset",
    "iommu",
    "intel_iommu",
    "amd_iommu",
    "iommu.",
    "pci",
    "vfio",
    "kvm",
    "nvidia",
    "amdgpu",
    "i915",
    "xe.",
    "modprobe.blacklist",
    "video",
    "mitigations",
    "hugepages",
    "default_hugepagesz",
    "transparent_hugepage",
    "pcie_",
    "initcall_blacklist",
];

/// `options` with only the ones that bear on a GPU guest, and how many others were left out.
fn boot_options(options: &str) -> String {
    let (kept, left): (Vec<&str>, Vec<&str>) = options
        .split_whitespace()
        .partition(|option| BOOT_OPTION_STARTS.iter().any(|start| option.starts_with(start)));
    match left.len() {
        0 => kept.join(" "),
        count => format!("{} (and {count} others left out)", kept.join(" ")).trim_start().to_owned(),
    }
}

const CPU_FILE: &str = "/proc/cpuinfo";
const MODULES_FILE: &str = "/proc/modules";
const NVIDIA_TAINT_FILE: &str = "/sys/module/nvidia/taint";
const AMD_MODULE_FOLDER: &str = "/sys/module/amdgpu";
const INTEL_MODULE_FOLDER: &str = "/sys/module/xe";
/// The kernel modules that matter to a GPU guest: the GPU drivers, KVM, and what hands memory around.
const MODULE_PREFIXES: [&str; 7] = ["nvidia", "amdgpu", "i915", "kvm", "vfio", "udmabuf", "drm"];
/// Intel's driver, by its whole name, as a prefix would take in Xen's modules too.
const INTEL_MODULE: &str = "xe";
const VERSION_SECONDS: u32 = 10;
const GIB: u64 = 1 << 30;

/// The facts as `name: value` lines, which the summary takes some of.
struct Facts(Vec<(String, String)>);

impl Facts {
    fn add(&mut self, name: &str, value: impl Into<String>) {
        self.0.push((name.to_owned(), value.into()));
    }

    fn text(&self) -> String {
        self.0.iter().map(|(name, value)| format!("{name}: {value}\n")).collect()
    }
}

pub fn collect(context: &Context, request: &Request, found: &mut Collection) {
    let mut facts = Facts(Vec::new());
    facts.add("Guesthouse", crate::updates::VERSION);
    let installed = if context.paths.guest_files_in_program() { "a release" } else { "a build of the repo" };
    facts.add("Installed as", installed);

    let distro = os_release_name().unwrap_or_else(|| "not known".to_owned());
    let kernel = first_line(KERNEL_FILE).unwrap_or_else(|| "not known".to_owned());
    facts.add("Distribution", &distro);
    facts.add("Kernel", &kernel);
    facts.add("Boot options", boot_options(&first_line(BOOT_OPTIONS_FILE).unwrap_or_default()));
    facts.add("CPU", cpu_description());
    facts.add("Memory", format!("{} GiB", total_ram(context).div_ceil(GIB)));
    found.summary.set(summary::DISTRO, &distro);
    found.summary.set(summary::KERNEL, &kernel);

    for card in find_cards(context) {
        facts.add("GPU", format!("{} at {}, {}", card.name, card.pci, support_word(card.support)));
    }
    for card in [GpuMaker::Amd, GpuMaker::Intel].into_iter().flat_map(|maker| render_cards(context, maker)) {
        facts.add("GPU", format!("{} at {}", card.name, card.pci));
    }
    if let Some(said) = recorded_intel_card(context) {
        facts.add("RECORDED INTEL CARD", &said);
        found.notes.push(format!("This report is from a lab run on a recorded Intel card: {said}"));
    }
    let nvidia = nvidia_driver(context);
    facts.add("NVIDIA driver", nvidia.clone().unwrap_or_else(|| "not loaded".to_owned()));
    let amd_loaded = Path::new(AMD_MODULE_FOLDER).is_dir();
    facts.add("amdgpu", if amd_loaded { "loaded" } else { "not loaded" });
    let intel_loaded = Path::new(INTEL_MODULE_FOLDER).is_dir();
    facts.add("xe", if intel_loaded { "loaded" } else { "not loaded" });
    facts.add("Loaded modules", loaded_modules().join(" "));

    let qemu = context.paths.qemu.to_string_lossy().into_owned();
    facts.add("QEMU", version_of(&qemu, "--version"));
    facts.add("swtpm", version_of("swtpm", "--version"));
    facts.add("GTK", gtk_version().unwrap_or_else(|| "not found".to_owned()));
    for (name, value) in &request.app_facts {
        facts.add(name, value.as_str());
    }
    let session = session();
    facts.add("Session", &session);
    found.summary.set(summary::SESSION, &session);

    let on_card = request.machine.as_ref().and_then(render_node_card);
    let card = match on_card {
        Some((maker, pci)) => render_card(context, maker, pci).map(|card| card.name),
        None => nvidia_card_of(context, request.machine.as_ref()).map(|card| card.name),
    };
    found.summary.set(summary::CARD, card.as_deref().unwrap_or_default());
    let driver = match on_card {
        Some((GpuMaker::Amd, _)) => format!("amdgpu, kernel {kernel}"),
        Some((GpuMaker::Intel, _)) => format!("xe, kernel {kernel}"),
        Some((GpuMaker::Nvidia, _)) | None => nvidia.unwrap_or_default(),
    };
    found.summary.set(summary::DRIVER, &driver);
    found.add("host/facts.txt", facts.text());
}

/// What a recorded Intel card is, when one stands in for a card: whose, and whether it reads.
fn recorded_intel_card(context: &Context) -> Option<String> {
    let file = context.recorded_intel_card.as_ref()?;
    let read = fs::read_to_string(file).map_err(|error| error.to_string());
    let card = read.and_then(|text| guesthouse_backend_intel::recording::read(&text).map_err(|bad| bad.0));
    Some(match card {
        Ok(card) => format!(
            "{} is a recording of the Intel card {:04x} at {}, and no such card is in this PC",
            file.display(),
            card.pci.device,
            card.address.as_deref().unwrap_or("no address")
        ),
        Err(why) => format!("{} was named as one, and can't be used: {why}", file.display()),
    })
}

/// The NVIDIA card a report is about: the one its machine was given, or setup's.
fn nvidia_card_of(context: &Context, machine: Option<&Machine>) -> Option<Card> {
    let given = machine.filter(|machine| machine.windows_maker() == Some(GpuMaker::Nvidia));
    match given.and_then(Machine::windows_card_address) {
        Some(pci) => nvidia_card_at(context, pci),
        None => card_in_use(context),
    }
}

/// The maker and address of the card the machine reaches through a render node: a Linux guest's AMD
/// card, or a Windows guest's AMD or Intel card.
fn render_node_card(machine: &Machine) -> Option<(GpuMaker, &str)> {
    if let Some(pci) = machine.amd_card() {
        return Some((GpuMaker::Amd, pci));
    }
    let maker = machine.windows_maker().filter(|maker| *maker != GpuMaker::Nvidia)?;
    Some((maker, machine.windows_card_address()?))
}

fn support_word(support: Support) -> &'static str {
    match support {
        Support::Tested => "tested",
        Support::Experimental => "experimental",
        Support::Unsupported => "unsupported",
    }
}

fn first_line(path: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    Some(text.lines().next()?.trim().to_owned())
}

fn os_release_name() -> Option<String> {
    let text = OS_RELEASE_FILES.iter().find_map(|path| fs::read_to_string(path).ok())?;
    let value = text.lines().find_map(|line| line.strip_prefix("PRETTY_NAME="))?;
    Some(value.trim_matches('"').to_owned())
}

fn cpu_description() -> String {
    let text = fs::read_to_string(CPU_FILE).unwrap_or_default();
    let model = text
        .lines()
        .find_map(|line| line.strip_prefix("model name").and_then(|rest| rest.split_once(':')))
        .map_or("not known", |(_, name)| name.trim());
    let threads = text.lines().filter(|line| line.starts_with("processor")).count();
    format!("{model}, {threads} threads")
}

/// The version and whether it's the open module, which is the only one Guesthouse runs on.
fn nvidia_driver(context: &Context) -> Option<String> {
    let version = fs::read_to_string(&context.driver_version_file).ok()?;
    let module = match fs::read_to_string(NVIDIA_TAINT_FILE) {
        Ok(taint) if parse_taint_as_open(&taint) => "open kernel module",
        Ok(_) => "proprietary kernel module",
        Err(_) => "kernel module of a kind that can't be told",
    };
    Some(format!("{}, {module}", version.trim()))
}

fn loaded_modules() -> Vec<String> {
    let text = fs::read_to_string(MODULES_FILE).unwrap_or_default();
    let mut names: Vec<String> = text
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name == INTEL_MODULE || MODULE_PREFIXES.iter().any(|prefix| name.starts_with(prefix)))
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

fn version_of(program: &str, option: &str) -> String {
    match output_of(program, &[option], VERSION_SECONDS) {
        Ok(said) => said.lines().next().unwrap_or("said nothing").trim().to_owned(),
        Err(reason) => reason,
    }
}

/// Wayland or X11, and the desktop, as the session says.
fn session() -> String {
    let variable = |name: &str| env::var(name).ok().filter(|value| !value.is_empty());
    let kind = variable("XDG_SESSION_TYPE")
        .or_else(|| variable("WAYLAND_DISPLAY").map(|_| "wayland".to_owned()))
        .or_else(|| variable("DISPLAY").map(|_| "x11".to_owned()))
        .unwrap_or_else(|| "no desktop session".to_owned());
    match variable("XDG_CURRENT_DESKTOP") {
        Some(desktop) => format!("{kind}, {desktop}"),
        None => kind,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_boot_options_kept_are_the_ones_that_bear_on_a_gpu_guest() {
        let line = "BOOT_IMAGE=/vmlinuz-6.1 root=UUID=1234 resume=/dev/mapper/quillon-swap quiet intel_iommu=on \
                    iommu=pt vfio-pci.ids=10de:2684 cryptdevice=PARTLABEL=quillon:root";
        assert_eq!(
            boot_options(line),
            "quiet intel_iommu=on iommu=pt vfio-pci.ids=10de:2684 (and 4 others left out)"
        );
        assert_eq!(boot_options("quiet splash"), "quiet splash");
        assert_eq!(boot_options("root=/dev/sda1"), "(and 1 others left out)");
        assert_eq!(boot_options(""), "");
    }

    use super::*;
    use crate::machine::Kind;

    #[test]
    fn a_recorded_card_is_said_and_one_that_is_not_named_is_not() {
        let scratch = crate::testing::Scratch::new();
        let mut context = crate::testing::scratch_context(&scratch);
        assert_eq!(recorded_intel_card(&context), None);
        context.recorded_intel_card = Some(scratch.write("b580.card", crate::render_cards::tests::ARC_B580));
        let said = recorded_intel_card(&context).unwrap();
        assert!(said.contains("recording of the Intel card e20b at 0000:03:00.0"), "{said}");
        context.recorded_intel_card = Some(scratch.path("nowhere.card"));
        assert!(recorded_intel_card(&context).unwrap().contains("can't be used"));
    }

    #[test]
    fn the_card_a_report_names_is_the_machines_by_its_maker() {
        let mut windows = Machine::new("w", "W", "/d", "/v");
        assert_eq!(render_node_card(&windows), None, "NVIDIA's card is setup's, not a render node");
        windows.use_windows_card(GpuMaker::Amd, "0000:7c:00.0");
        assert_eq!(render_node_card(&windows), Some((GpuMaker::Amd, "0000:7c:00.0")));
        windows.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
        assert_eq!(render_node_card(&windows), Some((GpuMaker::Intel, "0000:00:02.0")));
        let linux = Machine {
            kind: Kind::Linux,
            amd_gpu: "0000:7c:00.0".into(),
            ..Machine::new("u", "U", "/d", "/v")
        };
        assert_eq!(render_node_card(&linux), Some((GpuMaker::Amd, "0000:7c:00.0")));
    }
}
