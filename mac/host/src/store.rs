// SPDX-License-Identifier: GPL-2.0-or-later
//! The machine store: one folder per machine under the machines folder, each holding a
//! `machine.json` with the machine's settings. The store lists what is there and works out what
//! starting a machine needs from it.

use crate::launch::{DEFAULT_MODES, DEFAULT_SCALE, Graphics, InstallerBoot, LINUX, Launch};
use guesthouse_state_core::common::State;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The file in each machine's folder that holds its settings.
pub const MACHINE_FILE: &str = "machine.json";

/// Where the app keeps its things, under the user's home folder.
const SUPPORT_UNDER_HOME: &str = "Library/Application Support/Guesthouse";

/// Set to move everything the app keeps (machines, run folders, the agent's socket) somewhere
/// else, for trying the app without touching the real folder.
pub const SUPPORT_ENV: &str = "GUESTHOUSE_SUPPORT_DIR";

/// `~/Library/Application Support/Guesthouse`, or `None` without a home folder.
pub fn support_folder() -> Option<PathBuf> {
    if let Some(folder) = std::env::var_os(SUPPORT_ENV) {
        return Some(PathBuf::from(folder));
    }
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(SUPPORT_UNDER_HOME))
}

/// The machines folder, `Machines` in the support folder.
pub fn default_folder() -> Option<PathBuf> {
    Some(support_folder()?.join("Machines"))
}

/// The run folder of machine `id`, beside the machines folder: the sockets, log and pid file of
/// a running machine. Kept short, since sockets live in it.
pub fn run_folder(machines: &Path, id: &str) -> PathBuf {
    machines.with_file_name("Run").join(id)
}

/// The mode of a machine's run folder and of the `Run` folder above it: this user only.
pub const RUN_FOLDER_MODE: u32 = 0o700;

/// Makes a machine's run folder (and the `Run` folder above it) readable by this user only. The
/// graphics process and QEMU make their sockets there under the process's umask, which leaves them
/// open to every user of the Mac, so the folder is what keeps them private. Done at every start, so
/// a folder an older version made with the default mode is closed too.
pub fn make_run_folder_private(run_dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(run_dir)?;
    for folder in [run_dir.parent(), Some(run_dir)].into_iter().flatten() {
        fs::set_permissions(folder, fs::Permissions::from_mode(RUN_FOLDER_MODE))?;
    }
    Ok(())
}

/// What a machine's `machine.json` holds.
#[derive(Deserialize)]
struct Settings {
    name: String,
    kind: String,
    cores: u32,
    memory_mb: u64,
    disk: String,
    #[serde(default)]
    frames: Option<String>,
    #[serde(default)]
    qmp: Option<String>,
    // What starting needs. A machine with no firmware files is one that something else runs.
    #[serde(default)]
    firmware_code: Option<String>,
    #[serde(default)]
    firmware_vars: Option<String>,
    #[serde(default)]
    stick: Option<String>,
    /// Set only while a machine is being installed.
    #[serde(default)]
    installer: Option<String>,
    #[serde(default)]
    answer_disc: Option<String>,
    /// The installer's kernel and initrd, taken from its disc, and its kernel line: set only
    /// while an installer that needs them runs.
    #[serde(default)]
    installer_kernel: Option<String>,
    #[serde(default)]
    installer_initrd: Option<String>,
    #[serde(default)]
    installer_command_line: Option<String>,
    #[serde(default)]
    option_rom: Option<String>,
    #[serde(default)]
    modes: Option<Vec<String>>,
    /// Off unless the person turned it on: nothing listens for other devices by default.
    #[serde(default)]
    allow_remote_view: bool,
    /// Where the graphics device lives. A file without the key means the guarded path, which is
    /// what every machine is meant to use; `qemu` is chosen on purpose.
    #[serde(default)]
    graphics: Graphics,
    #[serde(default)]
    remote_port: Option<u16>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Machine {
    /// The machine's folder name, which names it to the agent and in its run folder.
    pub id: String,
    pub name: String,
    pub kind: String,
    pub cores: u32,
    pub memory_mb: u64,
    pub disk: String,
    /// The bootstrap name the machine's screen is published under, if it has one.
    pub frames: Option<String>,
    /// The QMP socket that takes the machine's input, if it has one.
    pub qmp: Option<String>,
    /// Where it stands as the store knows it (always off); the agent knows better once it runs.
    pub state: State,
    /// The machine's own folder.
    pub folder: String,
    /// Whether the agent can start it: it names its firmware files.
    pub startable: bool,
    /// The display modes the device offers, as `WIDTHxHEIGHT@HZ`, the first being the one it
    /// starts in. Empty for a machine the app does not start, whose modes it cannot know.
    pub modes: Vec<String>,
    /// Whether the person allowed other devices to view this machine's screen.
    pub remote_view: bool,
    /// `guarded` (the graphics process behind QEMU's pvg-fwd) or `qemu` (QEMU's own device).
    pub graphics: String,
    /// The port the screen is shared on, once one has been chosen for this machine.
    pub remote_port: Option<u16>,
    #[serde(skip)]
    pub launch: Option<Launch>,
}

/// A folder that looked like a machine but could not be read, and why.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub folder: String,
    pub reason: String,
}

