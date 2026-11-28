//! The starter and the elevated part (src/main.rs says how a run is three processes).

use std::path::{Path, PathBuf};

use crate::archive::Archive;
use crate::engine::{self, Ctx, Event};
use crate::ui::{EXIT_CANCELLED, EXIT_DONE, EXIT_FAILED, EXIT_OPEN_APP};
use crate::{Args, log, system};

/// What the user opened: hands over to the elevated part, waits, and does what is left to do
/// as the user (open the app). Its exit code: 0 done, 1 failed, 2 cancelled.
pub fn starter(args: &Args) -> i32 {
    let me = std::env::current_exe().expect("this exe");
    let install_dir = system::install_dir();
    let inside = from_install_folder(args, &me);
    log::say(&format!(
        "started: {} {}{}",
        me.display(),
        args.forwarded().join(" "),
        if inside { " (from the install folder)" } else { "" }
    ));
    let fail = |text: &str| -> i32 {
        log::say(text);
        if !args.silent {
            system::message(text);
        }
        EXIT_FAILED
    };
    if let Err(e) = Archive::open(&me) {
        return fail(&format!("This file can't install or remove Guesthouse: {e}"));
    }

    let mut child_args = vec!["--stage2".to_string()];
    child_args.extend(args.forwarded());
    let verb = if args.dry_run { "open" } else { "runas" };
    system::allow_foreground();
    let child = match system::shell_start(&me, &child_args, verb, &system::windows_dir()) {
        Ok(system::Launched::Process(p)) => p,
        Ok(system::Launched::Cancelled) => {
            log::say("the UAC prompt was answered No: nothing was done");
            return if args.silent { EXIT_FAILED } else { EXIT_CANCELLED };
        }
        Err(e) => return fail(&format!("Guesthouse setup couldn't start: {e}")),
    };

    // The uninstaller, opened from Settings, doesn't wait: it would keep the folder open. A
    // silent one does, for its exit code (its file is moved out of the folder and deleted at
    // restart instead).
    if inside && !args.silent {
        log::say("handed over; the starter ends");
        return EXIT_DONE;
    }
    let code = child.wait() as i32;
    drop(child);
    log::say(&format!("the elevated part ended: {code}"));
    match code {
        EXIT_OPEN_APP => {
            open_app(&install_dir, args.dry_run);
            EXIT_DONE
        }
        EXIT_DONE | EXIT_CANCELLED => code,
        _ => EXIT_FAILED,
    }
}

/// Whether `exe` is in the install folder (the uninstaller). In a dry run, the dry run's install
/// folder counts as the install folder, so that its uninstaller copies itself out as the real
/// one does.
fn from_install_folder(args: &Args, exe: &Path) -> bool {
    exe.parent().is_some_and(|d| {
        system::same_path(d, &system::install_dir())
            || (args.dry_run && system::same_path(d, &engine::dry_run_dir()))
    })
}

/// The app, started as the user (this process is not elevated): what Open Guesthouse asks for.
fn open_app(install_dir: &Path, dry_run: bool) {
    let exe = install_dir.join(engine::APP_EXE);
    if dry_run {
        log::say(&format!("would start {}", exe.display()));
        return;
    }
    if !exe.exists() {
        log::say(&format!("{} isn't there", exe.display()));
        return;
    }
    let started = if !system::is_elevated() {
        system::start(&exe, &[])
    } else {
        // Run as an administrator (from an elevated prompt, say), what is started here would be
        // elevated too; Explorer starts it as the signed-in user instead.
        let explorer = system::windows_dir().join("explorer.exe");
        system::start(&explorer, &[&exe.to_string_lossy()])
    };
    log::say(&format!("started {}: {started:?}", exe.display()));
}

