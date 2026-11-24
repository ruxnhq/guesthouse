use super::*;
use std::process::Command;

/// A folder made for a machine is held, and while it is, nothing can rename it or the folder it's
/// in. A link is never held. Making a folder owned by the administrators needs an elevated prompt.
#[test]
fn a_folder_made_for_a_machine_is_held_in_place_and_a_link_is_not() {
    if !guesthouse_hcs::host::is_elevated() {
        return;
    }
    let root = std::env::temp_dir().join(format!("gh-hold-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let made = root.join("win");
    private_folder(&made).unwrap();
    assert!(private_folder(&made).is_err(), "a folder that's there isn't made again");

    let held = hold(&made).unwrap();
    assert!(fs::rename(&made, root.join("moved")).is_err(), "held, it stays where it is");
    assert!(fs::rename(&root, root.with_extension("moved")).is_err(), "and so does the folder it's in");
    assert!(fs::remove_dir(&made).is_err());
    fs::write(made.join("disk.vhdx"), b"disk").expect("files are made in it as usual");
    drop(held);

    // A junction to it is a link, which isn't held.
    let link = root.join("link");
    let linked =
        Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(&link).arg(&made).output().unwrap();
    assert!(linked.status.success());
    assert!(hold(&link).unwrap_err().contains("is a link"));
    Command::new("cmd").arg("/c").arg("rmdir").arg(&link).status().unwrap();
    assert!(free_space(&root).unwrap() > 0);
    fs::remove_dir_all(&root).unwrap();
}
