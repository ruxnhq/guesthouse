use super::*;
use std::fs;
use std::os::windows::fs::symlink_file;
use std::process::Command;
use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
use windows_sys::Win32::Security::GetKernelObjectSecurity;

/// A folder standing in for a guest's volume, and one beside it standing in for the host's own
/// files, which nothing done on the volume may reach.
struct Scratch {
    base: PathBuf,
    volume: PathBuf,
    host: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let base = std::env::temp_dir().join(format!("gh-guest-volume-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (volume, host) = (base.join("volume"), base.join("host"));
        fs::create_dir_all(&volume).unwrap();
        fs::create_dir_all(&host).unwrap();
        Scratch { base, volume, host }
    }

    fn open(&self) -> GuestVolume {
        GuestVolume::open(&self.volume).unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// A junction at `link` to the folder `target`, as anyone may make one.
pub(crate) fn junction(link: &Path, target: &Path) {
    let made = Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(link).arg(target).output().unwrap();
    assert!(made.status.success(), "mklink /J: {}", String::from_utf8_lossy(&made.stderr));
}

#[test]
fn a_write_through_a_junction_is_refused_and_nothing_lands_beyond_it() {
    let scratch = Scratch::new("write-junction");
    junction(&scratch.volume.join("Guesthouse"), &scratch.host);
    let volume = scratch.open();
    let refused = volume.write(r"Guesthouse\screen\trusted", "x").unwrap_err();
    assert!(refused.to_string().contains("link"), "{refused}");
    assert!(volume.create_dir_all(r"Guesthouse\screen").is_err());
    assert!(volume.read(r"Guesthouse\screen\trusted", 10).is_err());
    assert_eq!(volume.entry("Guesthouse").unwrap(), Some(Entry::Link));
    assert!(!volume.is_dir("Guesthouse"));
    assert_eq!(fs::read_dir(&scratch.host).unwrap().count(), 0, "nothing was made beyond the junction");
}

#[test]
fn a_file_link_is_never_read_or_written_through() {
    let scratch = Scratch::new("file-link");
    let secret = scratch.host.join("secret.txt");
    fs::write(&secret, "the host's own").unwrap();
    fs::create_dir_all(scratch.volume.join("Guesthouse")).unwrap();
    symlink_file(&secret, scratch.volume.join(r"Guesthouse\report.txt")).unwrap();
    let volume = scratch.open();
    assert!(volume.read(r"Guesthouse\report.txt", 4096).is_err());
    assert!(!volume.is_file(r"Guesthouse\report.txt"));
    assert_eq!(volume.entry(r"Guesthouse\report.txt").unwrap(), Some(Entry::Link));
    assert!(volume.write(r"Guesthouse\report.txt", "written").is_err());
    let copied_from = scratch.base.join("source.bin");
    fs::write(&copied_from, "copied").unwrap();
    assert!(volume.copy_in(&copied_from, r"Guesthouse\report.txt").is_err());
    assert_eq!(fs::read_to_string(&secret).unwrap(), "the host's own");
    // Removed, it's the link that goes.
    volume.remove_file(r"Guesthouse\report.txt").unwrap();
    assert_eq!(fs::read_to_string(&secret).unwrap(), "the host's own");
    assert_eq!(volume.entry(r"Guesthouse\report.txt").unwrap(), None);
}

#[test]
fn removing_a_folder_removes_its_links_and_never_what_they_name() {
    let scratch = Scratch::new("remove");
    let kept = scratch.host.join("kept");
    fs::create_dir_all(kept.join("inner")).unwrap();
    fs::write(kept.join("inner").join("file"), "kept").unwrap();
    let old = scratch.volume.join(r"wsl\drivers\old");
    fs::create_dir_all(old.join("sub")).unwrap();
    fs::write(old.join("sub").join("a.dll"), "a").unwrap();
    junction(&old.join("into-host"), &kept);
    symlink_file(kept.join("inner").join("file"), old.join("file-link")).unwrap();
    // A read-only file goes too.
    let read_only = old.join("read-only.bin");
    fs::write(&read_only, "r").unwrap();
    let mut permissions = fs::metadata(&read_only).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&read_only, permissions).unwrap();
    let volume = scratch.open();

    volume.remove_tree(r"wsl\drivers\old").unwrap();
    assert!(!old.exists());
    assert_eq!(fs::read_to_string(kept.join("inner").join("file")).unwrap(), "kept");

    // A junction on the way is refused, and one at the end is removed as a link.
    junction(&scratch.volume.join(r"wsl\drivers\other"), &scratch.host);
    assert!(volume.remove_tree(r"wsl\drivers\other\kept").is_err());
    assert!(volume.read_dir(r"wsl\drivers\other").is_err());
    volume.remove_tree(r"wsl\drivers\other").unwrap();
    assert!(kept.join("inner").join("file").is_file());
    assert!(volume.read_dir(r"wsl\drivers").unwrap().is_empty());
}

#[test]
fn files_are_written_read_copied_and_listed_inside_the_volume() {
    let scratch = Scratch::new("plain");
    let volume = scratch.open();
    volume.write(r"Guesthouse\screen\encoder.txt", "auto").unwrap();
    assert_eq!(volume.read(r"Guesthouse\screen\encoder.txt", 100).unwrap(), b"auto");
    volume.write(r"Guesthouse\screen\encoder.txt", "nvenc").unwrap();
    assert_eq!(fs::read_to_string(scratch.volume.join(r"Guesthouse\screen\encoder.txt")).unwrap(), "nvenc");
    assert_eq!(volume.read(r"Guesthouse\screen\encoder.txt", 2).unwrap(), b"nv", "read no more than asked");

    let source = scratch.host.join("setup.exe");
    fs::write(&source, vec![7u8; 5000]).unwrap();
    volume.copy_in(&source, r"Guesthouse\screen\setup.exe").unwrap();
    let Some(Entry::File { len, modified }) = volume.entry(r"Guesthouse\screen\setup.exe").unwrap() else {
        panic!("the copy is a file")
    };
    assert_eq!(len, 5000);
    assert_eq!(modified, fs::metadata(&source).unwrap().modified().unwrap(), "a copy keeps its time");

    let mut names: Vec<(String, bool)> = volume
        .read_dir(r"Guesthouse\screen")
        .unwrap()
        .into_iter()
        .map(|(name, entry)| (name.to_string_lossy().into_owned(), matches!(entry, Entry::File { .. })))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [("encoder.txt".to_owned(), true), ("setup.exe".to_owned(), true)],
        "no file is left behind"
    );
    assert!(volume.is_dir("Guesthouse"));
    assert!(volume.is_file(r"guesthouse\SCREEN\Encoder.txt"), "names match whatever their case");
    assert!(volume.write("Guesthouse", "a folder is no file").is_err());
    let kept = scratch.volume.join(r"Guesthouse\screen\setup.exe");
    let mut permissions = fs::metadata(&kept).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&kept, permissions).unwrap();
    volume.write(r"Guesthouse\screen\setup.exe", "newer").unwrap();
    assert_eq!(fs::read_to_string(&kept).unwrap(), "newer", "a read-only file is replaced too");
    volume.remove_file(r"Guesthouse\screen\encoder.txt").unwrap();
    assert_eq!(volume.entry(r"Guesthouse\screen\encoder.txt").unwrap(), None);
    assert_eq!(volume.entry(r"Nothing\here").unwrap(), None);
}