/// The elevated part: the pages from a private folder with the runtime beside them, or with
/// `--silent` the steps here. Its exit code is the pages' (src/ui.rs).
///
/// It first copies its own file, read once from a handle that keeps it as it is, into a new
/// folder only Administrators and SYSTEM may change, and everything after reads that copy only.
/// Started from the install folder (the uninstaller), it runs that copy instead (`--temp-copy`)
/// and ends, so that the folder can be deleted; the copy deletes its folder when it is done.
pub fn stage2(args: &Args) -> i32 {
    let me = std::env::current_exe().expect("this exe");
    let elevated = system::is_elevated();
    log::say(&format!("elevated part: {} (elevated: {elevated})", me.display()));
    // What it starts starts in Windows' folder, not in the one it was started from.
    let _ = std::env::set_current_dir(system::windows_dir());
    let fail = |text: &str| -> i32 {
        log::say(text);
        if !args.silent {
            system::message(text);
        }
        EXIT_FAILED
    };
    if !args.dry_run && !elevated {
        return fail("Guesthouse setup needs an administrator, and Windows didn't give it one.");
    }
    let code = match own_copy(args, &me, elevated) {
        Err(e) => fail(&format!("Guesthouse setup couldn't copy itself into a folder of its own: {e}")),
        Ok(copy) if copy.runs_from_it => {
            let code = stage2_inner(args, &copy.exe, elevated);
            system::delete_after_exit(&copy.dir, elevated);
            code
        }
        Ok(copy) if from_install_folder(args, &me) => hand_over(args, &copy, elevated),
        Ok(copy) => {
            let code = stage2_inner(args, &copy.exe, elevated);
            remove_soon(&copy.dir, elevated);
            code
        }
    };
    log::say(&format!("elevated part ends: {code}"));
    code
}

/// The elevated part's copy of its own file, and the folder it is in.
struct OwnCopy {
    dir: PathBuf,
    exe: PathBuf,
    /// This process is the copy (`--temp-copy`).
    runs_from_it: bool,
}