#[derive(Serialize, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub machines: Vec<Machine>,
    pub skipped: Vec<Skipped>,
}

/// Lists the machines in `folder`, sorted by name. A missing folder means no machines yet.
/// Plain files are ignored; a folder whose `machine.json` is missing or broken is skipped and
/// reported, so one bad machine never hides the others.
pub fn list(folder: &Path) -> io::Result<Listing> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Listing::default()),
        Err(error) => return Err(error),
    };

    let mut listing = Listing::default();
    for entry in entries {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        match read_machine(&path) {
            Ok(machine) => listing.machines.push(machine),
            Err(reason) => listing.skipped.push(Skipped { folder: path.display().to_string(), reason }),
        }
    }
    listing.machines.sort_by(|a, b| a.name.cmp(&b.name));
    listing.skipped.sort_by(|a, b| a.folder.cmp(&b.folder));
    Ok(listing)
}

fn read_machine(folder: &Path) -> Result<Machine, String> {
    let text = fs::read_to_string(folder.join(MACHINE_FILE))
        .map_err(|error| format!("cannot read {MACHINE_FILE}: {error}"))?;
    let settings: Settings =
        serde_json::from_str(&text).map_err(|error| format!("{MACHINE_FILE} is not valid: {error}"))?;
    let id = folder.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let launch = launch_for(folder, &id, &settings);
    // A machine the agent starts gets its screen and input names made up; one run by hand
    // keeps what its file says.
    let (frames, qmp) = match &launch {
        Some(launch) => (
            settings.frames.or(Some(launch.frame_name.clone())),
            settings.qmp.or(Some(launch.input_socket.display().to_string())),
        ),
        None => (settings.frames, settings.qmp),
    };
    Ok(Machine {
        id,
        name: settings.name,
        kind: settings.kind,
        cores: settings.cores,
        memory_mb: settings.memory_mb,
        disk: resolve(folder, &settings.disk).display().to_string(),
        frames,
        qmp,
        state: State::Off,
        folder: folder.display().to_string(),
        startable: launch.is_some(),
        modes: launch.as_ref().map(|l| l.modes.clone()).unwrap_or_default(),
        remote_view: settings.allow_remote_view,
        graphics: settings.graphics.name().to_string(),
        remote_port: settings.remote_port,
        launch,
    })
}

/// A path from `machine.json`: as it is when absolute, else inside the machine's own folder.
fn resolve(folder: &Path, path: &str) -> PathBuf {
    folder.join(path)
}

/// The installer's kernel for the firmware, when the machine names both its files.
fn installer_boot(folder: &Path, settings: &Settings) -> Option<InstallerBoot> {
    Some(InstallerBoot {
        kernel: resolve(folder, settings.installer_kernel.as_deref()?),
        initrd: resolve(folder, settings.installer_initrd.as_deref()?),
        command_line: settings.installer_command_line.clone().unwrap_or_default(),
    })
}

/// The file in a Linux machine's folder that keeps the last mode its screen window had, which the
/// graphics process took.
pub const SCREEN_MODE_FILE: &str = "screen-mode";

