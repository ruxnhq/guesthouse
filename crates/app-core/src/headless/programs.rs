//! `apps` and `launcher`: the Windows programs of a machine, and the entries that put them in this
//! desktop's menu. `apps` asks the helper in the guest, which lists the programs that have a shortcut
//! in the Start menu. A program that has none, such as the Store's Notepad and Calculator, is not
//! listed and is still started by its name or path, which is also what `launcher add` takes of it.

use super::windows::running_slot;
use crate::app_windows::launcher::{self, Program};
use crate::app_windows::{App, Launch, Request, connect_side, is_on, off_text};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::{Kind, Machine};
use crate::tasks::{State, status};
use crate::text::base64_decode;
use std::io::Write;
use std::sync::Arc;

/// The Start menu's programs, from the helper of a running machine. It is started for the look if it
/// was not running, and ended after.
fn ask_apps(context: &Context, machine: &Machine, icons: bool) -> Result<Vec<App>> {
    let slot = running_slot(context, machine, "app windows")?;
    let mut link = connect_side(&Arc::new(context.clone()), machine, Some(slot))?;
    let result = link.wait_ready().and_then(|()| {
        let name = &machine.name;
        let answer = link
            .client
            .ask(Request::Apps { icons }, context.app_windows.timing.apps_request)
            .map_err(|failure| Failure(format!("{name} did not list its programs: {failure}.")))?;
        if !answer.ok {
            let why = if answer.error.is_empty() { "it gave no reason" } else { &answer.error };
            return Err(Failure(format!("{name} could not list its programs: {why}.")));
        }
        Ok(answer.apps.unwrap_or_default())
    });
    link.end_if_started();
    result
}

pub(super) fn apps(context: &Context, machine: &Machine, out: &mut dyn Write) -> Result<()> {
    let found = ask_apps(context, machine, false)?;
    if found.is_empty() {
        return Ok(writeln!(out, "{} has no programs in its Start menu.", machine.name)?);
    }
    let width = found.iter().map(|app| app.id.chars().count()).max().unwrap_or(0);
    for app in &found {
        let minor = if app.minor { "  (uninstaller or help)" } else { "" };
        writeln!(out, "{:width$} {}{minor}", app.id, app.name)?;
    }
    writeln!(out, "launcher {} add takes an id from this list, or a program's path or name.", machine.id)?;
    Ok(())
}

/// What a word given to `launcher add` is, once the machine's Start menu has been looked at.
fn program_for(word: &str, found: &[App]) -> Result<Program> {
    let lower = word.to_lowercase();
    let by_id: Vec<&App> = found.iter().filter(|app| app.id.to_lowercase() == lower).collect();
    let by_name: Vec<&App> = found.iter().filter(|app| app.name.to_lowercase() == lower).collect();
    let mut chosen = if by_id.is_empty() { by_name } else { by_id };
    if chosen.len() > 1 {
        let ranked: Vec<&App> = chosen.iter().copied().filter(|app| !app.minor).collect();
        if ranked.len() == 1 {
            chosen = ranked;
        }
    }
    match chosen[..] {
        [] => Ok(by_path(word)),
        [app] => {
            let target = if app.target.is_empty() { &app.id } else { &app.target };
            Ok(Program {
                name: app.name.clone(),
                exe: target.clone(),
                launch: Launch::App(app.id.clone()),
                icon: base64_decode(&app.icon).filter(|png| !png.is_empty()),
            })
        }
        _ => {
            let each: Vec<&str> = chosen.iter().map(|app| app.id.as_str()).collect();
            Err(Failure(format!("{word} fits more than one program: {}. Use an id.", each.join(", "))))
        }
    }
}

/// A program by its path or name, which has no icon to take from anywhere.
fn by_path(word: &str) -> Program {
    let file = word.rsplit(['\\', '/']).next().unwrap_or(word);
    let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file);
    let mut name = stem.to_owned();
    if let Some(first) = name.get(..1) {
        name = first.to_uppercase() + &name[1..];
    }
    Program {
        name,
        exe: word.to_owned(),
        launch: Launch::Path { path: word.to_owned(), args: String::new() },
        icon: None,
    }
}

fn windows_with_app_windows_on(context: &Context, machine: &Machine) -> Result<()> {
    if machine.kind != Kind::Windows {
        return Err(Failure(format!(
            "{} is not a Windows machine, and only Windows has a helper for this.",
            machine.name
        )));
    }
    if !is_on(context, machine) {
        return Err(Failure(off_text(machine)));
    }
    Ok(())
}

pub(super) fn launcher(
    context: &Context,
    machine: &Machine,
    action: &str,
    word: Option<&str>,
    out: &mut dyn Write,
) -> Result<()> {
    let name = &machine.name;
    match (action, word) {
        ("list", _) => {
            let entries = launcher::list(context, machine);
            if entries.is_empty() {
                return Ok(writeln!(out, "{name} has no launcher entries.")?);
            }
            for entry in entries {
                let kind = if entry.start_menu { "Start menu program" } else { "program" };
                writeln!(out, "{} {} {kind} {}", entry.app_id, entry.name, entry.program)?;
            }
            Ok(())
        }
        ("remove", Some(word)) => {
            let removed = launcher::remove(context, machine, word)?;
            Ok(writeln!(out, "Removed {} from the launcher.", removed.name)?)
        }
        ("add", Some(word)) => {
            windows_with_app_windows_on(context, machine)?;
            let running = status(context, machine).state == State::Running;
            let program = if running {
                program_for(word, &ask_apps(context, machine, true)?)?
            } else {
                writeln!(
                    out,
                    "{name} is not running, so {word} is taken as a program's path or name, with no icon. \
                     Start it and add the program again to use its Start menu name and icon."
                )?;
                by_path(word)
            };
            let file = launcher::add(context, machine, &program)?;
            let note = match (&program.launch, program.icon.is_some()) {
                (Launch::Path { .. }, _) if running => " It is not a Start menu program, so it has no icon.",
                _ => "",
            };
            writeln!(
                out,
                "Added {} to the launcher: {}.{note}",
                launcher::entry_name(&program.name, machine),
                file.display()
            )?;
            Ok(())
        }
        (action, None) => Err(Failure(format!("launcher {action} needs a program."))),
        (action, _) => Err(Failure(format!("launcher has no action {action}."))),
    }
}
