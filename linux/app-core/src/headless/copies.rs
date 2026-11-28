//! `duplicate`, `export` and `import`: a copy of a machine, here or on another PC, by the same calls as
//! the app's.

use super::run_task_with;
use crate::context::Context;
use crate::duplicate::{DELETE_WORD, DUPLICATE_TASK, THROWAWAY_WORD, copy_name, plan_duplicate};
use crate::export::{EXPORT_TASK, plan_export};
use crate::failure::{Failure, Result};
use crate::machine::Machine;
use std::io::Write;
use std::path::Path;

/// What the copy is to be.
pub(super) struct Copy<'a> {
    pub name: Option<&'a str>,
    pub throwaway: bool,
    pub delete: bool,
}

pub(super) fn duplicate(
    context: &Context,
    source: &Machine,
    wanted: Copy,
    wait: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let name = wanted.name.map_or_else(|| copy_name(source), str::to_owned);
    let plan = plan_duplicate(context, source, &name, wanted.throwaway)?;
    writeln!(out, "{}", plan.cost_text())?;
    let copy = &plan.copy;
    let mut words = vec![name];
    if wanted.throwaway {
        words.push(THROWAWAY_WORD.to_owned());
    }
    if wanted.delete {
        words.push(DELETE_WORD.to_owned());
    }
    let Some(found) = run_task_with(context, source, DUPLICATE_TASK, &words, wait, out)? else {
        return Ok(writeln!(
            out,
            "Copying {} to {}. list shows it once it is made.",
            source.name, copy.name
        )?);
    };
    if !found.error.is_empty() {
        return Err(Failure(found.error));
    }
    Ok(writeln!(out, "{} is made, as {}.", copy.name, copy.id)?)
}

pub(super) fn export(
    context: &Context,
    machine: &Machine,
    file: &str,
    wait: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let to = std::path::absolute(file)?;
    let plan = plan_export(context, machine, &to)?;
    writeln!(out, "{}", plan.cost_text())?;
    let words = [to.to_string_lossy().into_owned()];
    let Some(found) = run_task_with(context, machine, EXPORT_TASK, &words, wait, out)? else {
        return Ok(writeln!(
            out,
            "Exporting {} to {}. status shows how far it is.",
            machine.name,
            to.display()
        )?);
    };
    if !found.error.is_empty() {
        return Err(Failure(found.error));
    }
    Ok(writeln!(out, "{} is exported to {}.", machine.name, to.display())?)
}

/// In this process, as there's no machine yet for a task to be about.
pub(super) fn import(context: &Context, file: &str, name: Option<&str>, out: &mut dyn Write) -> Result<()> {
    // A step that can't be printed costs a line of progress, and the import goes on.
    let made = crate::import::import(context, Path::new(file), name, &mut |step| {
        let _ = writeln!(out, "{step}");
    });
    let made = made?;
    for note in &made.notes {
        writeln!(out, "Note: {note}")?;
    }
    Ok(writeln!(out, "{} is made, as {}.", made.machine.name, made.machine.id)?)
}