/// The modes of a machine whose file names none. Windows' are run.sh's, the smallest first, as
/// they always were. A Linux guest's desktop comes up in the size its screen window last had,
/// with only the modes no larger behind it: a guest's driver or compositor may take the largest
/// whatever the order, as pvg-host's own window knows. With no size kept, run.sh's largest
/// first, so it starts at a size worth showing before the window, if any, gives its own.
fn default_modes(folder: &Path, kind: &str) -> Vec<String> {
    let mut modes: Vec<String> = DEFAULT_MODES.iter().map(|mode| mode.to_string()).collect();
    if kind != LINUX {
        return modes;
    }
    modes.reverse();
    let last = fs::read_to_string(folder.join(SCREEN_MODE_FILE)).unwrap_or_default();
    let last = last.trim();
    if let Some((width, height)) = mode_size(last) {
        modes.retain(|mode| mode_size(mode).is_some_and(|(w, h)| w <= width && h <= height) && mode != last);
        modes.insert(0, last.to_string());
    }
    modes
}

/// The width and height of `text` when it is a mode a guest can be given.
fn mode_size(text: &str) -> Option<(u32, u32)> {
    is_mode(text).then(|| {
        let (size, _) = text.split_once('@').unwrap_or_default();
        let (width, height) = size.split_once('x').unwrap_or_default();
        (width.parse().unwrap_or(0), height.parse().unwrap_or(0))
    })
}

