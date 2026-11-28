//! Moving a guest's agent to a new token. A copy's disk knows its original's token, and in QEMU's
//! own NAT a guest reaches the host's loopback, where every machine's agent listens on a port of its
//! own. So two machines with one token could each run PowerShell in the other. After a duplicate
//! both are given a new token, and the next start of each moves its guest to it.
//!
//! The host only takes the new token once the guest has answered to it, so a change cut short at
//! any point leaves the app able to reach the guest with one of the two. A guest that refuses the
//! change is said on the machine's card and by `show`, as until it works another machine's guest can
//! reach this one's agent.
//!
//! A change is only made as a start begins, as soon as the agent first answers and before the app
//! starts any helper in the guest. The helpers (clipboard, files, window size) read the token once,
//! when they start, and a helper's PowerShell outlives `Stop-ScheduledTask`, so one started before a
//! change would go on taking the old token until Windows restarts. None is: the app starts them
//! later in the same start, and no task in the guest starts one by itself.
//!
//! A guest installed from the disc has `token.txt` read-only, as files copied from a disc are. The
//! change clears that for its write and sets it again after. The attribute guards against nothing,
//! as SYSTEM can clear it, but the change leaves the file as it found it.

use crate::context::Context;
use crate::failure::Result;
use crate::guest_end::GuestEnd;
use crate::log::write_log;
use crate::machine::Machine;
use crate::random::random_hex;
use crate::settings::with_suffix;
use rustix::fs::{FlockOperation, flock};
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The token waiting for the guest to be moved to it, beside the one in use.
const NEXT_SUFFIX: &str = ".next";
/// Beside a waiting token: once the guest has it, another change is owed. A copy made while its
/// original's change was waiting may share that waiting token with it, so both move on once more.
const AGAIN_SUFFIX: &str = ".again";
/// Beside the token in use: every look at the token files that changes them is made holding this,
/// by whichever of our processes makes it, and only for the file work, never while the guest is asked.
const LOCK_SUFFIX: &str = ".lock";
/// Beside the token: what the guest said when it last refused a change, until one succeeds.
const REFUSED_SUFFIX: &str = ".refused";
/// The most of what the guest said that goes into the note.
const MOST_SAID: usize = 200;
/// The same length as the tokens an install makes.
const TOKEN_BYTES: usize = 16;
const ASK_SECONDS: u64 = crate::agent::QUICK_SECONDS;
/// What the guest says once its file and its running agent both have the new token.
const CHANGED: &str = "token changed";
const UP_SCRIPT: &str = "'up'";
const UP: &str = "up";
/// A change and the one it owes, at most, in one go.
const MOST_CHANGES_AT_ONCE: usize = 2;
const PRIVATE: u32 = 0o600;

/// The machine's own waiting token. A throwaway's is its run's: see [`waiting_file`].
pub fn next_token_file(context: &Context, machine: &Machine) -> PathBuf {
    with_suffix(&machine.token_file(context), NEXT_SUFFIX)
}

/// The waiting token beside the token at `token_file`, as in a machine's folder that isn't in place yet.
pub fn next_token_file_at(token_file: &Path) -> PathBuf {
    with_suffix(token_file, NEXT_SUFFIX)
}

/// The token the app asks the guest with: a throwaway run's own once it has one.
pub fn token_in_use(context: &Context, machine: &Machine) -> PathBuf {
    let run = crate::throwaway::run_token_file(context, machine);
    if machine.throwaway && run.exists() { run } else { machine.token_file(context) }
}

/// The token the guest is to be moved to. A throwaway's disk never keeps a change, so its guest is
/// moved to a token of the run's own at each start, kept with the run.
fn waiting_file(context: &Context, machine: &Machine) -> PathBuf {
    if machine.throwaway {
        with_suffix(&crate::throwaway::run_token_file(context, machine), NEXT_SUFFIX)
    } else {
        next_token_file(context, machine)
    }
}

/// Held while it lives. Another of our processes waits for it, which is a few file operations.
struct Locked {
    _file: File,
}