#[test]
fn only_plain_paths_inside_the_volume_are_taken() {
    let scratch = Scratch::new("names");
    let volume = scratch.open();
    for path in [r"..\host\x", r"a\..\..\x", r"report.txt:hidden", r"C:\x", r"\x", ""] {
        assert!(volume.write(path, "x").is_err(), "{path}");
    }
    assert_eq!(fs::read_dir(&scratch.host).unwrap().count(), 0);
}

#[test]
fn the_root_may_not_be_a_link() {
    let scratch = Scratch::new("root");
    let link = scratch.base.join("linked-root");
    junction(&link, &scratch.volume);
    assert!(GuestVolume::open(&link).is_err());
}

/// The DACL of what is at `path`, as SDDL.
fn dacl_of(path: &Path) -> String {
    let file = fs::OpenOptions::new()
        .access_mode(READ_CONTROL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .unwrap();
    let mut needed = 0;
    let mut buffer = vec![0u64; 1024];
    // SAFETY: the buffer is as many bytes long as the length given.
    let read = unsafe {
        GetKernelObjectSecurity(
            file.as_raw_handle(),
            DACL_SECURITY_INFORMATION,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut needed,
        )
    };
    assert_ne!(read, 0, "{}", io::Error::last_os_error());
    let mut text: *mut u16 = null_mut();
    // SAFETY: the descriptor is the one just read; the text is freed below.
    let made = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            buffer.as_ptr().cast_mut().cast(),
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            null_mut(),
        )
    };
    assert_ne!(made, 0);
    // SAFETY: the conversion gave a string that ends in a zero.
    let sddl = unsafe {
        let length = (0..).take_while(|&at| *text.add(at) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(text, length))
    };
    // SAFETY: allocated by the conversion with LocalAlloc.
    unsafe { LocalFree(text.cast()) };
    sddl
}

