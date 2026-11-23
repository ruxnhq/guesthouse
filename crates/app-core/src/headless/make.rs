//! Making a machine, installing it, and taking it away: what the New Machine dialog, Start Over and
//! the machine's Delete do.

use super::args::NewOptions;
use super::change::on_or_off;
use super::recipes;
use super::{follow_up, run_task};
use crate::card_choice::{choice_at, default_choice, windows_card_choices};
use crate::context::Context;
use crate::defaults::{NO_GPU, account_name, sizes_for_disc};
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::gpu_support::{kind_offered, plain_reason, read_disc, ubuntu_note};
use crate::host::{RamField, physical_cores};
use crate::iso::Iso;
use crate::library::{
    Extras, NewMachine, create_with, delete, folders_text, refuse_before_its_driver, remove_entry,
};
use crate::machine::{Kind, MIN_RAM_GIB, Machine, load};
use crate::macos_setup::{MACOS_SIZES, MACOS_TITLE, NewMac, create_macos, how_it_installs};
use crate::remote_access::{Access, Service};
use crate::render_cards::render_card;
use crate::setup::amd_driver::{read_amd_installer, unpack_amd};
use crate::setup::checks::machine_gpu_warning;
use crate::setup::driver::Extraction;
use crate::setup::intel_driver::{read_intel_installer, unpack_intel};
use crate::tasks::{State, status};
use std::cell::{Cell, RefCell};
use std::io::Write;

/// The copy says how far it is at each step of this many percent.
const PERCENT_STEP: u32 = 10;

/// What `--kind` calls a macOS machine, which is a plain one with a boot image of its own.
const MACOS_KIND: &str = "macos";

fn kind_text(kind: Kind) -> &'static str {
    match kind {
        Kind::Windows => "Windows with GPU-PV",
        Kind::Plain => "a plain VM",
        Kind::Linux => "Ubuntu",
    }
}

fn read_kind(word: &str) -> Result<Kind> {
    match word.to_lowercase().as_str() {
        "windows" => Ok(Kind::Windows),
        "plain" => Ok(Kind::Plain),
        "ubuntu" => Ok(Kind::Linux),
        other => Err(Failure(format!("The kind is windows, ubuntu or plain, not {other}."))),
    }
}

fn whole_number(option: &str, text: &str) -> Result<u32> {
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    text.parse().ok().filter(|_| digits).ok_or_else(|| Failure(format!("{option} must be a whole number.")))
}

/// What the disc becomes here, and what the dialog says of it.
fn disc_offered(context: &Context, path: &str, wanted: Option<&str>, out: &mut dyn Write) -> Result<Iso> {
    let found = read_disc(context, path)?;
    let kind = kind_offered(context, found.kind);
    let reason = plain_reason(context, found.kind);
    let verdict = match kind {
        Kind::Windows => format!("{}: {}.", found.title, kind_text(kind)),
        Kind::Linux => format!("{}: {} {}", found.title, kind_text(kind), ubuntu_note(context)),
        Kind::Plain => {
            format!("{}: {}. You install it by hand. {NO_GPU} {reason}", found.title, kind_text(kind))
        }
    };
    writeln!(out, "{}", verdict.trim())?;
    if let Some(word) = wanted {
        let asked = read_kind(word)?;
        if asked != kind {
            return Err(Failure(format!(
                "This disc makes {} here, and not {}.",
                kind_text(kind),
                kind_text(asked)
            )));
        }
    }
    Ok(Iso { kind, ..found })
}

/// The card a Windows machine runs on, as the dialog would choose it unless one was named, with its
/// maker. No choice at all is the NVIDIA card. Setup's NVIDIA card named by its address is kept as no
/// address, as the dialog keeps it.
fn card_for(context: &Context, named: Option<&str>) -> Result<(String, GpuMaker)> {
    let choices = windows_card_choices(context);
    let Some(pci) = named else {
        return Ok(choices
            .get(default_choice(context, &choices))
            .map_or_else(|| (String::new(), GpuMaker::Nvidia), |choice| (choice.pci.clone(), choice.maker)));
    };
    if let Some(choice) = choice_at(&choices, pci) {
        return Ok((choice.pci.clone(), choice.maker));
    }
    let cards: Vec<&str> = choices.iter().map(|choice| choice.label.as_str()).collect();
    Err(Failure(format!("There is no card at {pci} to use. The cards are {}.", cards.join(", "))))
}

