//! The devices the user removed from a machine's Remote View, kept on this PC. Which devices a guest
//! trusts is on the guest's own disk, so a snapshot that is gone back to, or a machine that was
//! imported, brings back devices the user never paired here or took out since. So at each start the
//! guest is told again to stop trusting each device on this list, and a machine that was just imported
//! has every device it came with revoked once.

use guesthouse_remote_view_core::ctl::check_fingerprint;
use std::fs;
use std::path::Path;

const FILE: &str = "remote-view-revoked.json";
/// Present in the folder of a machine that came from an import, until its first start has revoked what
/// its guest trusted.
const FRESH: &str = "remote-view-fresh";
/// The most fingerprints kept: a list of one machine's devices is short.
const MOST: usize = 256;

/// The fingerprints removed, in the order they were.
pub fn removed(folder: &Path) -> Vec<String> {
    let text = fs::read_to_string(folder.join(FILE)).unwrap_or_default();
    let listed: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
    listed.into_iter().filter(|fingerprint| check_fingerprint(fingerprint).is_ok()).take(MOST).collect()
}

fn write(folder: &Path, list: &[String]) -> std::io::Result<()> {
    let file = folder.join(FILE);
    if list.is_empty() {
        return match fs::remove_file(file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_string_pretty(list).unwrap_or_default();
    fs::write(file, text + "\n")
}

/// Notes that this device was removed.
pub fn add(folder: &Path, fingerprint: &str) -> std::io::Result<()> {
    let Ok(fingerprint) = check_fingerprint(fingerprint) else { return Ok(()) };
    let mut list = removed(folder);
    if !list.contains(&fingerprint) {
        list.push(fingerprint);
        list.drain(..list.len().saturating_sub(MOST));
    }
    write(folder, &list)
}

/// Notes that this device is trusted again, as it was paired again here.
pub fn forget(folder: &Path, fingerprint: &str) -> std::io::Result<()> {
    let Ok(fingerprint) = check_fingerprint(fingerprint) else { return Ok(()) };
    let mut list = removed(folder);
    let before = list.len();
    list.retain(|listed| *listed != fingerprint);
    if list.len() == before { Ok(()) } else { write(folder, &list) }
}

/// Marks a machine that was just imported.
pub fn mark_imported(folder: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(folder.join(FRESH))?;
    Ok(())
}

/// Whether the machine is an import that has not started yet.
pub fn is_fresh(folder: &Path) -> bool {
    folder.join(FRESH).exists()
}

/// Takes the mark off, once what its guest trusted has been revoked.
pub fn clear_fresh(folder: &Path) {
    let _ = fs::remove_file(folder.join(FRESH));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    const ONE: &str = "aa";
    fn print(letter: &str) -> String {
        letter.repeat(32)
    }

    #[test]
    fn a_removed_device_is_listed_once_and_forgotten_when_it_pairs_again() {
        let scratch = Scratch::new();
        let folder = scratch.root();
        assert!(removed(folder).is_empty());
        add(folder, &print(ONE)).unwrap();
        add(folder, &print(ONE)).unwrap();
        add(folder, &print("bb")).unwrap();
        add(folder, "not a fingerprint").unwrap();
        assert_eq!(removed(folder), [print(ONE), print("bb")]);
        forget(folder, &print(ONE)).unwrap();
        assert_eq!(removed(folder), [print("bb")]);
        forget(folder, &print("bb")).unwrap();
        assert!(!folder.join(FILE).exists());
    }

    #[test]
    fn what_is_in_the_file_but_not_a_fingerprint_is_not_taken() {
        let scratch = Scratch::new();
        fs::write(scratch.root().join(FILE), format!(r#"["{}", "--all", "x y"]"#, print(ONE))).unwrap();
        assert_eq!(removed(scratch.root()), [print(ONE)]);
    }

    #[test]
    fn an_import_is_fresh_until_cleared() {
        let scratch = Scratch::new();
        assert!(!is_fresh(scratch.root()));
        mark_imported(scratch.root()).unwrap();
        assert!(is_fresh(scratch.root()));
        clear_fresh(scratch.root());
        assert!(!is_fresh(scratch.root()));
    }

    #[test]
    fn the_guest_is_told_again_of_each_removed_device_and_of_an_imports_own() {
        use crate::machine::Machine;
        use crate::testing::scratch_context;
        use std::cell::RefCell;
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = Machine::new("w11", "W11", "/d", "/v");
        let folder = machine.folder(&context);
        fs::create_dir_all(&folder).unwrap();
        add(&folder, &print("aa")).unwrap();
        let sent = RefCell::new(Vec::new());
        let status = format!(r#"{{"devices":[{{"fingerprint":"{}","name":"Old phone"}}]}}"#, print("cc"));
        let ask = |script: &str, _seconds: u64| {
            sent.borrow_mut().push(script.to_owned());
            if script.contains("revoke") { "ok".to_owned() } else { status.clone() }
        };
        super::super::machine::revoke_removed(&context, &machine, &ask);
        let said = sent.borrow().join("\n");
        assert!(
            said.contains(&format!("'revoke' '{}'", print("aa"))) && !said.contains(&print("cc")),
            "{said}"
        );
        sent.borrow_mut().clear();
        mark_imported(&folder).unwrap();
        super::super::machine::revoke_removed(&context, &machine, &ask);
        let said = sent.borrow().join("\n");
        assert!(said.contains(&format!("'revoke' '{}'", print("aa"))), "{said}");
        assert!(said.contains(&format!("'revoke' '{}'", print("cc"))), "{said}");
        assert!(!is_fresh(&folder), "an import's own devices are revoked once");
    }
}
