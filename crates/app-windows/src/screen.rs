//! A Windows guest's screen through remoteman's viewer: what the service said about it written into
//! a state folder of the user's, and the viewer started with that folder.

use std::path::PathBuf;
use std::process::Command;

use guesthouse_control::screen::viewer_state;

/// Where the installer puts remoteman's viewer, beside the app.
const VIEWER: &str = r"viewer\remoteman-app.exe";
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

/// The viewer's program: `GUESTHOUSE_VIEWER` if it's set, else the one beside the app.
fn viewer() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(VIEWER_OVERRIDE) {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe().ok()?.parent()?.join(VIEWER);
    beside.is_file().then_some(beside)
}

/// Opens machine `name`'s screen, or says why it didn't open.
pub fn open(name: &str, screen: &Screen) -> Result<(), String> {
    let viewer = viewer().ok_or_else(|| {
        format!("remoteman's viewer isn't installed beside the app ({VIEWER}), so the screen can't be shown.")
    })?;
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let state = base.join("Guesthouse").join("screen").join(name);
    let target = viewer_state(
        &state,
        &screen.address,
        screen.port,
        &screen.host_fingerprint,
        &screen.viewer_certificate,
        &screen.viewer_key,
    )
    .map_err(|error| format!("The viewer's settings couldn't be written in {}: {error}", state.display()))?;
    let mut command = Command::new(&viewer);
    command.arg(&target).arg("--state").arg(&state);
    // Its FFmpeg DLLs are in its own folder, which Windows searches for an exe's DLLs anyway; the
    // folder is also where it starts, so anything it opens by a relative path is found there.
    if let Some(folder) = viewer.parent() {
        command.current_dir(folder);
    }
    command.spawn().map(drop).map_err(|error| format!("remoteman's viewer didn't open: {error}"))
}
