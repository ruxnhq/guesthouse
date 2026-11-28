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
use crate::log::write_log;
use crate::machine::{Machine, valid_id};
use rustix::fs::{FlockOperation, flock};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const PREFIX: &str = "guesthouse-";
const EXTENSION: &str = ".desktop";
const KEY_MACHINE: &str = "X-Guesthouse-Machine";
const KEY_PROGRAM: &str = "X-Guesthouse-Program";
const KEY_TARGET: &str = "X-Guesthouse-Target";
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
    /// The program's key the app id was made from: what its windows are matched by.
    pub target: String,
    pub start_menu: bool,
    pub hidden: bool,
}

const AUMID_PREFIX: &str = "aumid:";

/// `a.b c` as `a-b-c`: letters and digits kept, anything else a dash, none doubled or at the ends.
fn slugify(text: &str) -> String {
    let mut slug = String::new();
    for character in text.to_lowercase().chars() {
        let kept = if character.is_ascii_alphanumeric() { character } else { '-' };
        if kept != '-' || !slug.ends_with('-') {
            slug.push(kept);
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() { "program".to_owned() } else { slug.to_owned() }
}

/// The slug of a program's file name: `C:\Windows\Notepad.exe` is `notepad`.
pub fn slug_of(exe: &str) -> String {
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    let lower = file.to_lowercase();
    slugify(lower.strip_suffix(".exe").unwrap_or(&lower))
}

/// A program's path as one comparable word.
pub fn normal_key(key: &str) -> String {
    key.replace('/', "\\").to_lowercase()
}

/// Whether the key names a packaged app by its Application User Model ID.
pub fn is_aumid(key: &str) -> bool {
    key.get(..AUMID_PREFIX.len()).is_some_and(|start| start.eq_ignore_ascii_case(AUMID_PREFIX))
}

/// The key of a packaged app: `aumid:` and its Application User Model ID.
pub fn aumid_key(aumid: &str) -> String {
    format!("{AUMID_PREFIX}{aumid}")
}

/// What the guest is asked to start for a program key: a packaged app is started through the shell's
/// apps folder by its ID, anything else by its path.
pub fn launch_path(key: &str) -> String {
    if is_aumid(key) { format!("shell:AppsFolder\\{}", &key[AUMID_PREFIX.len()..]) } else { key.to_owned() }
}

/// Six hex digits of a stable hash (FNV-1a), so a copy of a program in another folder is another
/// program with an id of its own, and the same path gives the same id in every run and every version.
fn short_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:06x}", hash & 0xff_ffff)
}

/// What an app id's program part is made from. A key is a full path (`C:\Tools\paint.exe`), an
/// `aumid:` and a package's Application User Model ID, or a bare file name from a helper or a caller
/// that does not know the folder, which is `notepad` and the same program as any file of that name.
/// A path and an AUMID get a hash of the whole key after the slug, so that two programs of one file
/// name stay two.
pub fn program_part(key: &str) -> String {
    if is_aumid(key) {
        let aumid = &key[AUMID_PREFIX.len()..];
        let package = aumid.split(['_', '!']).next().unwrap_or(aumid);
        return format!("{}-{}", slugify(package), short_hash(&normal_key(key)));
    }
    if key.contains(['\\', '/']) {
        return format!("{}-{}", slug_of(key), short_hash(&normal_key(key)));
    }
    slug_of(key)
}

/// The app id of a machine's program: what its entry's file is called without `.desktop`, and what
/// the viewer gives each of the program's windows.
pub fn app_id(machine: &Machine, key: &str) -> String {
    format!("{PREFIX}{}-{}", machine.id, program_part(key))
}

/// The program part of one of the machine's app ids: `guesthouse-work-mspaint-1a2b3c` is `mspaint-1a2b3c`.
pub fn slug_in_app_id<'a>(machine: &Machine, app_id: &'a str) -> Option<&'a str> {
    app_id.strip_prefix(&format!("{PREFIX}{}-", machine.id))
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
    text.push_str(&format!("{KEY_TARGET}={}\n", escape_value(&program.exe)));
    if key {
        text.push_str(&format!("{KEY_START_MENU}=true\n"));
    }
    text
}