/// Takes the card's driver out of AMD's installer, as the dialog's driver row does, saying how far
/// it is.
fn unpack_amd_driver(context: &Context, pci: &str, installer: &str, out: &mut dyn Write) -> Result<()> {
    let card = render_card(context, GpuMaker::Amd, pci)
        .ok_or_else(|| Failure(format!("There is no AMD card at {pci} to unpack AMD's driver for.")))?;
    writeln!(out, "Reading AMD's installer")?;
    let package = read_amd_installer(installer, card.device)?;
    writeln!(out, "Taking out the driver for {}", card.name)?;
    let (said, out) = (Cell::new(0), RefCell::new(out));
    let folder = unpack_amd(&Extraction::new(installer), context, &package, &|percent| {
        let step = percent / PERCENT_STEP;
        if step > said.get() {
            said.set(step);
            let _ = writeln!(out.borrow_mut(), "{percent}%");
        }
    })?;
    Ok(writeln!(out.borrow_mut(), "AMD's driver {} is in {}.", package.version, folder.display())?)
}

/// Takes the card's drivers out of Intel's installer, as the dialog's driver row does, saying how
/// far it is.
fn unpack_intel_driver(context: &Context, pci: &str, installer: &str, out: &mut dyn Write) -> Result<()> {
    let card = render_card(context, GpuMaker::Intel, pci)
        .ok_or_else(|| Failure(format!("There is no Intel card at {pci} to unpack Intel's driver for.")))?;
    writeln!(out, "Reading Intel's installer")?;
    let package = read_intel_installer(installer, card.device)?;
    writeln!(out, "Taking out the driver for {}", card.name)?;
    let (said, out) = (Cell::new(0), RefCell::new(out));
    let folder = unpack_intel(&Extraction::new(installer), context, &package, &|percent| {
        let step = percent / PERCENT_STEP;
        if step > said.get() {
            said.set(step);
            let _ = writeln!(out.borrow_mut(), "{percent}%");
        }
    })?;
    Ok(writeln!(out.borrow_mut(), "Intel's driver {} is in {}.", package.version, folder.display())?)
}

/// Remote Desktop and SSH as the dialog starts them, unless told otherwise. Only Windows has them.
fn access_for(windows: bool, options: &NewOptions) -> Result<Access> {
    let mut access = Access::NEW;
    for (service, word) in [(Service::RemoteDesktop, options.remote_desktop), (Service::Ssh, options.ssh)] {
        let Some(word) = word else { continue };
        let option = format!("--{}", service.key());
        if !windows {
            return Err(Failure(format!("{option} is only for a Windows machine with GPU-PV.")));
        }
        access.set(service, on_or_off(&option, word)?);
    }
    Ok(access)
}

/// Lean Windows as the recipe starts it, unless told otherwise. What it takes out is shown in full
/// before the machine is made: by the recipe's own text when the recipe turned it on, and here when
/// `--lean on` did.
fn lean_for(windows: bool, options: &NewOptions, extras: &Extras, out: &mut dyn Write) -> Result<bool> {
    let from_recipe = extras.recipe.as_ref().is_some_and(|chosen| chosen.recipe.lean);
    let Some(word) = options.lean else { return Ok(from_recipe) };
    if !windows {
        return Err(Failure::new("--lean is only for a Windows machine with GPU-PV."));
    }
    let lean = on_or_off("--lean", word)?;
    if lean && !from_recipe {
        writeln!(out, "{}", crate::lean::what_it_does())?;
    }
    Ok(lean)
}

/// The folders a recipe names for Defender to leave out, only when `--skip-scanning on` says so, as
/// nothing else turns it on. What it means is printed first.
fn skipped_folders(options: &NewOptions, extras: &Extras, out: &mut dyn Write) -> Result<Vec<String>> {
    let Some(word) = options.skip_scanning else { return Ok(Vec::new()) };
    let named =
        extras.recipe.as_ref().map(|chosen| chosen.recipe.scan_exclusions.clone()).unwrap_or_default();
    if named.is_empty() {
        return Err(Failure::new(
            "--skip-scanning goes with a Windows recipe that names folders to leave out.",
        ));
    }
    if !on_or_off("--skip-scanning", word)? {
        return Ok(Vec::new());
    }
    writeln!(out, "{}", crate::scan_exclusions::what_it_does(&named))?;
    Ok(named)
}

