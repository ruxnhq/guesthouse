//! guest/windows/nv-files.ps1, which copies the driver's files from the host driver store to where a normal
//! install puts them. Folders stand in for the store and for Windows' own folders.

use crate::{Said, Scratch, guest_script, quote, run, shell};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const STORE_FOLDER: &str = "nv_dispi.inf_amd64_gpupv";

struct Layout {
    shell: &'static str,
    root: PathBuf,
    store: PathBuf,
    windows: PathBuf,
    list: PathBuf,
    _scratch: Scratch,
}

impl Layout {
    /// A store with `files` in it, and a list that places each in System32 under its own name.
    fn new(shell: &'static str, files: &[&str]) -> Self {
        let scratch = Scratch::new();
        let root = scratch.path("FileRepository");
        let store = root.join(STORE_FOLDER);
        let windows = scratch.path("Windows");
        fs::create_dir_all(&store).unwrap();
        fs::create_dir_all(windows.join("System32")).unwrap();
        let mut list = String::new();
        for name in files {
            fs::write(store.join(name), format!("{name} from the driver")).unwrap();
            list.push_str(&format!("System32\t{name}\t{name}\r\n"));
        }
        let list_path = scratch.path("nv-files.txt");
        fs::write(&list_path, list).unwrap();
        Self { shell, root, store, windows, list: list_path, _scratch: scratch }
    }

    fn place(&self, check: bool) -> Said {
        run(
            self.shell,
            &format!(
                "$env:windir = {}; & {} {} -StoreFolder {STORE_FOLDER} -StoreRoot {}{}",
                quote(&self.windows),
                quote(&guest_script("nv-files.ps1")),
                quote(&self.list),
                quote(&self.root),
                if check { " -Check" } else { "" }
            ),
        )
    }

    fn placed(&self, name: &str) -> PathBuf {
        self.windows.join("System32").join(name)
    }
}

fn set_modified(path: &PathBuf, when: SystemTime) {
    fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
}

#[test]
fn what_is_missing_is_copied_and_then_kept() {
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell, &["nvapi64.dll", "nvcuda.dll"]);
    let check = layout.place(true);
    assert_eq!(check.lines(), ["to place 2"], "{check}");
    let said = layout.place(false);
    assert_eq!(
        said.lines(),
        [
            r"copied System32\nvapi64.dll from nvapi64.dll",
            r"copied System32\nvcuda.dll from nvcuda.dll",
            "copied 2, kept 0, failed 0"
        ],
        "{said}"
    );
    assert_eq!(fs::read_to_string(layout.placed("nvcuda.dll")).unwrap(), "nvcuda.dll from the driver");
    let again = layout.place(false);
    assert_eq!(again.last(), "copied 0, kept 2, failed 0", "{again}");
    let check = layout.place(true);
    assert_eq!(check.lines(), ["to place 0"], "{check}");
}

/// Files with no version, as these are, go by their date.
#[test]
fn a_newer_file_in_place_stays_and_an_older_one_is_replaced() {
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell, &["newer.dll", "older.dll"]);
    let now = SystemTime::now();
    for name in ["newer.dll", "older.dll"] {
        fs::write(layout.placed(name), "already there").unwrap();
        set_modified(&layout.store.join(name), now - Duration::from_secs(3600));
    }
    set_modified(&layout.placed("newer.dll"), now);
    set_modified(&layout.placed("older.dll"), now - Duration::from_secs(7200));
    let said = layout.place(false);
    assert_eq!(said.last(), "copied 1, kept 1, failed 0", "{said}");
    assert!(said.lines().contains(&r"kept System32\newer.dll"), "{said}");
    assert_eq!(fs::read_to_string(layout.placed("newer.dll")).unwrap(), "already there");
    assert_eq!(fs::read_to_string(layout.placed("older.dll")).unwrap(), "older.dll from the driver");
}

#[test]
fn a_file_the_store_lacks_is_counted_as_failed() {
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell, &["nvapi64.dll", "missing.dll"]);
    fs::remove_file(layout.store.join("missing.dll")).unwrap();
    let said = layout.place(false);
    assert_eq!(said.last(), "copied 1, kept 0, failed 1", "{said}");
    assert!(
        said.lines().iter().any(|line| line.starts_with(r"FAILED System32\missing.dll from missing.dll: ")),
        "{said}"
    );
    let check = layout.place(true);
    assert_eq!(check.lines(), ["to place 1"], "a check counts it as still to place{check}");
}

/// A program that has a DLL loaded lets it be renamed and not written over. The file in use is moved
/// aside, and the copy moved aside goes at a later run.
#[cfg(windows)]
#[test]
fn a_file_in_use_is_moved_aside_and_replaced() {
    use std::os::windows::fs::OpenOptionsExt;
    const SHARE_READ_AND_DELETE: u32 = 1 | 4;
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell, &["nvapi64.dll"]);
    let placed = layout.placed("nvapi64.dll");
    fs::write(&placed, "loaded").unwrap();
    set_modified(&placed, SystemTime::now() - Duration::from_secs(3600));
    let held = fs::OpenOptions::new().read(true).share_mode(SHARE_READ_AND_DELETE).open(&placed).unwrap();
    let said = layout.place(false);
    assert_eq!(said.last(), "copied 1, kept 0, failed 0", "{said}");
    assert_eq!(fs::read_to_string(&placed).unwrap(), "nvapi64.dll from the driver");
    let aside = || -> Vec<String> {
        fs::read_dir(layout.windows.join("System32"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("nvapi64.dll.") && name.ends_with(".old"))
            .collect()
    };
    let first = aside();
    assert_eq!(first.len(), 1, "{said}");
    drop(held);
    set_modified(&layout.store.join("nvapi64.dll"), SystemTime::now() + Duration::from_secs(60));
    let next = layout.place(false);
    assert_eq!(next.last(), "copied 1, kept 0, failed 0", "{next}");
    // This run moved its own file aside in turn, and the one from before went.
    let now = aside();
    assert!(now.len() == 1 && now != first, "{first:?} then {now:?}{next}");
}

/// A file held open in a way that bars renaming it stays as it was, and the run says so.
#[cfg(windows)]
#[test]
fn a_file_that_cannot_be_moved_stays_and_is_counted_as_failed() {
    use std::os::windows::fs::OpenOptionsExt;
    const SHARE_READ_ONLY: u32 = 1;
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell, &["nvapi64.dll"]);
    let placed = layout.placed("nvapi64.dll");
    fs::write(&placed, "held").unwrap();
    set_modified(&placed, SystemTime::now() - Duration::from_secs(3600));
    let held = fs::OpenOptions::new().read(true).share_mode(SHARE_READ_ONLY).open(&placed).unwrap();
    let said = layout.place(false);
    drop(held);
    assert_eq!(said.last(), "copied 0, kept 0, failed 1", "{said}");
    assert_eq!(fs::read_to_string(&placed).unwrap(), "held");
}
