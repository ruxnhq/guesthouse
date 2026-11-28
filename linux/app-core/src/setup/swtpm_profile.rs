//! Ubuntu's swtpm comes with an AppArmor profile. It lets swtpm write in home folders and /tmp, but not
//! in /run/user, where the run folder is, so the virtual TPM can't make its log or its socket. It also
//! takes the data channel QEMU hands it only from libvirt's QEMU, and a jailed QEMU runs as Ubuntu's
//! `bwrap` and `unpriv_bwrap`. Either way a Windows machine can't start. install.sh adds both rules. This finds what's still
//! missing: on an install from before that, or with the machines moved out of the home folders.

use super::checks::{Check, CheckState, existing_parent};
use crate::context::Context;
use crate::text::shell_quote;
use std::fs;
use std::path::{Path, PathBuf};

const PROFILE: &str = "usr.bin.swtpm";
/// The rule install.sh adds. Every user's run folder is under it.
pub const RUN_FOLDER_RULE: &str = "owner /run/user/[0-9]*/guesthouse/** rwk,";
/// The rule for the socket pair QEMU hands swtpm with `CMD_SET_DATAFD`, whatever QEMU runs as.
pub const SOCKET_RULE: &str = "unix (send, receive) type=stream addr=none,";
/// The folders the profile already lets swtpm write in, as their owner, besides the home folders.
const ALLOWED: [&str; 3] = ["/root", "/tmp", "/var/tmp"];

/// The rules swtpm's profile still needs, for the run folder and for the machines' TPM folders.
/// None if AppArmor isn't on, swtpm has no profile, or the profile is turned off or only complains.
pub fn missing_rules(context: &Context) -> Vec<String> {
    let modules = fs::read_to_string(&context.security_modules).unwrap_or_default();
    if !modules.trim().split(',').any(|module| module == "apparmor") {
        return Vec::new();
    }
    let Ok(profile) = fs::read_to_string(context.apparmor.join(PROFILE)) else {
        return Vec::new();
    };
    let complains =
        profile.lines().any(|line| line.trim_start().starts_with("profile ") && line.contains("complain"));
    if complains || context.apparmor.join("disable").join(PROFILE).exists() {
        return Vec::new();
    }
    let local = fs::read_to_string(local_file(context)).unwrap_or_default();
    let present: Vec<&str> = local.lines().map(str::trim).collect();
    let mut rules = Vec::new();
    for folder in [&context.paths.runtime, &context.paths.machines] {
        let real = real_path(folder);
        if allowed_already(&real) {
            continue;
        }
        let rule = rule_for(&real);
        if !present.contains(&rule.as_str()) && !rules.contains(&rule) {
            rules.push(rule);
        }
    }
    if !present.contains(&SOCKET_RULE) {
        rules.push(SOCKET_RULE.to_owned());
    }
    rules
}

/// The command that adds these rules, and what to do after.
pub fn fix(context: &Context, rules: &[String]) -> String {
    let words: Vec<String> = rules.iter().map(|rule| shell_quote(rule)).collect();
    format!(
        "To let it, run printf '%s\\n' {} | sudo tee -a {} && sudo apparmor_parser -r {}, and start the machine \
         again.",
        words.join(" "),
        shell_quote(&local_file(context).display().to_string()),
        shell_quote(&context.apparmor.join(PROFILE).display().to_string()),
    )
}

pub fn check_swtpm_profile(context: &Context) -> Option<Check> {
    let rules = missing_rules(context);
    if rules.is_empty() {
        return None;
    }
    let text = "swtpm's AppArmor profile keeps it from writing where a machine's virtual TPM goes, so Windows \
                machines can't start.";
    Some(Check::new("Virtual TPM", CheckState::Warning, text, fix(context, &rules)))
}

fn local_file(context: &Context) -> PathBuf {
    context.apparmor.join("local").join(PROFILE)
}

/// AppArmor goes by where a file really is, past any link.
fn real_path(path: &Path) -> PathBuf {
    let parent = existing_parent(path);
    let rest = path.strip_prefix(&parent).map(Path::to_owned).unwrap_or_default();
    fs::canonicalize(&parent).unwrap_or(parent).join(rest)
}

/// The profile's `owner @{HOME}/**`, which is anything in a folder under /home, and its temporary folders.
fn allowed_already(folder: &Path) -> bool {
    let in_home = folder.starts_with("/home") && folder.components().count() >= 3;
    in_home || ALLOWED.iter().any(|allowed| folder.starts_with(allowed))
}

fn rule_for(folder: &Path) -> String {
    if is_users_run_folder(folder) {
        return RUN_FOLDER_RULE.to_owned();
    }
    let text = folder.display().to_string();
    if text.contains(char::is_whitespace) {
        format!("owner \"{text}/**\" rwk,")
    } else {
        format!("owner {text}/** rwk,")
    }
}

