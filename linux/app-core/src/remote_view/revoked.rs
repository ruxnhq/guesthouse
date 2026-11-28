//! The devices the user removed from a machine's Remote View, kept on this PC. Which devices a guest
//! trusts is on the guest's own disk, so a snapshot that is gone back to, or a machine that was
//! imported, brings back devices the user never paired here or took out since. So at each start the
//! guest is told again to stop trusting each device on this list, and a machine that was just imported
//! has every device it came with revoked once.

#[cfg(test)]
use guesthouse_remote_view_core::revoked::FILE;
pub use guesthouse_remote_view_core::revoked::{add, forget, removed};
use std::fs;
use std::path::Path;

/// Present in the folder of a machine that came from an import, until its first start has revoked what
/// its guest trusted.
const FRESH: &str = "remote-view-fresh";

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