fn access_text(access: &Access) -> String {
    let on: Vec<&str> =
        Service::ALL.iter().filter(|service| access.is_on(**service)).map(|service| service.name()).collect();
    match on.as_slice() {
        [] => "Remote Desktop and SSH are off.".to_owned(),
        [one] => format!("{one} is on, and a port forward reaches it."),
        _ => "Remote Desktop and SSH are on, and a port forward reaches each.".to_owned(),
    }
}

/// A macOS machine has no disc: its installer comes from Apple at its install.
fn new_mac(context: &Context, name: &str, options: &NewOptions, out: &mut dyn Write) -> Result<()> {
    let others = [
        options.iso,
        options.user,
        options.amd_card,
        options.amd_driver,
        options.intel_driver,
        options.remote_desktop,
        options.ssh,
        options.set_up_for,
        options.lean,
        options.skip_scanning,
    ];
    if others.iter().any(Option::is_some) {
        return Err(Failure::new("A macOS machine takes --disk, --cores and --ram, and nothing else."));
    }
    let number = |option: &str, text: Option<&str>, usual: u32| match text {
        Some(text) => whole_number(option, text),
        None => Ok(usual),
    };
    let ram = RamField::for_plain(context, 1, MACOS_SIZES.ram).value as u32;
    let new = NewMac {
        name: name.to_owned(),
        cores: number("--cores", options.cores, MACOS_SIZES.cores.min(physical_cores(&context.cpus)))?,
        ram_gib: number("--ram", options.ram, ram)?,
        disk_gib: number("--disk", options.disk, MACOS_SIZES.disk)?,
    };
    let machine = create_macos(context, &new)?;
    writeln!(out, "Made {} for {MACOS_TITLE} in {}.", machine.name, machine.folder(context).display())?;
    writeln!(out, "{}", how_it_installs(context))?;
    Ok(writeln!(out, "Run: guesthouse-vm install {}", machine.id)?)
}

pub(super) fn new_machine(
    context: &Context,
    name: &str,
    options: &NewOptions,
    out: &mut dyn Write,
) -> Result<()> {
    if options.kind.is_some_and(|kind| kind.eq_ignore_ascii_case(MACOS_KIND)) {
        return new_mac(context, name, options, out);
    }
    let iso = options.iso.ok_or_else(|| Failure::new("new needs --iso PATH, the disc to install from."))?;
    let disc = disc_offered(context, iso, options.kind, out)?;
    let kind = disc.kind;
    let windows = kind == Kind::Windows;
    if kind == Kind::Plain && options.user.is_some() {
        return Err(Failure::new("A plain VM has no account to name. It is installed by hand."));
    }
    if !windows && options.amd_card.is_some() {
        return Err(Failure::new("Only a Windows machine chooses its card here."));
    }
    // An Intel card is told from an AMD one by its maker, as both are named by address.
    let card = windows.then(|| card_for(context, options.amd_card)).transpose()?;
    // Both are judged before either is unpacked, so naming the two doesn't unpack one and then refuse the other.
    if options.amd_driver.is_some() && !matches!(card, Some((_, GpuMaker::Amd))) {
        return Err(Failure::new("--amd-driver is for a Windows machine on an AMD card."));
    }
    if options.intel_driver.is_some() && !matches!(card, Some((_, GpuMaker::Intel))) {
        return Err(Failure::new("--intel-driver is for a Windows machine on an Intel card."));
    }
    let sizes = sizes_for_disc(&disc);
    let field = if windows {
        RamField::for_gpu(context, context.slot, MIN_RAM_GIB.into(), sizes.ram)
    } else {
        RamField::for_plain(context, 1, sizes.ram)
    };
    let number = |option: &str, text: Option<&str>, usual: u32| match text {
        Some(text) => whole_number(option, text),
        None => Ok(usual),
    };
    // What the recipe asks of the sizes where this PC allows it, then what was typed over that.
    let mut usual = recipes::usual_of(&sizes, physical_cores(&context.cpus), field.value as u32);
    let mut extras = Extras::default();
    if let Some(wanted) = options.set_up_for {
        let chosen = recipes::pick(context, kind, wanted, options.yes, out)?;
        (usual, extras) = recipes::sized(context, kind, &card, options, &chosen, usual, out)?;
    } else if options.yes {
        return Err(Failure::new("--yes goes with --set-up-for."));
    }
    extras.lean = lean_for(windows, options, &extras, out)?;
    extras.scan_exclusions = skipped_folders(options, &extras, out)?;
    let new = NewMachine {
        name: name.to_owned(),
        iso: iso.to_owned(),
        kind,
        windows_disc: disc.windows_disc,
        cores: number("--cores", options.cores, usual.cores)?,
        ram_gib: number("--ram", options.ram, usual.ram_gib)?,
        disk_gib: number("--disk", options.disk, usual.disk_gib)?,
        refresh_hz: None,
        user: account_name(kind, options.user.unwrap_or_default().trim()),
        password: String::new(),
        amd_gpu: card.as_ref().map(|(pci, _)| pci.clone()),
        disk_place: options.disk_folder.unwrap_or_default().to_owned(),
        gpu_maker: card.as_ref().map(|(_, maker)| *maker),
        access: access_for(windows, options)?,
    };
    if new.name.trim().is_empty() {
        return Err(Failure::new("The machine needs a name."));
    }
    if let Some((pci, _)) = &card
        && (options.amd_driver.is_some() || options.intel_driver.is_some())
    {
        // Whatever else stops the machine is said before the driver is unpacked, not after.
        refuse_before_its_driver(context, &new)?;
        if let Some(installer) = options.amd_driver {
            unpack_amd_driver(context, pci, installer, out)?;
        }
        if let Some(installer) = options.intel_driver {
            unpack_intel_driver(context, pci, installer, out)?;
        }
    }
    let machine = create_with(context, &new, &extras)?;
    writeln!(out, "Made {} in {}.", machine.name, machine.folder(context).display())?;
    if !machine.disk_folder.is_empty() {
        writeln!(out, "Its disk is in {}.", machine.disk_folder)?;
    }
    if windows {
        writeln!(out, "{}", access_text(&new.access))?;
    }
    if extras.lean {
        writeln!(out, "It is lean Windows.")?;
    }
    if !extras.scan_exclusions.is_empty() {
        writeln!(out, "Defender leaves out {}.", extras.scan_exclusions.join(", "))?;
    }
    if let Some(chosen) = &extras.recipe {
        writeln!(out, "Its steps run once it is installed and up: {}.", chosen.recipe.name)?;
    }
    if machine.installing {
        let who = if windows { "Windows" } else { "Ubuntu" };
        writeln!(out, "{who} installs by itself. Run: guesthouse-vm install {}", machine.id)?;
    } else {
        writeln!(out, "Start it with: guesthouse-vm start {}", machine.id)?;
    }
    Ok(())
}