/// Whether `text` is a mode a guest can be given: `WxH@R`, at least 320 by 200 and at most what
/// the graphics process offers (`screen_size::MOST_PIXELS`).
fn is_mode(text: &str) -> bool {
    let number = |text: &str| -> Option<u32> {
        (!text.is_empty() && text.len() <= 5 && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
    };
    let Some((size, hertz)) = text.split_once('@') else {
        return false;
    };
    let Some((width, height)) = size.split_once('x') else {
        return false;
    };
    let (most_width, most_height) = crate::screen_size::MOST_PIXELS;
    matches!(
        (number(width), number(height), number(hertz)),
        (Some(w), Some(h), Some(r)) if (320..=most_width).contains(&w)
            && (200..=most_height).contains(&h)
            && (1..=240).contains(&r)
    )
}

/// Keeps `mode`, which the graphics process of the Linux machine in `folder` took for its screen
/// window, as the first mode of its next start. Nothing for other machines, or for a mode that
/// is not one.
pub fn remember_screen_mode(folder: &Path, kind: &str, mode: &str) -> io::Result<()> {
    if kind != LINUX || !is_mode(mode) {
        return Ok(());
    }
    fs::write(folder.join(SCREEN_MODE_FILE), format!("{mode}\n"))
}

/// What starting needs, when the machine names its firmware files.
fn launch_for(folder: &Path, id: &str, settings: &Settings) -> Option<Launch> {
    let code = settings.firmware_code.as_deref()?;
    let vars = settings.firmware_vars.as_deref()?;
    let run_dir = run_folder(folder.parent()?, id);
    let modes = settings.modes.clone().unwrap_or_else(|| default_modes(folder, &settings.kind));
    Some(Launch {
        graphics: settings.graphics,
        kind: settings.kind.clone(),
        // The app says Sharp or Standard when it starts the machine.
        scale: DEFAULT_SCALE,
        // The Mac's, when the machine starts (agent).
        keyboard: None,
        host_socket: run_dir.join("host.sock"),
        host_control: run_dir.join("control.sock"),
        cores: settings.cores,
        memory_mb: settings.memory_mb,
        disk: resolve(folder, &settings.disk),
        firmware_code: resolve(folder, code),
        firmware_vars: resolve(folder, vars),
        stick: settings.stick.as_deref().map(|path| resolve(folder, path)),
        installer: settings.installer.as_deref().map(|path| resolve(folder, path)),
        answer_disc: settings.answer_disc.as_deref().map(|path| resolve(folder, path)),
        installer_boot: installer_boot(folder, settings),
        option_rom: settings.option_rom.as_deref().map(|path| resolve(folder, path)),
        modes,
        frame_name: format!("guesthouse.{id}"),
        control_socket: run_dir.join("qmp.sock"),
        input_socket: run_dir.join("input.sock"),
        remote_socket: run_dir.join("remote.sock"),
        run_dir,
    })
}

/// A machine's folder, by its id.
pub fn default_folder_of(machines: &Path, id: &str) -> PathBuf {
    machines.join(id)
}

/// Reads one machine by its folder name, for the agent.
pub fn find(machines: &Path, id: &str) -> Result<Machine, String> {
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return Err(format!("no machine called {id}"));
    }
    let folder = machines.join(id);
    if !folder.is_dir() {
        return Err(format!("no machine called {id}"));
    }
    read_machine(&folder)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_run_folder_and_the_one_above_it_are_this_users_only() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("gh-run-mode-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let run = base.join("Run").join("win");
        // As an older version left them: open to everyone.
        fs::create_dir_all(&run).unwrap();
        fs::set_permissions(base.join("Run"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
        make_run_folder_private(&run).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&run), RUN_FOLDER_MODE);
        assert_eq!(mode(&base.join("Run")), RUN_FOLDER_MODE);
        let _ = fs::remove_dir_all(base);
    }

    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh empty folder for one test, under the system's temporary folder, removed when the
    /// test ends.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl std::ops::Deref for Scratch {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    fn scratch() -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name =
            format!("guesthouse-store-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }

    fn add_machine(store: &Path, folder: &str, json: &str) {
        let path = store.join(folder);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(MACHINE_FILE), json).unwrap();
    }

    fn settings(name: &str) -> String {
        format!(
            r#"{{"name": "{name}", "kind": "windows", "cores": 4, "memory_mb": 8192, "disk": "/disks/{name}.img"}}"#
        )
    }

    #[test]
    fn missing_folder_is_empty() {
        let store = scratch();
        assert_eq!(list(&store.join("not-there")).unwrap(), Listing::default());
    }

    #[test]
    fn reads_a_machine_as_stopped() {
        let store = scratch();
        add_machine(&store, "win", &settings("Windows 11"));
        let listing = list(&store).unwrap();
        assert!(listing.skipped.is_empty());
        let machine = &listing.machines[0];
        assert_eq!(machine.name, "Windows 11");
        assert_eq!(machine.kind, "windows");
        assert_eq!(machine.cores, 4);
        assert_eq!(machine.memory_mb, 8192);
        assert_eq!(machine.disk, "/disks/Windows 11.img");
        assert_eq!(machine.state, State::Off);
        assert_eq!(machine.frames, None);
        assert_eq!(machine.qmp, None);
        assert_eq!(machine.folder, store.join("win").display().to_string());
    }

    #[test]
    fn reads_screen_and_input() {
        let store = scratch();
        add_machine(
            &store,
            "win",
            r#"{"name": "W", "kind": "windows", "cores": 4, "memory_mb": 8192, "disk": "/d.img",
                "frames": "rmhost.frames", "qmp": "/run/rmhost-qmp.sock"}"#,
        );
        let listing = list(&store).unwrap();
        let machine = &listing.machines[0];
        assert_eq!(machine.frames.as_deref(), Some("rmhost.frames"));
        assert_eq!(machine.qmp.as_deref(), Some("/run/rmhost-qmp.sock"));
        let json = serde_json::to_value(&listing).unwrap();
        assert_eq!(json["machines"][0]["frames"], "rmhost.frames");
        assert_eq!(json["machines"][0]["qmp"], "/run/rmhost-qmp.sock");
    }

    #[test]
    fn missing_screen_and_input_cross_as_null() {
        let store = scratch();
        add_machine(&store, "win", &settings("Win"));
        let json = serde_json::to_value(list(&store).unwrap()).unwrap();
        assert!(json["machines"][0]["frames"].is_null());
        assert!(json["machines"][0]["qmp"].is_null());
    }

    #[test]
    fn sorts_by_name() {
        let store = scratch();
        add_machine(&store, "a", &settings("Zed"));
        add_machine(&store, "b", &settings("Alpha"));
        let names: Vec<_> = list(&store).unwrap().machines.into_iter().map(|m| m.name).collect();
        assert_eq!(names, ["Alpha", "Zed"]);
    }

    #[test]
    fn skips_broken_and_empty_folders_but_keeps_the_rest() {
        let store = scratch();
        add_machine(&store, "good", &settings("Good"));
        add_machine(&store, "broken", "{ not json");
        add_machine(&store, "short", r#"{"name": "Short"}"#);
        fs::create_dir_all(store.join("empty")).unwrap();
        let listing = list(&store).unwrap();
        assert_eq!(listing.machines.len(), 1);
        assert_eq!(listing.machines[0].name, "Good");
        let skipped: Vec<_> = listing.skipped.iter().map(|s| s.folder.clone()).collect();
        let expected: Vec<_> =
            ["broken", "empty", "short"].iter().map(|f| store.join(f).display().to_string()).collect();
        assert_eq!(skipped, expected);
        assert!(listing.skipped[1].reason.starts_with("cannot read"));
    }

    #[test]
    fn ignores_plain_files() {
        let store = scratch();
        fs::write(store.join(".DS_Store"), b"").unwrap();
        assert_eq!(list(&store).unwrap(), Listing::default());
    }

    #[test]
    fn listing_serialises_with_state_in_lowercase() {
        let store = scratch();
        add_machine(&store, "win", &settings("Win"));
        let json = serde_json::to_value(list(&store).unwrap()).unwrap();
        assert_eq!(json["machines"][0]["state"], "off");
        assert_eq!(json["skipped"], serde_json::json!([]));
    }

    #[test]
    fn an_installer_kernel_is_read_only_with_both_its_files() {
        let store = scratch();
        let machines = store.join("Machines");
        let file = |extra: &str| {
            format!(
                r#"{{"name": "U", "kind": "linux", "cores": 4, "memory_mb": 3072, "disk": "disk.img",
                    "firmware_code": "code.fd", "firmware_vars": "vars.fd"{extra}}}"#
            )
        };
        add_machine(
            &machines,
            "u",
            &file(
                r#", "installer_kernel": "boot/vmlinuz", "installer_initrd": "boot/initrd",
                    "installer_command_line": "autoinstall ---""#,
            ),
        );
        add_machine(&machines, "v", &file(r#", "installer_kernel": "boot/vmlinuz""#));
        let listing = list(&machines).unwrap().machines;
        let boot = listing[0].launch.as_ref().unwrap().installer_boot.clone();
        assert_eq!(
            boot,
            Some(InstallerBoot {
                kernel: machines.join("u/boot/vmlinuz"),
                initrd: machines.join("u/boot/initrd"),
                command_line: "autoinstall ---".into(),
            })
        );
        assert_eq!(listing[1].launch.as_ref().unwrap().installer_boot, None);
    }

    #[test]
    fn a_machine_with_firmware_files_can_be_started_and_gets_its_screen_and_input_names() {
        let store = scratch();
        add_machine(
            &store.join("Machines"),
            "win",
            r#"{"name": "W", "kind": "windows", "cores": 4, "memory_mb": 6144, "disk": "win.img",
                "firmware_code": "code.fd", "firmware_vars": "/abs/vars.fd",
                "stick": "stick.img", "option_rom": "pvgfb.rom", "modes": ["1024x768@60"]}"#,
        );
        let machines = store.join("Machines");
        let machine = list(&machines).unwrap().machines.remove(0);
        assert!(machine.startable);
        assert_eq!(machine.id, "win");
        let run = store.join("Run/win");
        assert_eq!(machine.frames.as_deref(), Some("guesthouse.win"));
        assert_eq!(machine.qmp, Some(run.join("input.sock").display().to_string()));
        assert_eq!(machine.modes, ["1024x768@60"]);
        let launch = machine.launch.unwrap();
        // Relative names are inside the machine's folder, absolute ones stay as they are.
        assert_eq!(launch.disk, machines.join("win/win.img"));
        assert_eq!(launch.firmware_code, machines.join("win/code.fd"));
        assert_eq!(launch.firmware_vars, PathBuf::from("/abs/vars.fd"));
        assert_eq!(launch.stick, Some(machines.join("win/stick.img")));
        assert_eq!(launch.option_rom, Some(machines.join("win/pvgfb.rom")));
        assert_eq!(launch.modes, ["1024x768@60"]);
        assert_eq!(launch.control_socket, run.join("qmp.sock"));
        assert_eq!(launch.cores, 4);
    }

    #[test]
    fn modes_default_to_run_sh_s_list_and_written_screen_names_win() {
        let store = scratch();
        let machines = store.join("Machines");
        add_machine(
            &machines,
            "win",
            r#"{"name": "W", "kind": "windows", "cores": 1, "memory_mb": 1, "disk": "d",
                "firmware_code": "c", "firmware_vars": "v", "frames": "mine.frames"}"#,
        );
        let machine = list(&machines).unwrap().machines.remove(0);
        assert_eq!(machine.frames.as_deref(), Some("mine.frames"));
        let modes = machine.launch.unwrap().modes;
        assert_eq!(modes, ["640x480@60", "1024x768@60", "1280x800@60", "1920x1080@60"]);
    }

    #[test]
    fn a_linux_machine_starts_largest_first_and_then_at_its_window_s_last_mode() {
        let store = scratch();
        let machines = store.join("Machines");
        add_machine(
            &machines,
            "deb",
            r#"{"name": "D", "kind": "linux", "cores": 1, "memory_mb": 1, "disk": "d",
                "firmware_code": "c", "firmware_vars": "v"}"#,
        );
        let modes = |machines: &Path| list(machines).unwrap().machines.remove(0).launch.unwrap().modes;
        assert_eq!(modes(&machines), ["1920x1080@60", "1280x800@60", "1024x768@60", "640x480@60"]);
        let folder = machines.join("deb");
        remember_screen_mode(&folder, "linux", "2560x1416@60").unwrap();
        assert_eq!(
            modes(&machines),
            ["2560x1416@60", "1920x1080@60", "1280x800@60", "1024x768@60", "640x480@60"]
        );
        // Behind a smaller one only what is no larger, once: the guest may take the largest.
        remember_screen_mode(&folder, "linux", "1280x800@60").unwrap();
        assert_eq!(modes(&machines), ["1280x800@60", "1024x768@60", "640x480@60"]);
        remember_screen_mode(&folder, "linux", "1600x700@60").unwrap();
        assert_eq!(modes(&machines), ["1600x700@60", "640x480@60"]);
        // What is no mode is neither written nor read.
        remember_screen_mode(&folder, "linux", "banana").unwrap();
        assert_eq!(modes(&machines)[0], "1600x700@60");
        fs::write(folder.join(SCREEN_MODE_FILE), "99999x1@60\n").unwrap();
        assert_eq!(modes(&machines)[0], "1920x1080@60");
    }

    #[test]
    fn a_windows_machine_keeps_run_sh_s_modes_and_no_screen_mode_file() {
        let store = scratch();
        let folder = store.join("win");
        fs::create_dir_all(&folder).unwrap();
        remember_screen_mode(&folder, "windows", "2560x1416@60").unwrap();
        assert!(!folder.join(SCREEN_MODE_FILE).exists());
        assert_eq!(default_modes(&folder, "windows"), DEFAULT_MODES);
    }

    #[test]
    fn a_machine_with_only_one_firmware_file_is_not_startable() {
        let store = scratch();
        add_machine(
            &store,
            "half",
            r#"{"name": "H", "kind": "windows", "cores": 1, "memory_mb": 1, "disk": "d", "firmware_code": "c"}"#,
        );
        let machine = list(&store).unwrap().machines.remove(0);
        assert!(!machine.startable && machine.launch.is_none() && machine.frames.is_none());
    }

    #[test]
    fn find_takes_a_folder_name_and_nothing_that_climbs_out() {
        let store = scratch();
        add_machine(&store, "win", &settings("Win"));
        assert_eq!(find(&store, "win").unwrap().name, "Win");
        assert!(find(&store, "nobody").is_err());
        assert!(find(&store, "../win").is_err());
        assert!(find(&store, "").is_err());
        assert!(find(&store, ".hidden").is_err());
    }
}
