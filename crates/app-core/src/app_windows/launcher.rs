//! Launcher entries for Windows programs: a `.desktop` file for each program of a machine, in the
//! user's own `$XDG_DATA_HOME/applications`, and its icon in `$XDG_DATA_HOME/icons`. Nothing is ever
//! written anywhere else, and every file name is made here from the machine's id (letters, digits and
//! dashes only) and a slug of the program, so no word of the guest's can reach a path.
//!
//! One file stands for one program of one machine, and its name is the app id the desktop matches a
//! window to a launcher by (on Wayland the `xdg_toplevel` app id is the desktop file's name without
//! `.desktop`). So the same file does two jobs:
//!
//! - a visible entry, which the user asked for (`guesthouse-vm launcher NAME add`), is in the desktop's
//!   menu and starts the program with `guesthouse --app`;
//! - a hidden one (`NoDisplay=true`), which the viewer writes for a program that has a window and no
//!   visible entry, gives that window its name and icon in the task bar all the same.
//!
//! Every file names its machine in `X-Guesthouse-Machine`, and that is how they are found again, so
//! that removing a machine takes its entries with it.

use super::message::Launch;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::{Machine, valid_id};
use std::fs;
use std::path::{Component, Path, PathBuf};

const PREFIX: &str = "guesthouse-";
const EXTENSION: &str = ".desktop";
const KEY_MACHINE: &str = "X-Guesthouse-Machine";
const KEY_PROGRAM: &str = "X-Guesthouse-Program";
const KEY_START_MENU: &str = "X-Guesthouse-Start-Menu";
/// The GUI program that `Exec=` starts. It sits beside `guesthouse-vm`.
pub const GUI_PROGRAM: &str = "guesthouse";
/// What a program with no name of its own is called.
const NO_NAME: &str = "Windows program";

/// What a launcher entry starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// What the entry is called before the machine's name: "Notepad".
    pub name: String,
    /// The program's file name or path in the guest, whose stem is what the viewer matches a window
    /// of the program by (the cell's `exe`). Empty if it is not known.
    pub exe: String,
    pub launch: Launch,
    /// The icon as a PNG, if there is one.
    pub icon: Option<Vec<u8>>,
}

/// An entry as it is read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub file: PathBuf,
    pub app_id: String,
    pub name: String,
    pub program: String,
    pub start_menu: bool,
    pub hidden: bool,
}

/// The slug of a program's file name: `C:\Windows\Notepad.exe` is `notepad`.
pub fn slug_of(exe: &str) -> String {
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    let lower = file.to_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    let mut slug = String::new();
    for character in stem.chars() {
        let kept = if character.is_ascii_alphanumeric() { character } else { '-' };
        if kept != '-' || !slug.ends_with('-') {
            slug.push(kept);
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() { "program".to_owned() } else { slug.to_owned() }
}

/// The app id of a machine's program: what its entry's file is called without `.desktop`, and what
/// the viewer gives each of the program's windows.
pub fn app_id(machine: &Machine, exe: &str) -> String {
    format!("{PREFIX}{}-{}", machine.id, slug_of(exe))
}

fn entry_file(context: &Context, app_id: &str) -> PathBuf {
    context.paths.applications().join(format!("{app_id}{EXTENSION}"))
}

fn icon_file(context: &Context, app_id: &str) -> PathBuf {
    context.paths.icons().join(format!("{app_id}.png"))
}

/// A string value of a desktop file: backslash, newline, tab and carriage return are escaped, and
/// the other control characters (a guest could send any) are left out.
fn escape_value(text: &str) -> String {
    let mut out = String::new();
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other if other.is_control() => {}
            other => out.push(other),
        }
    }
    out
}

