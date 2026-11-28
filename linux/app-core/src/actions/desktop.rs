//! The guest's desktop once it's up: the boots seen so far, its size on the basic display and its shared
//! folder.

use super::Task;
use super::display::set_up_display;
use crate::agent::guest_script;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::text::base64_encode;
use guesthouse_daemon::slot::Slot;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// The firmware writes one of these to the serial log each boot.
pub(super) const BOOT_LINE: &[u8] = b"BdsDxe: starting";
pub(super) const BOOTS_FILE: &str = "guesthouse-boots";
pub(super) const SERIAL_FILE: &str = "serial.log";
const SHARE_SCRIPT_SECONDS: u64 = 40;
/// Where `share-start.ps1` has the share's address, which holds the key it is served under.
const ADDRESS_MARK: &str = "@SHARE_ADDRESS@";
/// Where the guest keeps what the app puts there, and the account its desktop runs as.
pub(super) const GUEST_FOLDER: &str = "C:\\gpupv";
const UPLOAD_SECONDS: u64 = 60;
const PUT_MARK: &str = "file placed";

/// A script for the user's session: its name, and how many seconds it gets.
#[derive(Clone, Copy)]
pub(super) struct SessionScript {
    pub name: &'static str,
    pub patience: u32,
    pub answer_seconds: u64,
}

/// What the guest says about the account it picked, so the app's log shows whose desktop was asked.
const CONSOLE_USER_SAID: &str = "\"session user $u\";";
const DISPLAY_SCRIPT: SessionScript = SessionScript { name: "display", patience: 25, answer_seconds: 40 };
const SET_MODE_SCRIPT: SessionScript = SessionScript { name: "setmode", patience: 40, answer_seconds: 55 };
const DESKTOP_ATTEMPTS: u32 = 60;
const DESKTOP_POLL: Duration = Duration::from_secs(4);
const DESKTOP_SETTLE: Duration = Duration::from_secs(8);
const DESKTOP_ANSWER_SECONDS: u64 = 6;
/// The processes that mean a user's session is really up.
pub(super) const DESKTOP_SCRIPT: &str =
    "(Get-Process explorer,ctfmon,RuntimeBroker -ea 0 | Select -Unique Name).Count";
const DESKTOP_PROCESSES: u32 = 3;
/// Says so when nobody is signed in at Windows' console. A guest that can't say isn't taken to be
/// waiting for anyone.
const SIGNED_OUT_SCRIPT: &str =
    "$s=Get-CimInstance Win32_ComputerSystem -ea 0; if($s -and -not $s.UserName){'nobody signed in'}";
const SIGNED_OUT_MARK: &str = "nobody signed in";
const SIGNED_OUT_ANSWER_SECONDS: u64 = 20;
/// What a start that found nobody signed in tells the user.
pub(super) const START_SIGN_IN_NOTE: &str = "Windows is waiting for someone to sign in, so the VM was left \
     on. Sign in on its screen or by Remote Desktop. Its display and shared folder are set up once someone has.";
/// The same, after Windows restarted by itself.
pub(super) const RESTART_SIGN_IN_NOTE: &str = "Windows restarted and is waiting for someone to sign in. Its \
     display and shared folder are set up once someone has.";

/// How many times the guest has booted in this VM run.
pub fn boots(context: &Context, slot: Option<Slot>) -> usize {
    let data = fs::read(context.run_folder(slot).join(SERIAL_FILE)).unwrap_or_default();
    count(&data, BOOT_LINE)
}

pub(super) use guesthouse_serial_words_core::linux_host::count;

/// Whether Windows has restarted since the app last set the rate, which only lasts one boot. A VM
/// the app never set up has no record here, and is left as it is.
pub fn rebooted(context: &Context, slot: Option<Slot>) -> bool {
    let noted = fs::read_to_string(context.run_folder(slot).join(BOOTS_FILE)).unwrap_or_default();
    noted.trim().parse::<usize>().is_ok_and(|noted| boots(context, slot) > noted)
}

pub(super) fn note_boot(task: &Task) -> Result<()> {
    fs::write(task.run_file(BOOTS_FILE), boots(task.context, None).to_string())?;
    Ok(())
}