/// This test's own account, as a SID, so it can still remove its scratch folder.
fn own_sid() -> String {
    let out = Command::new("whoami").args(["/user", "/fo", "csv", "/nh"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().rsplit(',').next().unwrap().trim_matches('"').to_owned()
}

#[test]
fn a_folder_given_only_some_lets_only_them_in_and_holds_no_links() {
    let scratch = Scratch::new("give-only");
    let folder = scratch.volume.join("Guesthouse");
    fs::create_dir_all(folder.join("screen")).unwrap();
    fs::write(folder.join(r"screen\screen.ps1"), "script").unwrap();
    // Everyone was let in before, on a file and on a folder of its own.
    for path in [folder.join(r"screen\screen.ps1"), folder.join("screen")] {
        let granted = Command::new("icacls").arg(&path).args(["/grant", "*S-1-1-0:(F)"]).output().unwrap();
        assert!(granted.status.success());
    }
    junction(&folder.join("share"), &scratch.host);
    fs::write(scratch.host.join("kept.txt"), "kept").unwrap();
    let sid = own_sid();
    let grants = [("SY", "FA"), ("BA", "FA"), (sid.as_str(), "FA"), ("OW", "RC")];
    let volume = scratch.open();

    volume.give_only("Guesthouse", &grants).unwrap();
    let top = dacl_of(&folder);
    assert!(top.starts_with("D:P"), "the folder's permissions are its own: {top}");
    assert!(top.contains("(A;OICI;FA;;;SY)") && top.contains("(A;OICI;FA;;;BA)"), "{top}");
    for path in [folder.join("screen"), folder.join(r"screen\screen.ps1")] {
        let dacl = dacl_of(&path);
        assert!(!dacl.contains(";;;WD)") && !dacl.contains(";;;AU)") && !dacl.contains(";;;BU)"), "{dacl}");
        assert!(dacl.contains("ID;FA;;;SY)"), "taken from the folder: {dacl}");
    }
    assert_eq!(volume.entry(r"Guesthouse\share").unwrap(), None, "the link went");
    assert_eq!(fs::read_to_string(scratch.host.join("kept.txt")).unwrap(), "kept");

    volume.write(r"Guesthouse\report.ps1", "new").unwrap();
    let new = dacl_of(&folder.join("report.ps1"));
    assert!(
        new.contains(";FA;;;SY)")
            && !new.contains(";;;AU)")
            && !new.contains(";;;BU)")
            && !new.contains(";;;WD)"),
        "a new file takes the folder's: {new}"
    );

    // A folder that's missing is made, and one that's a link is made in its place.
    fs::remove_dir_all(&folder).unwrap();
    junction(&folder, &scratch.host);
    volume.give_only("Guesthouse", &grants).unwrap();
    assert!(volume.is_dir("Guesthouse"));
    assert!(dacl_of(&folder).starts_with("D:P"));
    assert_eq!(fs::read_to_string(scratch.host.join("kept.txt")).unwrap(), "kept");
    assert!(!dacl_of(&scratch.host).starts_with("D:P"), "what the link named was left as it was");
}

#[test]
fn a_compressed_file_is_a_file_and_a_junction_is_still_refused() {
    let scratch = Scratch::new("compressed");
    let file = scratch.volume.join("big.dll");
    fs::write(&file, vec![b'a'; 200_000]).unwrap();
    let compacted = Command::new("compact").arg("/c").arg("/exe:xpress4k").arg(&file).output().unwrap();
    assert!(compacted.status.success(), "{}", String::from_utf8_lossy(&compacted.stdout));
    junction(&scratch.volume.join("linked"), &scratch.host);
    let volume = scratch.open();
    assert!(matches!(volume.entry("big.dll").unwrap(), Some(Entry::File { len: 200_000, .. })));
    assert_eq!(volume.read("big.dll", 300_000).unwrap(), vec![b'a'; 200_000]);
    volume.write("big.dll", "replaced").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "replaced");
    assert_eq!(volume.entry("linked").unwrap(), Some(Entry::Link));
    assert!(volume.write(r"linked\x", "x").is_err());
    assert_eq!(fs::read_dir(&scratch.host).unwrap().count(), 0);
}

/// Sets a reparse point of a tag no filter handles and that redirects no name on `path`, as a
/// file system filter's own might be. `None` when this account may not set one.
fn set_plain_reparse_point(path: &Path) -> Option<()> {
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
    // Not Microsoft's, and not a name surrogate.
    const TAG: u32 = 0x0000_1234;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    // REPARSE_GUID_DATA_BUFFER: tag, data length, reserved, GUID, then 4 bytes of data.
    let mut buffer = Vec::new();
    buffer.extend(TAG.to_le_bytes());
    buffer.extend(4u16.to_le_bytes());
    buffer.extend(0u16.to_le_bytes());
    buffer.extend([0x47u8; 16]);
    buffer.extend([1u8, 2, 3, 4]);
    let mut returned = 0u32;
    // SAFETY: the buffer is as long as the length given.
    let set = unsafe {
        windows_sys::Win32::System::IO::DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            null_mut(),
            0,
            &mut returned,
            null_mut(),
        )
    };
    (set != 0).then_some(())
}

