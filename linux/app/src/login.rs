//! Starting the app when the user logs in: a desktop file in the autostart folder, which every
//! desktop that follows the autostart specification runs.

use crate::application::APP_ID;
use gtk::glib;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

/// What the autostart file gives the app, to say it was started by a login and not by hand.
pub const AT_LOGIN: &str = "--at-login";
const EXEC: &str = "Exec=";

fn autostart_file() -> PathBuf {
    glib::user_config_dir().join("autostart").join(format!("{APP_ID}.desktop"))
}

/// How the installed launcher starts the app, which knows where the program was put. A build run
/// from where it was built has no launcher, and starts as it was started now.
fn exec_line() -> io::Result<String> {
    let launcher = glib::user_data_dir().join("applications").join(format!("{APP_ID}.desktop"));
    let installed = fs::read_to_string(launcher).ok();
    let line = installed.as_deref().and_then(|text| text.lines().find(|line| line.starts_with(EXEC)));
    match line {
        Some(line) => Ok(line.to_owned()),
        None => Ok(format!("{EXEC}\"{}\"", env::current_exe()?.display())),
    }
}

pub fn starts_at_login() -> bool {
    autostart_file().exists()
}

pub fn set_starts_at_login(wanted: bool) -> io::Result<()> {
    let file = autostart_file();
    if !wanted {
        return match fs::remove_file(file) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)?;
    }
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Guesthouse\nIcon={APP_ID}\n{} {AT_LOGIN}\nTerminal=false\n",
        exec_line()?
    );
    fs::write(file, entry)
}

/// Whether this run was started by the autostart file.
pub fn started_at_login() -> bool {
    env::args().any(|argument| argument == AT_LOGIN)
}
