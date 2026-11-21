//! A Windows guest's screen through remoteman's viewer, started with its `connect` command and
//! what the service said about the guest.

use std::path::PathBuf;
use std::process::Command;

use guesthouse_control::screen::connect_command;

/// Where the installer puts remoteman's viewer: `viewer\` beside the app's own folder, `app\`.
const VIEWER: &str = r"viewer\remoteman.exe";
/// A viewer elsewhere, for trying the app from a build.
const VIEWER_OVERRIDE: &str = "GUESTHOUSE_VIEWER";

/// What the service's answer to `Screen` gave.
pub struct Screen {
    pub address: String,
    pub port: u16,
    pub host_fingerprint: String,
    pub viewer_certificate: String,
    pub viewer_key: String,
}

/// The viewer's program: `GUESTHOUSE_VIEWER` if it's set, else the installed one.
fn viewer() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(VIEWER_OVERRIDE) {
        return Some(PathBuf::from(path));
    }
    let installed = std::env::current_exe().ok()?.parent()?.parent()?.join(VIEWER);
    installed.is_file().then_some(installed)
}

/// Opens machine `name`'s screen, or says why it didn't open.
pub fn open(name: &str, screen: &Screen) -> Result<(), String> {
    let viewer = viewer().ok_or_else(|| {
        format!("remoteman's viewer isn't installed ({VIEWER} in Guesthouse's folder), so the screen can't be shown.")
    })?;
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let state = base.join("Guesthouse").join("screen").join(name);
    let args = connect_command(
        &state,
        &screen.address,
        screen.port,
        &screen.host_fingerprint,
        &screen.viewer_certificate,
        &screen.viewer_key,
    )
    .map_err(|error| format!("The viewer's identity couldn't be written in {}: {error}", state.display()))?;
    let mut command = Command::new(&viewer);
    command.args(args);
    // It starts in its own folder, where its FFmpeg DLLs and runtime are.
    if let Some(folder) = viewer.parent() {
        command.current_dir(folder);
    }
    command.spawn().map(drop).map_err(|error| format!("remoteman's viewer didn't open: {error}"))
}
