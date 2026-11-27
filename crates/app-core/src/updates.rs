//! Newer releases, and updating to one from the app. An install from install.sh has the script, which
//! finds them and updates. A deb, rpm or pacman package names its package manager in `installed-by`,
//! reads the file a release publishes on pkg.ruxn.com, and updates through that manager. A build of the
//! repo has neither, so it never asks and can't update itself. What the file says is decided in
//! guesthouse-update-check, which every host shares.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::log::write_log;
pub use guesthouse_update_check::{Found, Method};
use guesthouse_update_check::{LINUX_LATEST_URL, LONGEST, found};
use std::fs;
use std::process::{Command, Stdio};

const INSTALL_SCRIPT: &str = "install.sh";
/// What a package writes in the program folder: apt, dnf or pacman.
const INSTALLED_BY: &str = "installed-by";
const CHECK_OPTION: &str = "--check-update";
const UPDATE_OPTION: &str = "--update";
/// Tells the script the app is running it, and which process the app is.
const FROM_APP_OPTION: &str = "--from-app=";
/// What the script exits with when there's something newer.
const UPDATE_AVAILABLE: i32 = 10;
const UPDATE_WORD: &str = "update-available";
const UP_TO_DATE_WORD: &str = "up-to-date";
/// The script gives up on the network well before this.
const MOST_SECONDS: &str = "60";
/// For fetching the latest file, which is a few dozen bytes.
const FETCH_SECONDS: &str = "20";
/// Where the script says what went wrong: `install.sh: ` and the reason.
const SCRIPT_PREFIX: &str = "install.sh: ";
const NO_SCRIPT: &str =
    "This copy of Guesthouse wasn't installed with install.sh or a package, so it can't update itself.";
/// Arch takes no partial upgrades, so Guesthouse comes with the rest of the system.
pub const PACMAN_UPDATE: &str = "sudo pacman -Syu";
const PACMAN_SAYS: &str =
    "On Arch, Guesthouse updates with the rest of the system. Run sudo pacman -Syu in a terminal.";
/// Only Guesthouse, and whatever new it depends on: never a removal, and never the rest of the system.
/// The list is the one the docs have you make, so the update reads Guesthouse's repository and no
/// other. With another name, every list is read again, which is slower and comes to the same.
const APT_UPDATE: &str = r#"export DEBIAN_FRONTEND=noninteractive; list=/etc/apt/sources.list.d/guesthouse.sources; if [ -f "$list" ]; then apt-get update -o Dir::Etc::sourcelist="$list" -o Dir::Etc::sourceparts=- -o APT::Get::List-Cleanup=0; else apt-get update; fi && apt-get install -y --only-upgrade --no-remove guesthouse"#;
const DNF_UPDATE: &str = "dnf upgrade -y --refresh guesthouse";

/// The version this app is.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How this copy was installed, which is how it updates, or `None` for a build of the repo.
pub fn method(context: &Context) -> Option<Method> {
    if context.paths.helper(INSTALL_SCRIPT).is_file() {
        return Some(Method::Script);
    }
    let said = fs::read_to_string(context.paths.helper(INSTALLED_BY)).ok()?;
    Method::from_installed_by(&said)
}

/// What the script said, from a line like `update-available 1.0 1.1` or `up-to-date 1.1`.
fn read_answer(said: &str, exit: Option<i32>) -> Found {
    let words = |word: &str| -> Option<Vec<&str>> {
        Some(said.lines().find(|line| line.starts_with(word))?.split_whitespace().collect())
    };
    match exit {
        Some(UPDATE_AVAILABLE) => match words(UPDATE_WORD).as_deref() {
            Some([_, _, newest]) => Found::Newer((*newest).to_owned()),
            _ => Found::CantTell,
        },
        Some(0) if words(UP_TO_DATE_WORD).is_some() => Found::Latest,
        _ => Found::CantTell,
    }
}

/// Whether there's a newer release. This can take a while, so it's for a thread.
pub fn check(context: &Context) -> Found {
    match method(context) {
        None => Found::NotInstalled,
        Some(Method::Script) => check_with_script(context),
        Some(_) => found(VERSION, fetch_latest().as_deref()),
    }
}

fn check_with_script(context: &Context) -> Found {
    let Ok(result) = Command::new("timeout")
        .args([MOST_SECONDS, "sh"])
        .arg(context.paths.helper(INSTALL_SCRIPT))
        .arg(CHECK_OPTION)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Found::CantTell;
    };
    read_answer(&String::from_utf8_lossy(&result.stdout), result.status.code())
}

/// The latest file, with the curl a package depends on. Only HTTPS, with a time limit, and no more
/// than the file can be.
fn fetch_latest() -> Option<String> {
    let limit = LONGEST.to_string();
    let output = Command::new("curl")
        .args(["--fail", "--silent", "--location", "--proto", "=https", "--max-time", FETCH_SECONDS])
        .args(["--max-filesize", &limit, LINUX_LATEST_URL])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The script's own reason for stopping, or its last words if it gave none.
fn reason(said: &str) -> String {
    let last = said.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default();
    let reason = said.lines().rev().find_map(|line| line.strip_prefix(SCRIPT_PREFIX)).unwrap_or(last);
    format!("The update stopped: {reason}")
}

/// What root runs to update a package install, the same words whatever the app has been told, so
/// nothing from outside goes into a command that runs as root.
fn package_command(method: Method) -> Option<&'static str> {
    match method {
        Method::Apt => Some(APT_UPDATE),
        Method::Dnf => Some(DNF_UPDATE),
        Method::Script | Method::Pacman => None,
    }
}

