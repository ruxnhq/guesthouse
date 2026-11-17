//! Tells Windows to show programs that draw in a window without a copy of every frame, or takes that
//! back. Windows does this by itself only for what it counts as a game, and the copy can halve the
//! frame rate of anything else. It goes by registry entries the guest's script writes, and the setting
//! is the machine's, so it's put right at every start: a guest from an older version gets the entries,
//! a folder made since gets one, and with the setting off the script removes what it wrote.

use super::Task;
use crate::agent::guest_script;
use crate::failure::Result;
use crate::log::write_log;

const SCRIPT_NAME: &str = "window-speed.ps1";
const SCRIPT_SECONDS: u64 = 30;
const KEPT_MARK: &str = "kept";
const FAILED_MARK: &str = "FAILED";

/// The script with the machine's setting above it, as the guest runs it.
fn with_setting(script: &str, on: bool) -> String {
    format!("$on = ${on}\n{script}")
}

/// Windows is up by now, so anything that goes wrong here is said and doesn't stop the machine.
pub(super) fn apply_window_speed(task: &mut Task) -> Result<()> {
    let script = match guest_script(task.context, SCRIPT_NAME) {
        Ok(script) => script,
        Err(failure) => {
            write_log(task.context, &format!("window speed: {}", failure.0))?;
            return Ok(());
        }
    };
    let on = task.machine.window_speed();
    let said = task.ask_guest(&with_setting(&script, on), SCRIPT_SECONDS);
    if said.contains(FAILED_MARK) {
        task.error = if on {
            "Windows could not be told to speed up programs in a window. The log says why.".to_owned()
        } else {
            "The entries that speed up programs in a window are still in Windows. The log says why."
                .to_owned()
        };
    }
    if !said.trim().starts_with(KEPT_MARK) {
        write_log(task.context, &format!("window speed: {}", said.trim()))?;
    }
    Ok(())
}