/// `/run/user/UID/guesthouse`, or a folder in it.
fn is_users_run_folder(folder: &Path) -> bool {
    let parts: Vec<_> = folder.iter().map(|part| part.to_string_lossy()).collect();
    parts.len() >= 5
        && parts[1] == "run"
        && parts[2] == "user"
        && !parts[3].is_empty()
        && parts[3].bytes().all(|byte| byte.is_ascii_digit())
        && parts[4] == "guesthouse"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    const STOCK_PROFILE: &str = "profile swtpm /usr/bin/swtpm {\n  owner @{HOME}/** rwk,\n}\n";

    /// A PC with AppArmor on and Ubuntu's swtpm profile, its run folder in /run/user and its machines at home.
    fn ubuntu(scratch: &Scratch) -> Context {
        let mut context = scratch_context(scratch);
        context.security_modules = scratch.write("lsm", "lockdown,capability,landlock,yama,apparmor,bpf\n");
        context.apparmor = scratch.path("apparmor.d");
        fs::create_dir_all(context.apparmor.join("local")).unwrap();
        fs::write(context.apparmor.join(PROFILE), STOCK_PROFILE).unwrap();
        fs::write(local_file(&context), "").unwrap();
        context.paths.runtime = "/run/user/1000/guesthouse".into();
        context.paths.machines = "/home/someone/.local/share/guesthouse/machines".into();
        context
    }

    #[test]
    fn the_run_folder_and_the_jail_need_the_rules_install_sh_adds() {
        let scratch = Scratch::new();
        let context = ubuntu(&scratch);
        assert_eq!(missing_rules(&context), [RUN_FOLDER_RULE, SOCKET_RULE]);
        let check = check_swtpm_profile(&context).unwrap();
        assert_eq!(check.state, CheckState::Warning);
        let local = local_file(&context).display().to_string();
        assert!(
            check.fix.contains(&format!(
                "printf '%s\\n' 'owner /run/user/[0-9]*/guesthouse/** rwk,' 'unix (send, receive) type=stream addr=none,' \
                 | sudo tee -a {local}"
            )),
            "{}",
            check.fix
        );
    }

    #[test]
    fn with_the_rules_in_place_nothing_is_missing() {
        let scratch = Scratch::new();
        let context = ubuntu(&scratch);
        fs::write(local_file(&context), format!("# Guesthouse\n{RUN_FOLDER_RULE}\n{SOCKET_RULE}\n")).unwrap();
        assert!(missing_rules(&context).is_empty());
        assert!(check_swtpm_profile(&context).is_none());
    }

    #[test]
    fn an_install_with_only_the_run_folder_rule_is_told_the_socket_rule() {
        let scratch = Scratch::new();
        let context = ubuntu(&scratch);
        fs::write(local_file(&context), format!("{RUN_FOLDER_RULE}\n")).unwrap();
        assert_eq!(missing_rules(&context), [SOCKET_RULE]);
    }

    #[test]
    fn machines_outside_the_home_folders_need_a_rule_of_their_own() {
        let scratch = Scratch::new();
        let mut context = ubuntu(&scratch);
        fs::write(local_file(&context), format!("{RUN_FOLDER_RULE}\n{SOCKET_RULE}\n")).unwrap();
        context.paths.machines = "/nothing/here/vm store/machines".into();
        assert_eq!(missing_rules(&context), ["owner \"/nothing/here/vm store/machines/**\" rwk,"]);
        context.paths.machines = "/tmp/machines".into();
        assert!(missing_rules(&context).is_empty());
    }

    #[test]
    fn a_link_into_the_home_folders_counts_where_it_leads() {
        let scratch = Scratch::new();
        let mut context = ubuntu(&scratch);
        fs::write(local_file(&context), format!("{RUN_FOLDER_RULE}\n{SOCKET_RULE}\n")).unwrap();
        let store = scratch.path("store");
        fs::create_dir_all(&store).unwrap();
        let link = scratch.path("link");
        std::os::unix::fs::symlink(&store, &link).unwrap();
        context.paths.machines = link.join("machines");
        let real = fs::canonicalize(&store).unwrap().join("machines");
        let expected = if allowed_already(&real) { Vec::new() } else { vec![rule_for(&real)] };
        assert_eq!(missing_rules(&context), expected);
    }

    #[test]
    fn no_apparmor_no_profile_or_a_profile_turned_off_needs_nothing() {
        let scratch = Scratch::new();
        let mut context = ubuntu(&scratch);
        context.security_modules = scratch.write("lsm-off", "capability,landlock,selinux\n");
        assert!(missing_rules(&context).is_empty());

        let context = ubuntu(&scratch);
        fs::create_dir_all(context.apparmor.join("disable")).unwrap();
        fs::write(context.apparmor.join("disable").join(PROFILE), "").unwrap();
        assert!(missing_rules(&context).is_empty());

        let context = ubuntu(&scratch);
        fs::write(context.apparmor.join(PROFILE), "profile swtpm /usr/bin/swtpm flags=(complain) {\n}\n")
            .unwrap();
        assert!(missing_rules(&context).is_empty());

        let mut context = ubuntu(&scratch);
        context.apparmor = scratch.path("no-apparmor.d");
        assert!(missing_rules(&context).is_empty());
    }

    #[test]
    fn only_a_users_folder_in_run_user_gets_the_wide_rule() {
        assert!(is_users_run_folder(Path::new("/run/user/1000/guesthouse")));
        assert!(is_users_run_folder(Path::new("/run/user/1000/guesthouse/slot-2")));
        assert!(!is_users_run_folder(Path::new("/run/user/me/guesthouse")));
        assert!(!is_users_run_folder(Path::new("/run/user/1000/other")));
        assert_eq!(rule_for(Path::new("/srv/run")), "owner /srv/run/** rwk,");
    }
}