/// The lock for the token at `token_file`, which a throwaway's run tokens share with the machine's.
fn lock(token_file: &Path) -> std::io::Result<Locked> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(PRIVATE)
        .open(with_suffix(token_file, LOCK_SUFFIX))?;
    flock(&file, FlockOperation::LockExclusive)?;
    Ok(Locked { _file: file })
}

/// The lock's file, which stays with the machine and goes in no copy.
pub const LOCK_FILE: &str = "guest.token.lock";
/// The guest's last refusal, which is this machine's own and goes in no copy either.
pub const REFUSED_FILE: &str = "guest.token.refused";
/// The mark that another change is owed after the waiting one. A copy or an import plans its own.
pub const AGAIN_FILE: &str = "guest.token.next.again";

fn read(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    Some(text.trim().to_owned()).filter(|token| !token.is_empty())
}

pub fn waiting_token(context: &Context, machine: &Machine) -> Option<String> {
    read(&waiting_file(context, machine))
}

fn write_new(path: &Path) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(PRIVATE).open(path)?;
    writeln!(file, "{}", random_hex::<TOKEN_BYTES>())
}

/// Leaves a new token for the next start to move the guest to. A machine with no token has no agent
/// to move. One already waiting stays, as the guest may have it already, and another change is owed
/// after it.
pub fn plan_token_change(context: &Context, machine: &Machine) -> Result<()> {
    plan_token_change_at(&machine.token_file(context))
}

/// The same for the token at `token_file`, as in a machine's folder that isn't in place yet.
pub fn plan_token_change_at(token_file: &Path) -> Result<()> {
    let next = with_suffix(token_file, NEXT_SUFFIX);
    if !token_file.exists() {
        return Ok(());
    }
    let _locked = lock(token_file)?;
    if next.exists() {
        fs::write(with_suffix(&next, AGAIN_SUFFIX), "")?;
        return Ok(());
    }
    Ok(write_new(&next)?)
}

/// The file first, read back, and only then the running agent's own copy, so a guest that
/// restarts halfway still has a token the host knows. The agent runs what it's sent in its own
/// scope, which is where its token is.
fn change_script(token: &str) -> String {
    format!(
        "$file = 'C:\\gpupv\\token.txt'; $item = Get-Item -LiteralPath $file -ErrorAction SilentlyContinue; \
         $wasReadOnly = $item -and $item.IsReadOnly; \
         try {{ if ($wasReadOnly) {{ $item.IsReadOnly = $false }}; \
         Set-Content -LiteralPath $file -Value '{token}' -ErrorAction Stop }} \
         finally {{ if ($wasReadOnly) {{ (Get-Item -LiteralPath $file).IsReadOnly = $true }} }}; \
         if ((Get-Content $file -Raw).Trim() -eq '{token}') {{ $token = '{token}'; '{CHANGED}' }}"
    )
}

/// The token a change script moves the guest to, as a stand-in guest reads it.
#[cfg(test)]
pub(crate) fn token_in_change(script: &str) -> Option<&str> {
    let start = script.find("token.txt")?;
    let rest = &script[start..];
    let value = &rest[rest.find("-Value '")? + "-Value '".len()..];
    value.split('\'').next()
}

fn refused_file(context: &Context, machine: &Machine) -> PathBuf {
    with_suffix(&machine.token_file(context), REFUSED_SUFFIX)
}

/// What the user is told while the guest has refused its new token and still shares the old one.
pub fn refused_note(context: &Context, machine: &Machine) -> Option<String> {
    let said = fs::read_to_string(refused_file(context, machine)).ok()?;
    waiting_token(context, machine)?;
    Some(format!(
        "Its guest refused a new agent token, so it still shares one with another machine, whose guest \
         could run commands in it. The next start tries again. The guest said: {}",
        said.trim()
    ))
}

/// The host takes the waiting token, once the guest has answered to `token`, and plans the change owed
/// after it, if one is. Another process asking the guest, such as `guesthouse-vm idle` during a
/// start, may have found the guest on it and taken it first, and planned the next one: that one is
/// not the guest's yet, so it stays waiting.
fn take_waiting(context: &Context, machine: &Machine, token: &str) -> Result<()> {
    take_waiting_after(context, machine, token, || {})
}