pub(super) fn install(context: &Context, machine: &Machine, wait: bool, out: &mut dyn Write) -> Result<()> {
    let name = &machine.name;
    if !machine.installing {
        return Err(Failure(format!("{name} has no install to run.")));
    }
    let before = status(context, machine);
    if before.state != State::Off {
        return Err(Failure(format!("{name} is {}.", before.state.name().to_lowercase())));
    }
    if let Some(warning) = machine_gpu_warning(context, machine) {
        writeln!(out, "Note: {warning}")?;
    }
    let Some(found) = run_task(context, machine, "install", wait, out)? else {
        return Ok(writeln!(out, "Installing {name}. status shows how far it is.")?);
    };
    if load(context, &machine.id)?.installing {
        let log = context.paths.log_file();
        return Err(follow_up(
            &found,
            format!("The install of {name} did not finish. The log has more: {}", log.display()),
        ));
    }
    if !found.error.is_empty() {
        writeln!(out, "Note: {}", found.error)?;
    }
    if machine.macos() {
        return Ok(writeln!(out, "{name} has started Apple's installer. The rest is done in its window.")?);
    }
    Ok(writeln!(out, "{name} is installed.")?)
}

pub(super) fn remove(context: &Context, machine: &Machine, yes: bool, out: &mut dyn Write) -> Result<()> {
    let owned = machine.owns_files(context);
    let what = if owned {
        let places = folders_text(context, machine);
        format!(
            "This deletes {}, its disk and everything on it, in {places}. It cannot be undone.",
            machine.name
        )
    } else {
        format!(
            "Only the entry of {} goes. Its disk stays at {} and is not touched.",
            machine.name, machine.disk
        )
    };
    if !yes {
        return Err(Failure(format!("{what} Run it again with --yes to go on.")));
    }
    if owned { delete(context, machine) } else { remove_entry(context, machine) }?;
    Ok(writeln!(out, "Removed {}.", machine.name)?)
}
