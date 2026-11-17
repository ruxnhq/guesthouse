//! What this PC is: its system, its hardware, its GPU drivers, and the programs Guesthouse runs on.
//! Everything is read from files the kernel and the system keep, or asked of a program's version.

use super::command::output_of;
use super::{Collection, Request};
use crate::context::Context;
use crate::host::total_ram;
use crate::linux::amd::amd_cards;
use crate::report::summary;
use crate::setup::cards::{Support, card_in_use, find_cards};
use crate::setup::toolkit::gtk_version;
use guesthouse_card_check::host::parse_taint_as_open;
use std::env;
use std::fs;
use std::path::Path;

const OS_RELEASE_FILES: [&str; 2] = ["/etc/os-release", "/usr/lib/os-release"];
const KERNEL_FILE: &str = "/proc/sys/kernel/osrelease";
const BOOT_OPTIONS_FILE: &str = "/proc/cmdline";
const CPU_FILE: &str = "/proc/cpuinfo";
const MODULES_FILE: &str = "/proc/modules";
const NVIDIA_TAINT_FILE: &str = "/sys/module/nvidia/taint";
const AMD_MODULE_FOLDER: &str = "/sys/module/amdgpu";
/// The kernel modules that matter to a GPU guest: the GPU drivers, KVM, and what hands memory around.
const MODULE_PREFIXES: [&str; 6] = ["nvidia", "amdgpu", "kvm", "vfio", "udmabuf", "drm"];
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
    facts.add("Boot options", first_line(BOOT_OPTIONS_FILE).unwrap_or_default());
    facts.add("CPU", cpu_description());
    facts.add("Memory", format!("{} GiB", total_ram(context).div_ceil(GIB)));
    found.summary.set(summary::DISTRO, &distro);
    found.summary.set(summary::KERNEL, &kernel);

    for card in find_cards(context) {
        facts.add("GPU", format!("{} at {}, {}", card.name, card.pci, support_word(card.support)));
    }
    for card in amd_cards(context) {
        facts.add("GPU", format!("{} at {}", card.name, card.pci));
    }
    let nvidia = nvidia_driver(context);
    facts.add("NVIDIA driver", nvidia.clone().unwrap_or_else(|| "not loaded".to_owned()));
    let amd_loaded = Path::new(AMD_MODULE_FOLDER).is_dir();
    facts.add("amdgpu", if amd_loaded { "loaded" } else { "not loaded" });
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

    let on_amd =
        request.machine.as_ref().and_then(|machine| machine.amd_card().or(machine.windows_amd_card()));
    let card = match on_amd {
        Some(pci) => amd_cards(context).into_iter().find(|card| card.pci == pci).map(|card| card.name),
        None => card_in_use(context).map(|card| card.name),
    };
    found.summary.set(summary::CARD, card.as_deref().unwrap_or_default());
    let driver = match on_amd {
        Some(_) => format!("amdgpu, kernel {kernel}"),
        None => nvidia.unwrap_or_default(),
    };
    found.summary.set(summary::DRIVER, &driver);
    found.add("host/facts.txt", facts.text());
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
        .filter(|name| MODULE_PREFIXES.iter().any(|prefix| name.starts_with(prefix)))
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