/// The PNG of an entry's icon, if it has one. The app id is one of this machine's own entries, and
/// the path is made from it here.
pub fn icon_of(context: &Context, app_id: &str) -> Option<Vec<u8>> {
    let safe = app_id.starts_with(PREFIX)
        && app_id.chars().all(|character| character.is_ascii_alphanumeric() || character == '-');
    safe.then(|| fs::read(icon_file(context, app_id)).ok()).flatten().filter(|png| !png.is_empty())
}

/// Where the icon of an entry is saved, if it has one on this host now.
pub fn icon_path(context: &Context, app_id: &str) -> Option<PathBuf> {
    icon_of(context, app_id).map(|_| icon_file(context, app_id))
}

/// The most an icon may take, in bytes, and the most it may measure each way. A program's icon is a
/// small picture, and what the guest sends is not trusted: it ends up as a file that the desktop's
/// image loaders open.
const ICON_MOST_BYTES: usize = 256 << 10;
const ICON_MOST_SIDE: u32 = 512;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
/// The length of an IHDR chunk's data, and the signature, length, name, data and checksum before the first chunk's end.
const IHDR_LENGTH: u32 = 13;
const IHDR_END: usize = 8 + 4 + 4 + 13 + 4;
/// The last chunk of every PNG, IEND, with its length of nothing and its checksum.
const IEND: [u8; 12] = [0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82];

/// Whether these bytes are a PNG of a sane size that is whole: its signature, an IHDR first with each
/// side from 1 to 512, and IEND at the end.
pub fn valid_icon(png: &[u8]) -> bool {
    if png.len() < IHDR_END + IEND.len() || png.len() > ICON_MOST_BYTES || png[..8] != PNG_SIGNATURE {
        return false;
    }
    let number = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
    let side = |value: u32| (1..=ICON_MOST_SIDE).contains(&value);
    number(8) == IHDR_LENGTH
        && &png[12..16] == b"IHDR"
        && side(number(16))
        && side(number(20))
        && png.ends_with(&IEND)
}

/// Writes a file whole or not at all, in the folder it is for. Each write has a file of its own to
/// write into before it takes the name, as more than one thread or process may write the same entry.
fn write_whole(file: &Path, data: &[u8]) -> Result<()> {
    write_whole_unless(file, data, |_| false)
}

/// The same, but leaves the file as it is if, just before the new one would take its place, what is
/// there now is one `keep` says to keep. The look and the rename are made under a lock on the folder,
/// which every writer of these files takes, so nothing comes between them.
fn write_whole_unless(file: &Path, data: &[u8], keep: impl Fn(&[u8]) -> bool) -> Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let folder = file.parent().ok_or_else(|| Failure::new("A launcher file has no folder."))?;
    fs::create_dir_all(folder)?;
    let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    let new = folder.join(format!(".{name}.{}.{write}.new", std::process::id()));
    let written = fs::write(&new, data).map_err(Failure::from);
    let placed = written.and_then(|()| {
        // A folder that can't be locked (some network file systems) is written without: the write
        // is still whole, only the look before it may be overtaken.
        let held = fs::File::open(folder).ok();
        if let Some(held) = &held {
            let _ = flock(held, FlockOperation::LockExclusive);
        }
        if fs::read(file).is_ok_and(|there| keep(&there)) {
            fs::remove_file(&new)?;
        } else {
            fs::rename(&new, file)?;
        }
        Ok(())
    });
    if placed.is_err() {
        let _ = fs::remove_file(&new);
    }
    placed
}

/// Whether an entry's text is one the user asked for, which a hidden one never replaces.
fn is_visible_entry(text: &[u8]) -> bool {
    !String::from_utf8_lossy(text).lines().any(|line| line == "NoDisplay=true")
}

/// The most entries a machine keeps that the user did not ask for, which are only there to name a
/// window in the task bar. A new one past this takes the place of the oldest.
pub const MOST_HIDDEN_ENTRIES: usize = 64;

/// Removes the oldest hidden entries of the machine until there is room for one more.
fn make_room(context: &Context, machine: &Machine) {
    let mut hidden: Vec<Listed> =
        list_all(context, machine).into_iter().filter(|entry| entry.hidden).collect();
    if hidden.len() < MOST_HIDDEN_ENTRIES {
        return;
    }
    hidden.sort_by_key(|entry| fs::metadata(&entry.file).and_then(|found| found.modified()).ok());
    let extra = hidden.len() + 1 - MOST_HIDDEN_ENTRIES;
    for entry in hidden.iter().take(extra) {
        let _ = delete_entry(context, entry);
    }
}