/// [`take_waiting`], with `meanwhile` run between the look and the change, where a test puts
/// another taker.
fn take_waiting_after(
    context: &Context,
    machine: &Machine,
    token: &str,
    meanwhile: impl FnOnce(),
) -> Result<()> {
    let _locked = lock(&machine.token_file(context))?;
    let waiting = waiting_file(context, machine);
    if read(&waiting).as_deref() != Some(token) {
        return Ok(());
    }
    meanwhile();
    let in_use = if machine.throwaway {
        crate::throwaway::run_token_file(context, machine)
    } else {
        machine.token_file(context)
    };
    fs::rename(&waiting, &in_use)?;
    let _ = fs::remove_file(refused_file(context, machine));
    let again = with_suffix(&waiting, AGAIN_SUFFIX);
    if again.exists() {
        fs::remove_file(&again)?;
        write_new(&waiting)?;
    }
    Ok(())
}

fn answers_to(context: &Context, end: &GuestEnd, token: &str) -> bool {
    context.agent.ask(end, token, UP_SCRIPT, ASK_SECONDS).contains(UP)
}

/// Moves a running guest to its waiting token, and on to the one owed after it if there is one.
/// Run as soon as its agent has answered. A throwaway run gets a token of its own here, written
/// down before the guest is asked, so a change cut short can be found. A change that doesn't
/// happen is only logged: the old token still works, and the next start tries again.
pub fn finish_token_change(context: &Context, machine: &Machine, end: &GuestEnd) {
    let run_waiting = waiting_file(context, machine);
    let run_has_one = crate::throwaway::run_token_file(context, machine).exists() || run_waiting.exists();
    if machine.throwaway
        && !run_has_one
        && machine.token_file(context).exists()
        && let Err(error) = lock(&machine.token_file(context)).and_then(|_locked| write_new(&run_waiting))
    {
        let _ = write_log(context, &format!("{}: no token for this run: {error}", machine.id));
        return;
    }
    for _ in 0..MOST_CHANGES_AT_ONCE {
        let (Some(next), Some(old)) =
            (waiting_token(context, machine), read(&token_in_use(context, machine)))
        else {
            return;
        };
        let said = context.agent.ask(end, &old, &change_script(&next), ASK_SECONDS);
        let note = if !answers_to(context, end, &next) {
            let words: String =
                said.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MOST_SAID).collect();
            if !words.is_empty() {
                let _ = fs::write(refused_file(context, machine), &words);
            }
            format!("its guest's agent did not take a new token ({words})")
        } else if let Err(failure) = take_waiting(context, machine, &next) {
            format!("the guest has its new token, which could not be kept: {failure}")
        } else {
            "its guest's agent has a new token".to_owned()
        };
        let _ = write_log(context, &format!("{}: {note}", machine.id));
    }
}