#[test]
fn a_reparse_point_that_redirects_no_name_is_a_plain_file_or_folder() {
    let scratch = Scratch::new("plain-reparse");
    let folder = scratch.volume.join("placeholder");
    fs::create_dir_all(&folder).unwrap();
    let file = scratch.volume.join("tagged.bin");
    fs::write(&file, "data").unwrap();
    if set_plain_reparse_point(&folder).is_none() || set_plain_reparse_point(&file).is_none() {
        panic!("this account can't set a reparse point: {}", io::Error::last_os_error());
    }
    let volume = scratch.open();
    assert_eq!(volume.entry("placeholder").unwrap(), Some(Entry::Folder));
    assert!(matches!(volume.entry("tagged.bin").unwrap(), Some(Entry::File { len: 4, .. })));
    assert_eq!(volume.read("tagged.bin", 100).unwrap(), b"data");
    let listed = volume.read_dir("").unwrap();
    assert!(
        listed.iter().any(|(name, entry)| name == "placeholder" && *entry == Entry::Folder),
        "{listed:?}"
    );
    assert!(listed.iter().any(|(name, entry)| name == "tagged.bin" && matches!(entry, Entry::File { .. })));
    volume.write("tagged.bin", "replaced").unwrap();
    assert_eq!(volume.read("tagged.bin", 100).unwrap(), b"replaced");
    volume.remove_tree("placeholder").unwrap();
    assert_eq!(volume.entry("placeholder").unwrap(), None);
}