fn write_entry(context: &Context, machine: &Machine, program: &Program, hidden: bool) -> Result<PathBuf> {
    if !valid_id(&machine.id) {
        return Err(Failure(format!("{} has an id that is not safe in a file name.", machine.name)));
    }
    let id = app_id(machine, &program.exe);
    if hidden && !entry_file(context, &id).exists() {
        make_room(context, machine);
    }
    let icon = match &program.icon {
        Some(png) if valid_icon(png) => {
            let file = icon_file(context, &id);
            write_whole(&file, png)?;
            Some(file)
        }
        Some(png) if !png.is_empty() => {
            let _ = write_log(
                context,
                &format!("app windows: an icon for {id} is not a small PNG, so it is not used"),
            );
            icon_file(context, &id).is_file().then(|| icon_file(context, &id))
        }
        _ => icon_file(context, &id).is_file().then(|| icon_file(context, &id)),
    };
    let file = entry_file(context, &id);
    let text = desktop_text(context, machine, program, icon.as_deref(), hidden);
    // The user may have added the program while this was being written.
    write_whole_unless(&file, text.as_bytes(), |there| hidden && is_visible_entry(there))?;
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
    let there = fs::read_to_string(entry_file(context, &id)).ok();
    if there.as_ref().is_some_and(|text| is_visible_entry(text.as_bytes())) {
        return Ok(id);
    }
    let icon_there = icon_file(context, &id).is_file();
    let wants_icon = program.icon.as_ref().is_some_and(|png| !png.is_empty());
    let icon = (wants_icon || icon_there).then(|| icon_file(context, &id));
    let wanted = desktop_text(context, machine, program, icon.as_deref(), true);
    if there.as_deref() != Some(wanted.as_str()) || (wants_icon && !icon_there) {
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
            target: value_of(&text, KEY_TARGET).map(unescape_value).unwrap_or_default(),
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

#[cfg(test)]
mod icon_tests {
    use super::*;

    /// A PNG's frame with the sides given, and no picture data between the header and the end, which
    /// is all `valid_icon` looks at.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend(IHDR_LENGTH.to_be_bytes());
        bytes.extend(b"IHDR");
        bytes.extend(width.to_be_bytes());
        bytes.extend(height.to_be_bytes());
        bytes.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend(IEND);
        bytes
    }

    #[test]
    fn a_whole_small_png_is_an_icon() {
        assert!(valid_icon(&png(48, 48)));
        assert!(valid_icon(&png(512, 1)));
    }

    #[test]
    fn what_is_not_a_png_is_not() {
        assert!(!valid_icon(b""));
        assert!(!valid_icon(
            b"<svg xmlns='http://www.w3.org/2000/svg'/> and some more bytes to be long enough ....."
        ));
        let mut wrong = png(48, 48);
        wrong[1] = b'X';
        assert!(!valid_icon(&wrong));
    }

    #[test]
    fn a_png_cut_short_is_not_whole() {
        let whole = png(48, 48);
        assert!(!valid_icon(&whole[..whole.len() - 1]));
        assert!(!valid_icon(&whole[..20]));
        assert!(!valid_icon(&PNG_SIGNATURE));
    }

    #[test]
    fn a_picture_that_is_huge_or_has_no_area_is_not_an_icon() {
        assert!(!valid_icon(&png(513, 48)));
        assert!(!valid_icon(&png(48, 513)));
        assert!(!valid_icon(&png(65_000, 65_000)));
        assert!(!valid_icon(&png(0, 48)));
        assert!(!valid_icon(&png(48, 0)));
    }

    #[test]
    fn a_png_over_the_byte_cap_is_not_an_icon() {
        let mut big = png(48, 48);
        let end = big.split_off(big.len() - IEND.len());
        big.resize(ICON_MOST_BYTES, 0);
        big.extend(end);
        assert!(!valid_icon(&big));
    }

    #[test]
    fn a_header_that_says_another_chunk_is_not_a_png() {
        let mut other = png(48, 48);
        other[12..16].copy_from_slice(b"IDAT");
        assert!(!valid_icon(&other));
    }
}