/// The token to ask with when the one in use got no answer: the waiting one, if the guest answers to
/// it, which is a change cut short after the guest had it. The host takes it then.
pub fn recovered_token(context: &Context, machine: &Machine, end: &GuestEnd) -> Option<String> {
    let next = waiting_token(context, machine)?;
    if !answers_to(context, end, &next) {
        return None;
    }
    if let Err(failure) = take_waiting(context, machine, &next) {
        let _ = write_log(
            context,
            &format!("{}: the guest's new token could not be kept: {failure}", machine.id),
        );
    }
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::GuestAgent;
    use crate::testing::{Scratch, scratch_context};
    use std::os::unix::fs::MetadataExt;
    use std::sync::{Arc, Mutex};

    /// A guest's agent: it only answers to its token, and runs the change script as the real one does.
    struct Guest {
        token: Mutex<String>,
        /// The guest goes away after taking the token, before it can answer to it.
        gone_after_change: bool,
        /// Its token.txt is read-only, as an install from the disc leaves it.
        read_only: bool,
        /// It refuses every change, as a guest whose file can't be written does.
        refuses: bool,
        /// What another process does once the guest has taken a token.
        meanwhile: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        gone: Mutex<bool>,
        asked: Mutex<Vec<String>>,
    }

    impl Guest {
        fn with(token: &str, gone_after_change: bool) -> Arc<Self> {
            Self::made(token, gone_after_change, false, false)
        }

        fn made(token: &str, gone_after_change: bool, read_only: bool, refuses: bool) -> Arc<Self> {
            Arc::new(Self {
                token: Mutex::new(token.to_owned()),
                gone_after_change,
                read_only,
                refuses,
                meanwhile: Mutex::default(),
                gone: Mutex::new(false),
                asked: Mutex::default(),
            })
        }
    }

    impl GuestAgent for Guest {
        fn ask(&self, _end: &crate::guest_end::GuestEnd, token: &str, script: &str, _seconds: u64) -> String {
            self.asked.lock().unwrap().push(script.to_owned());
            if *self.gone.lock().unwrap() || *self.token.lock().unwrap() != token {
                return String::new();
            }
            if let Some(new) = token_in_change(script) {
                if self.refuses || (self.read_only && !script.contains("$item.IsReadOnly = $false")) {
                    return "ERROR: Access to the path 'C:\\gpupv\\token.txt' is denied.\n".to_owned();
                }
                *self.token.lock().unwrap() = new.to_owned();
                *self.gone.lock().unwrap() = self.gone_after_change;
                let meanwhile = self.meanwhile.lock().unwrap().take();
                if let Some(meanwhile) = meanwhile {
                    meanwhile();
                }
                return format!("{CHANGED}\n");
            }
            if script == "Quiet-Thing" {
                return String::new();
            }
            "up\n".to_owned()
        }
    }

    fn windows_machine(scratch: &Scratch, context: &Context, token: &str) -> Machine {
        let machine = Machine::new("win", "Win", &scratch.path("d").display().to_string(), "/v");
        machine.save(context).unwrap();
        fs::write(machine.token_file(context), format!("{token}\n")).unwrap();
        machine
    }

    #[test]
    fn the_guest_moves_first_and_then_the_host() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let guest = Guest::with("old", false);
        context.agent = guest.clone();
        let machine = windows_machine(&scratch, &context, "old");
        plan_token_change(&context, &machine).unwrap();
        let next = waiting_token(&context, &machine).unwrap();
        assert_eq!(fs::metadata(next_token_file(&context, &machine)).unwrap().mode() & 0o777, 0o600);
        assert_ne!(next, "old");
        finish_token_change(&context, &machine, &crate::guest_end::GuestEnd::agent(&context, &machine, None));
        assert_eq!(*guest.token.lock().unwrap(), next);
        assert_eq!(fs::read_to_string(machine.token_file(&context)).unwrap().trim(), next);
        plan_token_change(&context, &machine).unwrap();
        let again = waiting_token(&context, &machine).unwrap();
        plan_token_change(&context, &machine).unwrap();
        assert_eq!(waiting_token(&context, &machine).unwrap(), again, "one already waiting stays");
        finish_token_change(&context, &machine, &crate::guest_end::GuestEnd::agent(&context, &machine, None));
        let last = guest.token.lock().unwrap().clone();
        assert!(last != again && last != next, "and another is owed after it");
        assert_eq!(fs::read_to_string(machine.token_file(&context)).unwrap().trim(), last);
        assert!(!next_token_file(&context, &machine).exists());
        assert!(!machine.folder(&context).join(AGAIN_FILE).exists(), "the mark goes once its change is done");
        assert_eq!(context.ask_guest(&machine, "'up'", 1, None), "up\n");
    }

    #[test]
    fn a_guest_whose_token_file_is_read_only_still_moves() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let guest = Guest::made("old", false, true, false);
        context.agent = guest.clone();
        let machine = windows_machine(&scratch, &context, "old");
        plan_token_change(&context, &machine).unwrap();
        let next = waiting_token(&context, &machine).unwrap();
        finish_token_change(&context, &machine, &crate::guest_end::GuestEnd::agent(&context, &machine, None));
        assert_eq!(*guest.token.lock().unwrap(), next);
        assert_eq!(fs::read_to_string(machine.token_file(&context)).unwrap().trim(), next);
        let script = change_script("x");
        let cleared = script.find("$item.IsReadOnly = $false").unwrap();
        let set_again = script
            .find("finally { if ($wasReadOnly) { (Get-Item -LiteralPath $file).IsReadOnly = $true")
            .unwrap();
        assert!(cleared < script.find("Set-Content").unwrap() && set_again > cleared, "left as it was found");
    }

    #[test]
    fn a_refused_change_is_told_until_one_succeeds() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.agent = Guest::made("old", false, false, true);
        let machine = windows_machine(&scratch, &context, "old");
        plan_token_change(&context, &machine).unwrap();
        assert_eq!(refused_note(&context, &machine), None, "nothing is said before it is tried");
        finish_token_change(&context, &machine, &crate::guest_end::GuestEnd::agent(&context, &machine, None));
        let note = refused_note(&context, &machine).unwrap();
        assert!(note.starts_with("Its guest refused a new agent token"), "{note}");
        assert!(note.ends_with("ERROR: Access to the path 'C:\\gpupv\\token.txt' is denied."), "{note}");
        context.agent = Guest::with("old", false);
        finish_token_change(&context, &machine, &crate::guest_end::GuestEnd::agent(&context, &machine, None));
        assert_eq!(refused_note(&context, &machine), None);
        assert!(!machine.folder(&context).join(REFUSED_FILE).exists());
    }

    #[test]
    fn a_guest_that_does_not_take_it_keeps_the_old_one_and_the_host_too() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let guest = Guest::with("other", false);
        context.agent = guest.clone();
        let machine = windows_machine(&scratch, &context, "old");
        plan_token_change(&context, &machine).unwrap();
        finish_token_change(&context, &machine, &crate::guest_end::GuestEnd::agent(&context, &machine, None));
        assert_eq!(fs::read_to_string(machine.token_file(&context)).unwrap().trim(), "old");
        assert!(waiting_token(&context, &machine).is_some(), "the next start tries again");
    }

    #[test]
    fn a_change_cut_short_is_found_by_the_next_question() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let guest = Guest::with("old", true);
        context.agent = guest.clone();
        let machine = windows_machine(&scratch, &context, "old");
        plan_token_change(&context, &machine).unwrap();
        let next = waiting_token(&context, &machine).unwrap();
        finish_token_change(&context, &machine, &crate::guest_end::GuestEnd::agent(&context, &machine, None));
        assert_eq!(fs::read_to_string(machine.token_file(&context)).unwrap().trim(), "old");
        *guest.gone.lock().unwrap() = false;
        guest.asked.lock().unwrap().clear();
        assert_eq!(context.ask_guest(&machine, "Get-Thing", 1, None), "up\n");
        assert_eq!(fs::read_to_string(machine.token_file(&context)).unwrap().trim(), next);
        let asked = guest.asked.lock().unwrap().clone();
        assert_eq!(asked.iter().filter(|script| *script == "Get-Thing").count(), 2, "{asked:?}");
    }

    #[test]
    fn a_script_is_never_run_twice_by_a_guest_that_has_the_token_in_use() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let guest = Guest::with("old", false);
        context.agent = guest.clone();
        let machine = windows_machine(&scratch, &context, "old");
        plan_token_change(&context, &machine).unwrap();
        // A script that prints nothing gets no answer, which looks like an agent that isn't up.
        guest.asked.lock().unwrap().clear();
        context.ask_guest(&machine, "Quiet-Thing", 1, None);
        let asked = guest.asked.lock().unwrap().clone();
        assert_eq!(asked.iter().filter(|script| *script == "Quiet-Thing").count(), 1, "{asked:?}");
        assert!(waiting_token(&context, &machine).is_some());
    }

    #[test]
    fn a_question_from_elsewhere_during_a_change_leaves_the_host_on_the_guests_token() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let guest = Guest::with("old", false);
        context.agent = guest.clone();
        let machine = windows_machine(&scratch, &context, "old");
        plan_token_change(&context, &machine).unwrap();
        // A copy made while this change waited: another change is owed after it.
        plan_token_change(&context, &machine).unwrap();
        let (elsewhere, asked) = (context.clone(), machine.clone());
        *guest.meanwhile.lock().unwrap() = Some(Box::new(move || {
            // As `guesthouse-vm idle` asks: the token in use gets no answer, so the waiting one is tried.
            assert_eq!(elsewhere.ask_guest(&asked, "'up'", 1, None), "up\n");
        }));
        finish_token_change(&context, &machine, &crate::guest_end::GuestEnd::agent(&context, &machine, None));
        let guests = guest.token.lock().unwrap().clone();
        assert_eq!(fs::read_to_string(machine.token_file(&context)).unwrap().trim(), guests);
        assert_eq!(context.ask_guest(&machine, "'up'", 1, None), "up\n");
    }

    #[test]
    fn two_takers_at_once_leave_the_host_on_the_token_the_guest_has() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let guest = Guest::with("old", false);
        context.agent = guest.clone();
        let machine = windows_machine(&scratch, &context, "old");
        plan_token_change(&context, &machine).unwrap();
        plan_token_change(&context, &machine).unwrap();
        let first = waiting_token(&context, &machine).unwrap();
        // The guest has moved, and both of our processes have seen it answer to the waiting token.
        *guest.token.lock().unwrap() = first.clone();
        let (other, asked, token) = (context.clone(), machine.clone(), first.clone());
        let mut second = None;
        take_waiting_after(&context, &machine, &first, || {
            second = Some(std::thread::spawn(move || take_waiting(&other, &asked, &token).unwrap()));
            std::thread::sleep(std::time::Duration::from_millis(300));
        })
        .unwrap();
        second.unwrap().join().unwrap();
        assert_eq!(fs::read_to_string(machine.token_file(&context)).unwrap().trim(), first);
        assert_eq!(context.ask_guest(&machine, "'up'", 1, None), "up\n");
        assert!(waiting_token(&context, &machine).is_some_and(|next| next != first), "the one owed waits");
    }

    #[test]
    fn a_machine_with_no_token_has_nothing_to_change() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = Machine::new("plain", "Plain", "/d", "/v");
        machine.save(&context).unwrap();
        plan_token_change(&context, &machine).unwrap();
        assert!(!next_token_file(&context, &machine).exists());
    }

    #[test]
    fn a_copy_of_a_change_cut_short_reaches_its_guest_and_moves_on_from_the_shared_token() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let guest = Guest::with("middle", false);
        context.agent = guest.clone();
        let machine = windows_machine(&scratch, &context, "old");
        fs::write(next_token_file(&context, &machine), "middle\n").unwrap();
        plan_token_change(&context, &machine).unwrap();
        assert_eq!(waiting_token(&context, &machine).unwrap(), "middle", "the waiting one stays");
        finish_token_change(&context, &machine, &crate::guest_end::GuestEnd::agent(&context, &machine, None));
        let now = guest.token.lock().unwrap().clone();
        assert!(now != "middle" && now != "old", "{now}");
        assert_eq!(fs::read_to_string(machine.token_file(&context)).unwrap().trim(), now);
        assert!(waiting_token(&context, &machine).is_none());
    }

    #[test]
    fn a_throwaway_runs_change_cut_short_is_found_by_the_next_question() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let guest = Guest::with("base", true);
        context.agent = guest.clone();
        let mut machine = windows_machine(&scratch, &context, "base");
        machine.throwaway = true;
        fs::create_dir_all(crate::throwaway::run_folder(&context, &machine)).unwrap();
        finish_token_change(&context, &machine, &crate::guest_end::GuestEnd::agent(&context, &machine, None));
        let run_token = guest.token.lock().unwrap().clone();
        assert_ne!(run_token, "base");
        *guest.gone.lock().unwrap() = false;
        assert_eq!(context.ask_guest(&machine, "'up'", 1, None), "up\n");
        let run = crate::throwaway::run_token_file(&context, &machine);
        assert_eq!(fs::read_to_string(run).unwrap().trim(), run_token);
        assert_eq!(fs::read_to_string(machine.token_file(&context)).unwrap().trim(), "base");
    }
}
