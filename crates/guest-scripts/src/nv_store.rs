//! guest/nv-store.ps1, which brings the guest's copy of NVIDIA's driver to the version on the app's
//! driver disc. Folders stand in for the store and the disc. The CD drives can't be run here, and the
//! queue for the next boot only on Windows.

use crate::{Said, Scratch, guest_script, names, quote, run, shell};
use std::fs;
use std::path::{Path, PathBuf};

const OFFERED: &str = "32.0.16.1800";
const OLDER: &str = "32.0.15.9159";

/// The store with an older driver in it, and a disc with the offered one.
struct Layout {
    shell: &'static str,
    root: PathBuf,
    disc: PathBuf,
    store: PathBuf,
    _scratch: Scratch,
}

impl Layout {
    fn new(shell: &'static str) -> Self {
        let scratch = Scratch::new();
        let root = scratch.path("FileRepository");
        let disc = scratch.path("disc").join("nvidia");
        let store = root.join("nv_dispi.inf_amd64_gpupv");
        write_inf(&store, OLDER);
        fs::write(store.join("nvwgf2umx.dll"), "old").unwrap();
        fs::write(store.join("gone.dll"), "a file the new driver doesn't have").unwrap();
        write_inf(&disc, OFFERED);
        fs::write(disc.join("nvwgf2umx.dll"), "newer").unwrap();
        fs::create_dir_all(disc.join("sub")).unwrap();
        fs::write(disc.join("sub").join("NvGpuComp64.dll"), "deep").unwrap();
        Self { shell, root, disc, store, _scratch: scratch }
    }

    /// The script as the app runs it, with `version` as the one wanted, or none.
    fn update(&self, version: &str) -> Said {
        let version = if version.is_empty() { String::new() } else { format!(" -Version {version}") };
        run(
            self.shell,
            &format!(
                "& {}{version} -Source {} -StoreRoot {}",
                quote(&guest_script("nv-store.ps1")),
                quote(&self.disc),
                quote(&self.root)
            ),
        )
    }
}

impl Drop for Layout {
    fn drop(&mut self) {
        #[cfg(windows)]
        queue::forget(self.shell, &self.root);
    }
}

fn write_inf(folder: &Path, version: &str) {
    fs::create_dir_all(folder).unwrap();
    fs::write(folder.join("nv_dispi.inf"), format!("[Version]\r\nDriverVer=01/01/2026,{version}\r\n"))
        .unwrap();
}

#[test]
fn the_script_puts_the_discs_driver_in_place_of_the_old_and_leaves_nothing_behind() {
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell);
    let said = layout.update(OFFERED);
    assert!(said.lines().contains(&"store copying"), "{said}");
    assert_eq!(said.last(), format!("store replaced {OFFERED}"), "{said}");
    assert_eq!(names(&layout.store), names(&layout.disc), "the old driver's other files went with it");
    assert_eq!(fs::read_to_string(layout.store.join("nvwgf2umx.dll")).unwrap(), "newer");
    assert!(
        !layout.root.join("nv_dispi.inf_amd64_gpupv.old").exists()
            && !layout.root.join("nv_dispi.inf_amd64_gpupv.new").exists()
    );
    let again = layout.update("");
    assert_eq!(
        again.lines(),
        [format!("store current {OFFERED}")],
        "the disc's own version, and nothing to do{again}"
    );
}

#[test]
fn the_script_leaves_the_store_alone_when_the_disc_is_another_version() {
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell);
    let before = names(&layout.store);
    let said = layout.update("32.0.16.9999");
    assert_eq!(said.last(), "store failed: the disc has driver 32.0.16.1800, not 32.0.16.9999", "{said}");
    assert_eq!(names(&layout.store), before);
    assert!(!layout.root.join("nv_dispi.inf_amd64_gpupv.new").exists());
}

/// A change cut short between its two renames, by a power cut, left the store's place empty.
#[test]
fn the_script_puts_back_an_old_store_left_beside_its_place() {
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell);
    fs::rename(&layout.store, layout.root.join("nv_dispi.inf_amd64_gpupv.old")).unwrap();
    let said = layout.update(OFFERED);
    assert_eq!(said.last(), format!("store replaced {OFFERED}"), "{said}");
    assert_eq!(names(&layout.store), names(&layout.disc));
}

/// A copy whose file is the wrong size is not the driver, even with the right version in its INF.
#[test]
fn the_script_copies_again_when_the_store_has_the_version_but_not_the_files() {
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell);
    write_inf(&layout.store, OFFERED);
    let said = layout.update(OFFERED);
    assert_eq!(said.last(), format!("store replaced {OFFERED}"), "{said}");
    assert_eq!(names(&layout.store), names(&layout.disc));
}

/// A disc's files are read-only, and the copy's must not be, or the next change couldn't remove it.
#[test]
fn the_copy_of_a_read_only_disc_can_be_replaced_in_turn() {
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell);
    let disc_files =
        ["nv_dispi.inf", "nvwgf2umx.dll", "sub/NvGpuComp64.dll"].map(|name| layout.disc.join(name));
    set_read_only(&disc_files, true);
    let said = layout.update(OFFERED);
    set_read_only(&disc_files, false);
    assert_eq!(said.last(), format!("store replaced {OFFERED}"), "{said}");
    for name in names(&layout.store) {
        let path = layout.store.join(&name);
        assert!(!fs::metadata(&path).unwrap().permissions().readonly(), "{name} is read-only in the store");
    }
    write_inf(&layout.disc, "32.0.16.1900");
    let next = layout.update("32.0.16.1900");
    assert_eq!(next.last(), "store replaced 32.0.16.1900", "the old copy went{next}");
}

