//! Making a machine, installing it, and taking it away: what the New Machine dialog, Start Over and
//! the machine's Delete do.

use super::args::NewOptions;
use super::change::on_or_off;
use super::{follow_up, run_task};
use crate::card_choice::{default_choice, windows_card_choices};
use crate::context::Context;
use crate::defaults::{NO_GPU, account_name, sizes_for_disc};
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::gpu_support::{kind_offered, plain_reason, read_disc, ubuntu_note};
use crate::host::{RamField, physical_cores};
use crate::iso::Iso;
use crate::library::{NewMachine, create, delete, folders_text, remove_entry};
use crate::machine::{Kind, MIN_RAM_GIB, Machine, load};
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
/// maker. No choice at all is the NVIDIA card.
fn card_for(context: &Context, named: Option<&str>) -> Result<(String, GpuMaker)> {
    let choices = windows_card_choices(context);
    let Some(pci) = named else {
        return Ok(choices
            .get(default_choice(context, &choices))
            .map_or_else(|| (String::new(), GpuMaker::Nvidia), |choice| (choice.pci.clone(), choice.maker)));
    };
    if let Some(choice) = choices.iter().find(|choice| !choice.pci.is_empty() && choice.pci == pci) {
        return Ok((pci.to_owned(), choice.maker));
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

fn access_text(access: &Access) -> String {
    let on: Vec<&str> =
        Service::ALL.iter().filter(|service| access.is_on(**service)).map(|service| service.name()).collect();
    match on.as_slice() {
        [] => "Remote Desktop and SSH are off.".to_owned(),
        [one] => format!("{one} is on, and a port forward reaches it."),
        _ => "Remote Desktop and SSH are on, and a port forward reaches each.".to_owned(),
    }
}

pub(super) fn new_machine(
    context: &Context,
    name: &str,
    options: &NewOptions,
    out: &mut dyn Write,
) -> Result<()> {
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
    if let (Some(installer), Some((pci, _))) = (options.amd_driver, &card) {
        unpack_amd_driver(context, pci, installer, out)?;
    }
    if let (Some(installer), Some((pci, _))) = (options.intel_driver, &card) {
        unpack_intel_driver(context, pci, installer, out)?;
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
    let new = NewMachine {
        name: name.to_owned(),
        iso: iso.to_owned(),
        kind,
        windows_disc: disc.windows_disc,
        cores: number("--cores", options.cores, sizes.cores.min(physical_cores(&context.cpus)))?,
        ram_gib: number("--ram", options.ram, field.value as u32)?,
        disk_gib: number("--disk", options.disk, sizes.disk)?,
        refresh_hz: None,
        user: account_name(kind, options.user.unwrap_or_default().trim()),
        password: String::new(),
        amd_gpu: card.as_ref().map(|(pci, _)| pci.clone()),
        disk_place: options.disk_folder.unwrap_or_default().to_owned(),
        gpu_maker: card.map(|(_, maker)| maker),
        access: access_for(windows, options)?,
    };
    if new.name.trim().is_empty() {
        return Err(Failure::new("The machine needs a name."));
    }
    let machine = create(context, &new)?;
    writeln!(out, "Made {} in {}.", machine.name, machine.folder(context).display())?;
    if !machine.disk_folder.is_empty() {
        writeln!(out, "Its disk is in {}.", machine.disk_folder)?;
    }
    if windows {
        writeln!(out, "{}", access_text(&new.access))?;
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