fn unescape_value(text: &str) -> String {
    let mut out = String::new();
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// One word of an `Exec=` line, as the desktop entry rules have it: quoted when it holds anything the
/// rules reserve, with `"`, `` ` ``, `$` and `\` escaped there, and `%` doubled so it is no field code.
/// The value escaping of the file itself then applies to the whole line, which is why a backslash that
/// is in the program's path is four in the file.
fn exec_word(word: &str) -> String {
    let word = word.replace('%', "%%");
    let reserved = " \t\n\"'\\><~|&;$*?#()`";
    if !word.chars().any(|character| reserved.contains(character)) {
        return word;
    }
    let mut out = String::from("\"");
    for character in word.chars() {
        if matches!(character, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('"');
    out
}

/// The command a launcher entry runs.
fn exec_line(context: &Context, machine: &Machine, launch: &Launch) -> String {
    let binary = context.paths.binary(GUI_PROGRAM).display().to_string();
    let (flag, program) = match launch {
        Launch::App(id) => (" --start-menu", id.as_str()),
        Launch::Path { path, .. } => ("", path.as_str()),
    };
    format!("{} --app {}{flag} {}", exec_word(&binary), machine.id, exec_word(program))
}

/// The name a person sees: the program's, then the machine's in brackets, as the machine's windows
/// are told apart in a menu of many.
pub fn entry_name(program_name: &str, machine: &Machine) -> String {
    let clean = |text: &str| text.chars().filter(|character| !character.is_control()).collect::<String>();
    let program = clean(program_name);
    let program = program.trim();
    format!("{} ({})", if program.is_empty() { NO_NAME } else { program }, clean(&machine.name).trim())
}

/// The text of an entry. `icon` is the path of its icon file.
pub fn desktop_text(
    context: &Context,
    machine: &Machine,
    program: &Program,
    icon: Option<&Path>,
    hidden: bool,
) -> String {
    let id = app_id(machine, &program.exe);
    let mut text = String::from("[Desktop Entry]\nType=Application\nVersion=1.0\n");
    text.push_str(&format!("Name={}\n", escape_value(&entry_name(&program.name, machine))));
    text.push_str(&format!(
        "Comment={}\n",
        escape_value(&format!("A program of {}, in a window of its own on this desktop.", machine.name))
    ));
    text.push_str(&format!("Exec={}\n", escape_value(&exec_line(context, machine, &program.launch))));
    if let Some(icon) = icon {
        text.push_str(&format!("Icon={}\n", escape_value(&icon.display().to_string())));
    }
    text.push_str("Terminal=false\nCategories=Utility;\nStartupNotify=false\n");
    text.push_str(&format!("StartupWMClass={id}\n"));
    if hidden {
        text.push_str("NoDisplay=true\n");
    }
    text.push_str(&format!("{KEY_MACHINE}={}\n", machine.id));
    let (key, value) = match &program.launch {
        Launch::App(id) => (true, id.as_str()),
        Launch::Path { path, .. } => (false, path.as_str()),
    };
    text.push_str(&format!("{KEY_PROGRAM}={}\n", escape_value(value)));
    if key {
        text.push_str(&format!("{KEY_START_MENU}=true\n"));
    }
    text
}

/// Writes a file whole or not at all, in the folder it is for.
fn write_whole(file: &Path, data: &[u8]) -> Result<()> {
    let folder = file.parent().ok_or_else(|| Failure::new("A launcher file has no folder."))?;
    fs::create_dir_all(folder)?;
    let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let new = folder.join(format!(".{name}.new"));
    fs::write(&new, data)?;
    fs::rename(&new, file)?;
    Ok(())
}

fn write_entry(context: &Context, machine: &Machine, program: &Program, hidden: bool) -> Result<PathBuf> {
    if !valid_id(&machine.id) {
        return Err(Failure(format!("{} has an id that is not safe in a file name.", machine.name)));
    }
    let id = app_id(machine, &program.exe);
    let icon = match &program.icon {
        Some(png) if !png.is_empty() => {
            let file = icon_file(context, &id);
            write_whole(&file, png)?;
            Some(file)
        }
        _ => icon_file(context, &id).is_file().then(|| icon_file(context, &id)),
    };
    let file = entry_file(context, &id);
    write_whole(&file, desktop_text(context, machine, program, icon.as_deref(), hidden).as_bytes())?;
    Ok(file)
}

/// Writes the entry the user asked for, replacing a hidden one or an older one of the same program.
pub fn add(context: &Context, machine: &Machine, program: &Program) -> Result<PathBuf> {
    write_entry(context, machine, program, false)
}

/// What a window of a program gets its place in the task bar from. Returns the app id to give the
/// window. A visible entry that is already there is left as it is; a hidden one is written if there is
/// none, or rewritten when it had no icon and now has one.
pub fn ensure_for_window(context: &Context, machine: &Machine, program: &Program) -> Result<String> {
    let id = app_id(machine, &program.exe);
    let file = entry_file(context, &id);
    let wants_icon = program.icon.as_ref().is_some_and(|png| !png.is_empty());
    let there = fs::read_to_string(&file).ok();
    let settled = there.as_ref().is_some_and(|text| {
        let visible = !text.lines().any(|line| line == "NoDisplay=true");
        visible || !wants_icon || icon_file(context, &id).is_file()
    });
    if !settled {
        write_entry(context, machine, program, true)?;
    }
    Ok(id)
}

/// The value of a key in the entry's main group.
fn value_of<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}

/// The files of a machine's entries, each with its text.
fn entries_of(context: &Context, machine: &Machine) -> Vec<(PathBuf, String)> {
    let Ok(folder) = fs::read_dir(context.paths.applications()) else { return Vec::new() };
    let mut found: Vec<(PathBuf, String)> = folder
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with(PREFIX) || !name.ends_with(EXTENSION) {
                return None;
            }
            let text = fs::read_to_string(&path).ok()?;
            (value_of(&text, KEY_MACHINE) == Some(machine.id.as_str())).then_some((path, text))
        })
        .collect();
    found.sort();
    found
}