/// Copies a file into the guest, and says whether it got there.
pub(super) fn put_file(task: &Task, local: &Path, guest: &str) -> Result<bool> {
    let data = base64_encode(&fs::read(local)?);
    let script =
        format!("[IO.File]::WriteAllBytes('{guest}', [Convert]::FromBase64String('{data}')); '{PUT_MARK}'");
    Ok(task.ask_guest(&script, UPLOAD_SECONDS).contains(PUT_MARK))
}

/// Copies a file from the program folder into the guest.
pub(super) fn put_guest_file(task: &Task, local: &str, guest: &str) -> Result<()> {
    put_file(task, &task.context.paths.guest_files().join(local), guest)?;
    Ok(())
}

/// Runs a script this app holds as the logged-in user.
pub(super) fn run_in_session_text(task: &Task, script: SessionScript, text: &str) -> Result<String> {
    let data = base64_encode(text.as_bytes());
    let file = format!("{GUEST_FOLDER}\\{}.ps1", script.name);
    task.ask_guest(
        &format!("[IO.File]::WriteAllBytes('{file}', [Convert]::FromBase64String('{data}'))"),
        UPLOAD_SECONDS,
    );
    run_placed_script(task, script)
}

/// Runs one of the app's scripts as the logged-in user, as session 0 has no display, and returns what
/// it wrote. It's run as a scheduled task, which is the agent's way into that session.
fn run_in_session(task: &Task, script: SessionScript) -> Result<String> {
    let file = format!("{GUEST_FOLDER}\\{}.ps1", script.name);
    put_guest_file(task, &format!("{}.ps1", script.name), &file)?;
    run_placed_script(task, script)
}

/// Names the account at the console in `$u`, as Windows sees it, with the machine's own account when
/// nobody is signed in. A task for an account Windows hasn't got is registered and never runs, and a
/// machine's record can name one: the install made another, or the account was renamed in Windows.
/// The console user is the one whose desktop a session script has to reach, so it comes first.
pub(super) fn console_user_script(task: &Task) -> String {
    let user = task.machine.account();
    format!(
        "$u=(Get-CimInstance Win32_ComputerSystem -ea 0).UserName; if(-not $u){{$u='{user}'}}; {CONSOLE_USER_SAID}"
    )
}

/// What starts a session script. A console program started on its own gets a window from the
/// default terminal, which is Windows Terminal on Windows 11, and the script waits on that window.
/// While the desktop draws nothing it never comes, so the script hangs. A console with no window
/// needs nothing from the desktop.
const SESSION_HOST: &str = "conhost.exe";

/// The session host's arguments: PowerShell in a console with no window, running `file`.
fn session_arguments(file: &str) -> String {
    format!("--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File {file}")
}

/// The script is in the guest's folder by now.
fn run_placed_script(task: &Task, script: SessionScript) -> Result<String> {
    let SessionScript { name, patience, answer_seconds } = script;
    let user = console_user_script(task);
    let (file, result) = (format!("{GUEST_FOLDER}\\{name}.ps1"), format!("{GUEST_FOLDER}\\cap\\{name}.txt"));
    let arguments = session_arguments(&file);
    // The session can't make files in the guest's folder, so SYSTEM makes its result, empty, and gives
    // that account alone the right to read and write it.
    let run = format!(
        "{user} [IO.File]::WriteAllText('{result}', ''); icacls '{result}' /grant \"${{u}}:(R,W)\" | Out-Null; \
         $a=New-ScheduledTaskAction -Execute {SESSION_HOST} -Argument \"{arguments}\"; \
         $p=New-ScheduledTaskPrincipal -UserId $u -LogonType Interactive; \
         Register-ScheduledTask -TaskName gh{name} -Action $a -Principal $p -Force | Out-Null; \
         Start-ScheduledTask gh{name}; $t=0; \
         while((Get-Item {result}).Length -eq 0 -and $t -lt {patience}){{Start-Sleep 1;$t++}}; \
         Unregister-ScheduledTask gh{name} -Confirm:$false; Get-Content {result}"
    );
    Ok(task.ask_guest(&run, answer_seconds))
}