/// Brings the install up to the latest release, while the app (process `app`) runs. Its VMs must be
/// off. The app has asked already, and if root is needed, pkexec asks for the password in a window.
/// Everything the script or the package manager said goes in the log. This takes minutes, so it's for
/// a thread.
pub fn update(context: &Context, app: u32) -> Result<()> {
    let method = method(context).ok_or_else(|| Failure::new(NO_SCRIPT))?;
    let mut command = if method == Method::Script {
        let mut command = Command::new("sh");
        command
            .arg(context.paths.helper(INSTALL_SCRIPT))
            .arg(UPDATE_OPTION)
            .arg(format!("{FROM_APP_OPTION}{app}"));
        command
    } else {
        let words = package_command(method).ok_or_else(|| Failure::new(PACMAN_SAYS))?;
        let mut command = Command::new("pkexec");
        command.args(["sh", "-c", words]);
        command
    };
    let result = match command.stdin(Stdio::null()).output() {
        Ok(result) => result,
        Err(_) if method != Method::Script => return Err(Failure::new(no_pkexec(method))),
        Err(error) => return Err(error.into()),
    };
    let said =
        format!("{}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    let _ = write_log(context, &format!("app: the update said:\n{said}"));
    if result.status.success() { Ok(()) } else { Err(Failure::new(reason(&said))) }
}

fn no_pkexec(method: Method) -> String {
    let instead = if method == Method::Dnf {
        "sudo dnf upgrade guesthouse"
    } else {
        "sudo apt install --only-upgrade guesthouse"
    };
    format!("This needs root, and there's no pkexec to ask for it in a window. Run {instead} in a terminal.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    #[test]
    fn the_newest_version_is_the_lines_last_word() {
        assert_eq!(read_answer("update-available 1.0 1.1\n", Some(10)), Found::Newer("1.1".to_owned()));
        assert_eq!(read_answer("update-available 1.0\n", Some(10)), Found::CantTell);
        assert_eq!(read_answer("up-to-date 1.1\n", Some(0)), Found::Latest);
        assert_eq!(read_answer("unknown 1.1\n", Some(1)), Found::CantTell);
        assert_eq!(read_answer("", None), Found::CantTell);
    }

    fn with_script(scratch: &Scratch, text: &str) -> Found {
        scratch.write("program/install.sh", text);
        check(&scratch_context(scratch))
    }

    #[test]
    fn a_newer_release_is_said() {
        let scratch = Scratch::new();
        let found = with_script(&scratch, "echo update-available 1.0 1.2\nexit 10\n");
        assert_eq!(found, Found::Newer("1.2".to_owned()));
    }

    #[test]
    fn being_up_to_date_is_told_from_not_knowing() {
        let scratch = Scratch::new();
        assert_eq!(with_script(&scratch, "echo up-to-date 1.0\nexit 0\n"), Found::Latest);
        assert_eq!(with_script(&scratch, "echo unknown 1.0\nexit 1\n"), Found::CantTell);
    }

    #[test]
    fn a_build_with_no_script_never_asks() {
        let scratch = Scratch::new();
        assert_eq!(check(&scratch_context(&scratch)), Found::NotInstalled);
        assert_eq!(update(&scratch_context(&scratch), 1), Err(Failure::new(NO_SCRIPT)));
    }

    #[test]
    fn a_package_says_how_it_came() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        for (word, way) in [("apt\n", Method::Apt), ("dnf\n", Method::Dnf), ("pacman\n", Method::Pacman)] {
            scratch.write("program/installed-by", word);
            assert_eq!(method(&context), Some(way));
        }
        scratch.write("program/installed-by", "something else\n");
        assert_eq!(method(&context), None);
        assert_eq!(check(&context), Found::NotInstalled);
        scratch.write("program/install.sh", "exit 0\n");
        assert_eq!(method(&context), Some(Method::Script), "the script is what an install.sh install has");
    }

    #[test]
    fn apt_and_dnf_update_guesthouse_alone() {
        let apt = package_command(Method::Apt).unwrap();
        assert!(apt.contains("list=/etc/apt/sources.list.d/guesthouse.sources;"));
        assert!(apt.ends_with("apt-get install -y --only-upgrade --no-remove guesthouse"));
        assert_eq!(package_command(Method::Dnf), Some("dnf upgrade -y --refresh guesthouse"));
        assert_eq!(package_command(Method::Pacman), None, "no partial upgrades on Arch");
        assert_eq!(package_command(Method::Script), None);
    }

    #[test]
    fn arch_is_told_to_upgrade_the_system() {
        let scratch = Scratch::new();
        scratch.write("program/installed-by", "pacman\n");
        let failure = update(&scratch_context(&scratch), 1).unwrap_err();
        assert!(failure.0.contains(PACMAN_UPDATE));
    }

    #[test]
    fn the_update_says_which_process_the_app_is() {
        let scratch = Scratch::new();
        scratch.write("program/install.sh", "[ \"$1 $2\" = '--update --from-app=42' ] || exit 3\n");
        assert_eq!(update(&scratch_context(&scratch), 42), Ok(()));
    }

    #[test]
    fn a_stopped_update_gives_the_scripts_reason() {
        let scratch = Scratch::new();
        scratch.write(
            "program/install.sh",
            "echo Checking this PC.\necho 'install.sh: Guesthouse is running, so nothing changes yet.' >&2\n\
             echo '123 qemu-system-x86_64' >&2\nexit 1\n",
        );
        let failure = update(&scratch_context(&scratch), 1).unwrap_err();
        assert_eq!(failure.0, "The update stopped: Guesthouse is running, so nothing changes yet.");
    }
}