/// The entries a machine has, hidden ones too.
pub fn list_all(context: &Context, machine: &Machine) -> Vec<Listed> {
    entries_of(context, machine)
        .into_iter()
        .map(|(file, text)| Listed {
            app_id: file.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default(),
            name: unescape_value(value_of(&text, "Name").unwrap_or_default()),
            program: unescape_value(value_of(&text, KEY_PROGRAM).unwrap_or_default()),
            start_menu: value_of(&text, KEY_START_MENU) == Some("true"),
            hidden: value_of(&text, "NoDisplay") == Some("true"),
            file,
        })
        .collect()
}

/// The entries the user asked for.
pub fn list(context: &Context, machine: &Machine) -> Vec<Listed> {
    list_all(context, machine).into_iter().filter(|entry| !entry.hidden).collect()
}

/// Whether a path is a file in the user's icon folder, as a path read from a file could be anywhere.
fn inside_icons(context: &Context, path: &Path) -> bool {
    path.starts_with(context.paths.icons()) && !path.components().any(|part| part == Component::ParentDir)
}

fn delete_entry(context: &Context, listed: &Listed) -> Result<()> {
    if let Ok(text) = fs::read_to_string(&listed.file)
        && let Some(icon) = value_of(&text, "Icon")
    {
        let icon = PathBuf::from(unescape_value(icon));
        if inside_icons(context, &icon) {
            let _ = fs::remove_file(icon);
        }
    }
    fs::remove_file(&listed.file)?;
    Ok(())
}

/// Removes the entry the user asked for that `key` names: its app id, its name, its program (a path or
/// a Start menu id) or the program's file name, in any case. A key that fits none, or several, says so.
pub fn remove(context: &Context, machine: &Machine, key: &str) -> Result<Listed> {
    let shown = list(context, machine);
    let lower = key.to_lowercase();
    let fits = |entry: &&Listed| {
        let name = entry.name.to_lowercase();
        entry.app_id == key
            || entry.program.to_lowercase() == lower
            || name == lower
            || name
                .strip_suffix(&format!(" ({})", machine.name.to_lowercase()))
                .is_some_and(|own| own == lower)
            || slug_of(&entry.program) == slug_of(key)
    };
    let found: Vec<&Listed> = shown.iter().filter(fits).collect();
    match found[..] {
        [only] => {
            delete_entry(context, only)?;
            Ok(only.clone())
        }
        [] => Err(Failure(format!("{} has no launcher entry for {key}.", machine.name))),
        _ => {
            let each: Vec<&str> = found.iter().map(|entry| entry.app_id.as_str()).collect();
            Err(Failure(format!(
                "{key} fits more than one entry of {}: {}. Use one of these.",
                machine.name,
                each.join(", ")
            )))
        }
    }
}

/// Removes every entry of the machine, hidden ones too, as the machine itself is removed. Says how many
/// went. An entry that cannot be removed is skipped: the machine's removal is not held up by it.
pub fn remove_all(context: &Context, machine: &Machine) -> usize {
    list_all(context, machine).iter().filter(|entry| delete_entry(context, entry).is_ok()).count()
}