/// Asks the logged-in session, because Windows can stretch an old size over the screen.
fn desktop_size(task: &Task) -> Result<Option<String>> {
    let said = run_in_session(task, DISPLAY_SCRIPT)?;
    let rest = said.split("screen ").nth(1).unwrap_or("");
    let size: String = rest.chars().take_while(|letter| letter.is_ascii_digit() || *letter == 'x').collect();
    Ok(size.contains('x').then_some(size))
}

/// For the basic display, whose size comes from the VGA memory.
pub(super) fn apply_screen(task: &mut Task) -> Result<()> {
    let wanted = task.machine.basic_screen().to_owned();
    task.step("Checking the desktop size");
    if desktop_size(task)?.as_deref() == Some(&wanted) {
        return Ok(());
    }
    task.step(&format!("Setting the desktop to {wanted}"));
    run_in_session(task, SET_MODE_SCRIPT)?;
    let size = desktop_size(task)?;
    if size.as_deref() != Some(&wanted) {
        let said = size.as_deref().unwrap_or("an unknown size");
        task.error = format!("The desktop is {said}, not {wanted}.");
    }
    Ok(())
}

pub fn apply_share(task: &mut Task) -> Result<()> {
    if !task.machine.share.is_empty() {
        task.step("Connecting the shared folder");
        let key = crate::agent::share_key(task.context, &task.machine)?;
        let address = crate::agent::share_address(&key);
        let script = guest_script(task.context, "share-start.ps1")?.replace(ADDRESS_MARK, &address);
        if !task.ask_guest(&script, SHARE_SCRIPT_SECONDS).replace("not mapped", "").contains("mapped") {
            task.error = "The shared folder did not reach the guest.".to_owned();
        }
    }
    Ok(())
}

/// Waits until the guest's desktop is really up: its processes running and a screen in the session.
pub(super) fn wait_for_desktop(task: &Task) -> bool {
    (0..DESKTOP_ATTEMPTS).any(|_| desktop_up(task))
}

/// One look, after a pause, at whether the desktop is up. One that is gets a moment to settle.
pub(super) fn desktop_up(task: &Task) -> bool {
    task.sleep(DESKTOP_POLL);
    let said = task.ask_guest(DESKTOP_SCRIPT, DESKTOP_ANSWER_SECONDS);
    let count = said.lines().find_map(|line| line.trim().parse::<u32>().ok());
    let up = count.is_some_and(|count| count >= DESKTOP_PROCESSES);
    if up {
        task.sleep(DESKTOP_SETTLE);
    }
    up
}

/// Whether Windows answers and says nobody is signed in at its console.
fn nobody_signed_in(task: &Task) -> bool {
    let said = task.ask_guest(SIGNED_OUT_SCRIPT, SIGNED_OUT_ANSWER_SECONDS);
    said.lines().any(|line| line.trim() == SIGNED_OUT_MARK)
}

/// The wait for the desktop in a start, or after a restart of Windows. A Windows that answers with
/// nobody signed in is up, only waiting for a password or PIN: the work that needs no session goes on,
/// and the desktop's own setup waits for someone to sign in (see [`super::sign_in`]). A guest that
/// doesn't answer, or has someone signed in and still no desktop, fails with `never_came`.
pub(super) fn wait_for_windows(task: &mut Task, never_came: &str) -> Result<()> {
    if wait_for_desktop(task) {
        task.desktop_owed = false;
        return Ok(());
    }
    if nobody_signed_in(task) {
        task.desktop_owed = true;
        let _ = crate::log::write_log(task.context, "desktop: Windows is up, and nobody has signed in");
        return Ok(());
    }
    Err(Failure::new(never_came))
}

pub(super) fn set_up_desktop(task: &mut Task) -> Result<()> {
    note_boot(task)?;
    set_up_display(task);
    apply_share(task)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_script_runs_in_a_console_with_no_window() {
        assert_eq!(SESSION_HOST, "conhost.exe");
        assert_eq!(
            session_arguments(r"C:\gpupv\display.ps1"),
            r"--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\gpupv\display.ps1"
        );
    }
}
