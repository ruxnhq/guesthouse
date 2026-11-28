//! The recipe part of `guesthouse-vm`: choosing a recipe for a new machine, listing them, and running
//! a machine's steps again. A recipe is shown before it is used, as the dialog shows it.

use super::{follow_up, run_task};
use crate::actions::{LEAN_TASK, RECIPE_TASK, SCAN_ALL_TASK};
use crate::context::Context;
use crate::defaults::Sizes;
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::library::Extras;
use crate::machine::{Kind, Machine};
use crate::recipe::{
    Chosen, Guest, GuestKind, Source, Usual, find, fit, list, read_chosen, what_it_will_run,
};
use crate::settings::max_vram_gib_for;
use crate::tasks::{State, status};
use std::io::Write;

use super::args::NewOptions;

/// What to say when a person's own recipe is asked for without the word that says they have looked.
const OWN_RECIPE: &str = "This is your own recipe, and what it runs goes into the machine as its administrator. \
                          It is shown above. Run the command again with --yes to go on.";

/// The recipe the person named, shown in full. A recipe of the user's own that runs something is only
/// taken once they have said `--yes`, as no window is here to ask.
pub(super) fn pick(
    context: &Context,
    kind: Kind,
    wanted: &str,
    yes: bool,
    out: &mut dyn Write,
) -> Result<Chosen> {
    if Guest::of_kind(kind).is_none() {
        return Err(Failure::new("A plain VM has no recipes. It is installed by hand."));
    }
    let entry = find(context, kind, wanted)?;
    let file = entry.file.display().to_string();
    writeln!(out, "{}", what_it_will_run(&entry.recipe, entry.source, &file))?;
    if entry.source == Source::Yours && !entry.recipe.steps.is_empty() && !yes {
        return Err(Failure::new(OWN_RECIPE));
    }
    Ok(Chosen { source: entry.source, file, recipe: entry.recipe })
}

/// The sizes a recipe's machine is made with: the recipe's where this PC allows them, and the default
/// where it says nothing. What the person typed is theirs, and the recipe's asks for it are dropped.
pub(super) fn sized(
    context: &Context,
    kind: Kind,
    card: &Option<(String, GpuMaker)>,
    options: &NewOptions,
    chosen: &Chosen,
    usual: Usual,
    out: &mut dyn Write,
) -> Result<(Usual, Extras)> {
    let mut asked = chosen.recipe.settings.clone();
    for (typed, setting) in [
        (options.cores, &mut asked.cores),
        (options.ram, &mut asked.ram_gib),
        (options.disk, &mut asked.disk_gib),
    ] {
        if typed.is_some() {
            *setting = None;
        }
    }
    let mut probe = Machine::new("", "", "", "");
    if let Some((pci, maker)) = card {
        probe.use_windows_card(*maker, pci);
    }
    let fitted = fit(context, kind, &asked, usual, max_vram_gib_for(context, &probe));
    for note in &fitted.notes {
        writeln!(out, "Note: {note}")?;
    }
    let sizes = Usual { cores: fitted.cores, ram_gib: fitted.ram_gib, disk_gib: fitted.disk_gib };
    Ok((sizes, Extras { vram_gib: fitted.vram_gib, recipe: Some(chosen.clone()), ..Extras::default() }))
}

pub(super) fn usual_of(sizes: &Sizes, cores: u32, ram_gib: u32) -> Usual {
    Usual { cores: sizes.cores.min(cores), ram_gib, disk_gib: sizes.disk }
}

/// Every recipe there is for the kinds that have them, and the files that are not recipes.
pub(super) fn show_all(context: &Context, out: &mut dyn Write) -> Result<()> {
    let listing = list(context);
    for entry in &listing.entries {
        let recipe = &entry.recipe;
        let whose = match entry.source {
            Source::App => "",
            Source::Yours => ", your own: what it runs is shown in full before it is used",
        };
        writeln!(
            out,
            "{:<18} {:<8} {} ({}){whose}",
            entry.key,
            recipe.guest.name(),
            recipe.name,
            recipe.description
        )?;
    }
    for problem in &listing.problems {
        writeln!(out, "Not a recipe, {problem}")?;
    }
    if listing.entries.is_empty() && listing.problems.is_empty() {
        let folder = context.paths.your_recipes();
        writeln!(out, "There are no recipes. Your own go in {}, a file each.", folder.display())?;
    }
    Ok(())
}

/// Runs the steps of the machine's recipe again, in the guest, which has to be running.
pub(super) fn run_again(context: &Context, machine: &Machine, wait: bool, out: &mut dyn Write) -> Result<()> {
    let name = &machine.name;
    let Some(chosen) = read_chosen(context, machine) else {
        return Err(Failure(format!("{name} has no recipe.")));
    };
    let before = status(context, machine);
    if before.state != State::Running {
        return Err(Failure(format!("{name} is {}. Start it first.", before.state.name().to_lowercase())));
    }
    let Some(found) = run_task(context, machine, RECIPE_TASK, wait, out)? else {
        return Ok(writeln!(out, "Running {}'s steps. status shows how far it is.", chosen.recipe.name)?);
    };
    if !found.error.is_empty() {
        return Err(follow_up(&found, String::new()));
    }
    Ok(writeln!(out, "The steps of {} ran.", chosen.recipe.name)?)
}

/// Lean Windows again, in the guest, which has to be running.
pub(super) fn lean_again(
    context: &Context,
    machine: &Machine,
    wait: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let name = &machine.name;
    if !crate::lean::is_lean(context, machine) {
        return Err(Failure(format!("{name} was not made lean.")));
    }
    let before = status(context, machine);
    if before.state != State::Running {
        return Err(Failure(format!("{name} is {}. Start it first.", before.state.name().to_lowercase())));
    }
    let Some(found) = run_task(context, machine, LEAN_TASK, wait, out)? else {
        return Ok(writeln!(out, "Making {name} lean again. status shows how far it is.")?);
    };
    if !found.error.is_empty() {
        return Err(follow_up(&found, String::new()));
    }
    Ok(writeln!(out, "{name} is lean again.")?)
}

/// Has Defender scan again the folders the machine left out, in the guest, which has to be running.
pub(super) fn scan_all(context: &Context, machine: &Machine, wait: bool, out: &mut dyn Write) -> Result<()> {
    let name = &machine.name;
    if crate::scan_exclusions::read_exclusions(context, machine).is_empty() {
        return Err(Failure(format!("Defender already scans every folder in {name}.")));
    }
    let before = status(context, machine);
    if before.state != State::Running {
        return Err(Failure(format!("{name} is {}. Start it first.", before.state.name().to_lowercase())));
    }
    let Some(found) = run_task(context, machine, SCAN_ALL_TASK, wait, out)? else {
        return Ok(writeln!(
            out,
            "Having Defender scan every folder in {name}. status shows how far it is."
        )?);
    };
    if !found.error.is_empty() {
        return Err(follow_up(&found, String::new()));
    }
    Ok(writeln!(out, "Defender scans every folder in {name} again.")?)
}