fn own_copy(args: &Args, me: &Path, elevated: bool) -> std::io::Result<OwnCopy> {
    if args.temp_copy
        && let Some(dir) = me.parent()
        && system::is_private_dir(dir, elevated)
    {
        return Ok(OwnCopy { dir: dir.to_path_buf(), exe: me.to_path_buf(), runs_from_it: true });
    }
    let dir = system::private_dir(elevated)?;
    let exe = dir.join("guesthouse-setup.exe");
    let copied = system::open_own_file(me).and_then(|mut from| {
        let mut to = std::fs::File::create_new(&exe)?;
        std::io::copy(&mut from, &mut to)?;
        to.sync_all()
    });
    if let Err(e) = copied {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    log::say(&format!("its files are read from {}", exe.display()));
    Ok(OwnCopy { dir, exe, runs_from_it: false })
}

/// Runs the copy as the elevated part and, silent, waits for its exit code.
fn hand_over(args: &Args, copy: &OwnCopy, elevated: bool) -> i32 {
    let mut child_args = vec!["--stage2".to_string()];
    child_args.extend(args.forwarded());
    child_args.push("--temp-copy".into());
    log::say(&format!("hands over to its copy {}", copy.exe.display()));
    system::allow_foreground();
    match std::process::Command::new(&copy.exe).args(&child_args).current_dir(&copy.dir).spawn() {
        Ok(mut child) if args.silent => child.wait().ok().and_then(|s| s.code()).unwrap_or(EXIT_FAILED),
        Ok(_) => EXIT_DONE,
        Err(e) => {
            remove_soon(&copy.dir, elevated);
            log::say(&format!("Guesthouse setup couldn't start its copy: {e}"));
            if !args.silent {
                system::message(&format!("Guesthouse setup couldn't start its copy: {e}"));
            }
            EXIT_FAILED
        }
    }
}

fn stage2_inner(args: &Args, source: &Path, elevated: bool) -> i32 {
    let fail = |text: &str| -> i32 {
        log::say(text);
        if !args.silent {
            system::message(text);
        }
        EXIT_FAILED
    };
    if !system::single_instance() {
        return fail("Guesthouse setup is already running.");
    }

    if args.silent {
        let ctx = Ctx::new(args, source.to_path_buf());
        let archive = Archive::open(source).ok();
        let facts = engine::facts(&ctx, archive.as_ref());
        if ctx.mode == engine::Mode::Uninstall && facts.installed.is_none() {
            log::say("Guesthouse isn't installed: nothing to remove");
            return EXIT_DONE;
        }
        let mut emit = |e: Event| {
            if let Event::Detail(_, line) = &e {
                log::say(&format!("  {line}"));
            }
        };
        let outcome = engine::run(&ctx, &facts, &mut emit);
        if outcome.restart_needed {
            log::say("Windows has to restart before a machine can run");
        }
        return if outcome.ok { EXIT_DONE } else { EXIT_FAILED };
    }

    // The pages, with the Windows App Runtime beside them, in a folder nobody unelevated can
    // write to, so that no DLL in it is anyone else's.
    let archive = match Archive::open(source) {
        Ok(a) => a,
        Err(e) => return fail(&e.to_string()),
    };
    let dir = match system::private_dir(elevated) {
        Ok(d) => d,
        Err(e) => return fail(&format!("Guesthouse setup couldn't make its folder: {e}")),
    };
    let started = std::time::Instant::now();
    let exe = dir.join("guesthouse-setup.exe");
    let unpacked = archive
        .unpack(&archive.index.ui, &dir, &mut |_| {})
        .and_then(|files| archive.write_exe(&exe).map(|()| files));
    let code = match unpacked {
        Err(e) => fail(&format!("Guesthouse setup couldn't unpack itself into {}: {e}", dir.display())),
        Ok(files) => {
            log::say(&format!(
                "unpacked the pages' runtime into {}: {} files in {:.1} s",
                dir.display(),
                files.len(),
                started.elapsed().as_secs_f64()
            ));
            let mut ui_args =
                vec!["--ui".to_string(), "--source".into(), source.to_string_lossy().into_owned()];
            ui_args.extend(args.forwarded());
            system::allow_foreground();
            match std::process::Command::new(&exe).args(&ui_args).current_dir(&dir).status() {
                Ok(status) => status.code().unwrap_or(EXIT_FAILED),
                Err(e) => fail(&format!("Guesthouse setup couldn't open its window: {e}")),
            }
        }
    };
    remove_soon(&dir, elevated);
    code
}

/// Deletes a folder whose files a process that just ended may still hold for a moment.
fn remove_soon(dir: &Path, elevated: bool) {
    for _ in 0..25 {
        if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    log::say(&format!("{} couldn't be deleted now; it goes later", dir.display()));
    system::delete_after_exit(dir, elevated);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("launch-test-{name}-{}", std::process::id()));
        let _ = std::process::Command::new("cmd").args(["/c", "rd", "/s", "/q"]).arg(&dir).status();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn its_own_file_stays_as_it_is_while_read() {
        let me = std::env::current_exe().unwrap();
        let held = system::open_own_file(&me).unwrap();
        let moved = me.with_extension("moved");
        let renamed = std::fs::rename(&me, &moved).is_ok();
        if renamed {
            std::fs::rename(&moved, &me).unwrap();
        }
        drop(held);
        assert!(!renamed, "the file was renamed while it was being read");
    }

    #[test]
    fn another_file_at_its_path_is_not_read() {
        let dir = folder("other");
        let other = dir.join("guesthouse-setup.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &other).unwrap();
        assert!(system::open_own_file(&other).is_err(), "a file it doesn't run from was read as its own");
        assert!(own_copy(&Args::default(), &other, false).is_err());
    }

    #[test]
    fn the_copy_is_its_own_file_in_a_private_folder() {
        let me = std::env::current_exe().unwrap();
        let copy = own_copy(&Args::default(), &me, false).unwrap();
        assert!(!copy.runs_from_it);
        assert!(system::is_private_dir(&copy.dir, false));
        assert_eq!(std::fs::read(&copy.exe).unwrap(), std::fs::read(&me).unwrap());
        std::fs::remove_dir_all(&copy.dir).unwrap();
    }

    #[test]
    fn only_a_private_folder_is_deleted_after_exit() {
        let dir = folder("delete");
        let nested = dir.join("guesthouse-setup-nested");
        std::fs::create_dir(&nested).unwrap();
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let link = std::env::temp_dir().join(format!("guesthouse-setup-link-{}", std::process::id()));
        let _ = std::fs::remove_dir(&link);
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/j"])
            .arg(&link)
            .arg(&elsewhere)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink /j: {}", String::from_utf8_lossy(&made.stdout));
        let private = system::private_dir(false).unwrap();
        let refused =
            [(dir.clone(), false), (nested.clone(), false), (link.clone(), false), (private.clone(), true)];
        let scheduled: Vec<_> =
            refused.iter().filter(|(d, elevated)| system::delete_after_exit(d, *elevated)).collect();
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir(&private);
        assert!(scheduled.is_empty(), "scheduled for deletion: {scheduled:?}");
        assert!(elsewhere.exists());
    }
}