fn set_read_only(files: &[PathBuf], read_only: bool) {
    for file in files {
        let mut permissions = fs::metadata(file).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(read_only);
        fs::set_permissions(file, permissions).unwrap();
    }
}

/// The queue for the next boot moves every old file out before any new one comes in.
#[test]
fn the_renames_for_the_next_boot_move_the_old_files_out_first() {
    let Some(shell) = shell() else { return };
    let layout = Layout::new(shell);
    let old = layout.root.join("old");
    let said = run(
        shell,
        &format!(
            ". {}; Get-RenamePairs {} {} {} | ForEach-Object {{ $_ -join '|' }}",
            quote(&guest_script("nv-store.ps1")),
            quote(&layout.store),
            quote(&layout.disc),
            quote(&old)
        ),
    );
    let pairs: Vec<(&str, &str)> = said.lines().into_iter().filter_map(|line| line.split_once('|')).collect();
    assert_eq!(pairs.len(), 6, "{said}");
    let (out, into) = pairs.split_at(3);
    let (store, disc, old) =
        (layout.store.to_string_lossy(), layout.disc.to_string_lossy(), old.to_string_lossy());
    assert!(out.iter().all(|(from, to)| from.starts_with(&*store) && to.starts_with(&*old)), "{said}");
    assert!(into.iter().all(|(from, to)| from.starts_with(&*disc) && to.starts_with(&*store)), "{said}");
    assert!(into.iter().any(|(_, to)| to.ends_with("NvGpuComp64.dll") && to.contains("sub")), "{said}");
}

/// Windows refuses to rename a folder with a file open in it, as a program that has the driver loaded
/// has, and the change then waits in Windows' queue for the next boot. That queue is in the registry
/// of the whole machine, so this runs on CI's throwaway Windows only, and takes out what it put there.
#[cfg(windows)]
#[test]
fn a_store_in_use_is_queued_for_the_next_boot() {
    use std::os::windows::fs::OpenOptionsExt;
    const SHARE_READ_ONLY: u32 = 1;
    let Some(shell) = shell() else { return };
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        eprintln!("this writes to the registry of the whole machine, so it runs on CI only");
        return;
    }
    let layout = Layout::new(shell);
    let before = names(&layout.store);
    let others_waiting = queue::read(shell).is_some();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ_ONLY)
        .open(layout.store.join("nvwgf2umx.dll"))
        .unwrap();
    let said = layout.update(OFFERED);
    if others_waiting {
        // Windows on the runner already has changes waiting, which the script leaves alone.
        assert_eq!(
            said.last(),
            "store failed: the change could not be queued for the next boot: Windows has other changes waiting for its next restart",
            "{said}"
        );
        drop(held);
        return;
    }
    assert_eq!(said.last(), format!("store queued {OFFERED}"), "{said}");
    let old = layout.root.join("nv_dispi.inf_amd64_gpupv.old");
    let old_names = if old.exists() { names(&old) } else { Vec::new() };
    assert_eq!(
        names(&layout.store),
        before,
        "the refused rename left the store whole, with nothing moved out yet: .old has {old_names:?}"
    );
    let queued = queue::read(shell).unwrap_or_default();
    let mark = format!(r"\??\{}", layout.root.display());
    assert!(queued.iter().all(|entry| entry.starts_with(&mark)), "{queued:#?}");
    assert_eq!(queued.len(), 12, "three files out and three in, each a pair of paths: {queued:#?}");
    // Every file of the store moves out, and before a file of the copy takes its place.
    let store = format!(r"{mark}\nv_dispi.inf_amd64_gpupv\");
    let pairs: Vec<(&str, &str)> =
        queued.chunks(2).map(|pair| (pair[0].as_str(), pair[1].as_str())).collect();
    let out_at = |path: &str| pairs.iter().position(|(from, _)| from.eq_ignore_ascii_case(path));
    for name in &before {
        let path = format!("{store}{}", name.replace('/', r"\"));
        assert!(out_at(&path).is_some(), "{path} never moves out: {queued:#?}");
    }
    for (at, (_, to)) in pairs.iter().enumerate().filter(|(_, (from, _))| !from.starts_with(&store)) {
        if let Some(out) = out_at(to) {
            assert!(out < at, "{to} moves in before the old one moves out: {queued:#?}");
        }
    }
    let again = layout.update(OFFERED);
    assert_eq!(
        again.lines(),
        [format!("store queued {OFFERED}")],
        "a run before the boot sees the queue{again}"
    );
    drop(held);
}

/// Windows' queue of renames for its next boot.
#[cfg(windows)]
mod queue {
    use crate::run;
    use std::path::Path;

    const KEY: &str = r"'SYSTEM\CurrentControlSet\Control\Session Manager'";

    /// The entries, or None when there is no queue.
    pub fn read(shell: &str) -> Option<Vec<String>> {
        let said = run(
            shell,
            &format!(
                "$key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey({KEY}); \
                 if ($key.GetValueNames() -contains 'PendingFileRenameOperations') {{ \
                 'queued'; $key.GetValue('PendingFileRenameOperations') }}"
            ),
        );
        let lines = said.lines();
        let (first, rest) = lines.split_first()?;
        (*first == "queued").then(|| rest.iter().map(|line| line.to_string()).collect())
    }

    /// Takes the queue out if everything in it is under `root`, which only a test put there.
    pub fn forget(shell: &str, root: &Path) {
        let mark = format!(r"\??\{}", root.display()).replace('\'', "''");
        run(
            shell,
            &format!(
                "$key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey({KEY}, $true); \
                 $entries = $key.GetValue('PendingFileRenameOperations'); \
                 if ($entries -and -not @($entries | Where-Object {{ -not $_.StartsWith('{mark}', 'OrdinalIgnoreCase') }})) {{ \
                 $key.DeleteValue('PendingFileRenameOperations') }}"
            ),
        );
    }
}
