//! Says what an ISO is by looking inside it. Windows ISOs are UDF, which isoinfo can't read, so this
//! asks 7z, the one tool that reads both.

use crate::failure::{Failure, Result};
use crate::machine::Kind;
use crate::vm_args::on_path;
use std::collections::HashMap;
use std::io::{ErrorKind, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// These take the same words for what's asked of them here. Arch has no `genisoimage`.
const ISO_MAKERS: [&str; 3] = ["genisoimage", "mkisofs", "xorrisofs"];

/// The program that makes an ISO on this PC. With none, the first is named, and fails as itself.
pub fn iso_maker() -> &'static str {
    ISO_MAKERS.iter().copied().find(|program| on_path(program)).unwrap_or(ISO_MAKERS[0])
}

pub fn any_iso_maker() -> bool {
    ISO_MAKERS.iter().any(|program| on_path(program))
}

use guesthouse_answers_core::linux_host::editions::same_edition;
/// The Windows 11 editions a guest runs here and the key that picks each, and the edition the answer
/// file installs when the disc has no other: the Linux host's table, in the shared crate.
pub use guesthouse_answers_core::linux_host::editions::{WINDOWS_EDITION, WINDOWS_EDITIONS, edition_key};
/// A WIM's XML block and the images it lists, read as the shared crate reads them.
use guesthouse_answers_core::linux_host::wim::{FIRST_WINDOWS_11_BUILD, WIM_AMD64, images_in, wim_xml};
#[cfg(test)]
use guesthouse_answers_core::linux_host::wim::{
    MOST_WIM_XML, WIM_HEADER_SIZE, WIM_MAGIC, WIM_XML_ENTRY, field, utf16_text,
};
const READ_SECONDS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iso {
    /// Windows, or plain for anything that doesn't install by itself.
    pub kind: Kind,
    /// What it is, in a few words.
    pub title: String,
    pub edition: String,
    /// Whether it's a Windows disc, of any edition. A plain VM made from one still gets a TPM and
    /// Secure Boot, as Windows 11's setup refuses a PC without them.
    pub windows_disc: bool,
}

const UBUNTU_KERNEL: &str = "casper/vmlinuz";
const UBUNTU_INITRD: &str = "casper/initrd";
/// Only the server ISO has its kernel here, and the install needs the exact one it holds.
const KERNEL_POOL: &str = "pool/main/l/linux-signed/";

/// The generic kernel's package name, from a file in the ISO's pool.
pub fn kernel_package(path: &str) -> Option<&str> {
    let name = path.trim_start_matches('/').strip_prefix(KERNEL_POOL)?;
    let (package, _) = name.split_once('_')?;
    let version = package.strip_prefix("linux-image-")?.strip_suffix("-generic")?;
    version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.' || byte == b'-').then_some(package)
}

/// What an ISO with Ubuntu's layout is, from the names in it. `None` if it isn't Ubuntu.
fn read_ubuntu(names: &HashMap<String, &str>) -> Option<Iso> {
    if !names.contains_key(UBUNTU_KERNEL) || !names.contains_key(UBUNTU_INITRD) {
        return None;
    }
    let found = names.keys().filter_map(|name| kernel_package(name)).min();
    Some(found.map_or_else(
        || Iso::plain("Ubuntu, but not the server ISO that installs by itself"),
        |package| Iso {
            kind: Kind::Linux,
            title: format!("Ubuntu server, {package}"),
            edition: package.to_owned(),
            windows_disc: false,
        },
    ))
}

impl Iso {
    fn plain(title: &str) -> Self {
        Self { kind: Kind::Plain, title: title.to_owned(), edition: String::new(), windows_disc: false }
    }

    /// A Windows disc that doesn't install by itself.
    fn plain_windows(title: &str) -> Self {
        Self { windows_disc: true, ..Self::plain(title) }
    }
}

fn wim_images(path: &str, entry: &str) -> Vec<(String, String, u32, String)> {
    let child = Command::new("7z")
        .args(["x", "-so", path, entry])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return Vec::new() };
    let found = child.stdout.take().and_then(|mut stream| wim_xml(&mut stream));
    let _ = child.kill();
    let _ = child.wait();
    found.as_deref().map(images_in).unwrap_or_default()
}

/// 7z's listing of a disc, held to a time limit as a damaged file can take for ever.
fn list_disc(path: &str) -> Result<String> {
    let child = Command::new("7z")
        .args(["l", "-slt", path])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = child.map_err(|error| match error.kind() {
        ErrorKind::NotFound => Failure::new("Reading an ISO needs 7z, which is not installed."),
        _ => error.into(),
    })?;
    let mut stdout = child.stdout.take().ok_or_else(|| Failure::new("7z gave no output."))?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stdout.read_to_end(&mut text);
        let _ = sender.send(text);
    });
    let Ok(text) = receiver.recv_timeout(Duration::from_secs(READ_SECONDS)) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Failure::new("The ISO took too long to read."));
    };
    let status = child.wait()?;
    let text = String::from_utf8_lossy(&text).into_owned();
    let is_disc = text.lines().any(|line| line == "Type = Iso" || line == "Type = Udf");
    if !status.success() || !is_disc {
        return Err(Failure::new("This file is not a disc image."));
    }
    Ok(text)
}

/// How far to look into a Windows disc.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    /// As far as its editions, which means reading through the install image: some gigabytes, and
    /// minutes on a slow drive.
    Editions,
    /// Only that it's Windows.
    Names,
}

/// Says what an ISO is.
pub fn read_iso(path: &str) -> Result<Iso> {
    read_disc(path, Look::Editions)
}

/// Says what an ISO is on a PC where a Windows disc makes a plain VM whatever its edition. The edition
/// would change nothing there, so the install image isn't read.
pub fn read_iso_for_plain_windows(path: &str) -> Result<Iso> {
    read_disc(path, Look::Names)
}

fn read_disc(path: &str, look: Look) -> Result<Iso> {
    let listing = list_disc(path)?;
    let names: HashMap<String, &str> = listing
        .lines()
        .filter_map(|line| line.strip_prefix("Path = "))
        .map(|name| (name.to_lowercase(), name))
        .collect();
    let entry = names.get("sources/install.wim").or_else(|| names.get("sources/install.esd"));
    let Some(entry) = entry else {
        return Ok(read_ubuntu(&names).unwrap_or_else(|| Iso::plain("Not Windows")));
    };
    if look == Look::Names {
        return Ok(Iso::plain_windows("Windows"));
    }
    let images = wim_images(path, entry);
    // The best edition the disc has, not the first image on it: a disc that carries Home and Pro
    // installs Pro, as it did when Pro was the only one looked for. The answer file names the image
    // by its own name, which is what Setup's /IMAGE/NAME matches. The display name can be in the
    // disc's language, so it is only for the title.
    for (edition, _) in WINDOWS_EDITIONS {
        let found = images.iter().find(|(name, _, build, architecture)| {
            same_edition(name, edition) && *build >= FIRST_WINDOWS_11_BUILD && architecture == WIM_AMD64
        });
        if let Some((name, shown, build, _)) = found {
            let title = format!("{shown}, build {build}");
            return Ok(Iso { kind: Kind::Windows, title, edition: name.clone(), windows_disc: true });
        }
    }
    Ok(Iso::plain_windows("Windows, but not 64-bit Windows 11 Pro or IoT Enterprise LTSC"))
}

#[cfg(test)]
pub mod stand_in {
    use super::*;
    use crate::testing::Scratch;
    use std::fs;

    /// Enough of a WIM for `read_iso`: its header, then the XML block where the header says.
    pub fn wim(names: &[&str], build: u32, architecture: u32) -> Vec<u8> {
        const XML_AT: usize = 4096;
        let images: String = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                format!(
                    "<IMAGE INDEX=\"{index}\"><NAME>{name}</NAME><WINDOWS><ARCH>{architecture}</ARCH>\
                     <VERSION><BUILD>{build}</BUILD></VERSION></WINDOWS></IMAGE>"
                )
            })
            .collect();
        let xml: Vec<u8> = format!("<WIM>{images}</WIM>").encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut file = vec![0; XML_AT];
        file[..8].copy_from_slice(WIM_MAGIC);
        file[WIM_XML_ENTRY..WIM_XML_ENTRY + 8].copy_from_slice(&(xml.len() as u64 | 1 << 56).to_le_bytes());
        file[WIM_XML_ENTRY + 8..WIM_XML_ENTRY + 16].copy_from_slice(&(XML_AT as u64).to_le_bytes());
        file.extend(xml);
        file
    }

    pub fn make_iso(scratch: &Scratch, name: &str, files: &[(&str, &[u8])]) -> String {
        for (path, data) in files {
            scratch.write(&format!("{name}.tree/{path}"), data);
        }
        let iso = scratch.path(&format!("{name}.iso"));
        let tree = scratch.path(&format!("{name}.tree"));
        let done = Command::new("genisoimage")
            .args(["-quiet", "-J", "-r", "-udf", "-o"])
            .arg(&iso)
            .arg(&tree)
            .status();
        assert!(done.unwrap().success());
        fs::remove_dir_all(tree).unwrap();
        iso.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::{make_iso, wim};
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn a_windows_11_pro_disc_is_known_from_inside() {
        let scratch = Scratch::new();
        let data = wim(&["Windows 11 Home", "Windows 11 Pro"], 26300, 9);
        let found = read_iso(&make_iso(&scratch, "plain-name", &[("sources/install.wim", &data)])).unwrap();
        assert_eq!((found.kind, found.edition.as_str()), (Kind::Windows, "Windows 11 Pro"));
        assert!(found.title.contains("26300"));
        let data = wim(&["Windows 11 Pro"], 26300, 9);
        let esd = make_iso(&scratch, "esd", &[("sources/install.esd", &data)]);
        assert_eq!(read_iso(&esd).unwrap().kind, Kind::Windows, "an ESD counts too");
    }

    #[test]
    fn an_iot_ltsc_disc_is_windows_and_keeps_its_own_edition() {
        let scratch = Scratch::new();
        // The image the IoT LTSC media carries, which is the whole disc: no Pro on it at all.
        let data = wim(&["Windows 11 IoT Enterprise LTSC"], 26100, 9);
        let found = read_iso(&make_iso(&scratch, "iot-ltsc", &[("sources/install.wim", &data)])).unwrap();
        assert_eq!((found.kind, found.edition.as_str()), (Kind::Windows, "Windows 11 IoT Enterprise LTSC"));
        assert_eq!(edition_key("Windows 11 IoT Enterprise LTSC"), Some("KBN8V-HFGQ4-MGXVD-347P6-PDQGT"));
        assert_eq!(edition_key("Windows 11 Enterprise LTSC"), None, "an edition not run here has no key");
    }

    #[test]
    fn the_edition_is_the_images_own_name_and_the_title_its_display_name() {
        let scratch = Scratch::new();
        let mut data = wim(&["Windows 11 IoT Enterprise LTSC 2024"], 26100, 9);
        let shown = "<DISPLAYNAME>Windows 11 IoT Enterprise LTSC</DISPLAYNAME>";
        let xml = String::from_utf16_lossy(
            &data[4096..].chunks(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect::<Vec<_>>(),
        )
        .replace("</NAME>", &format!("</NAME>{shown}"));
        let xml: Vec<u8> = xml.encode_utf16().flat_map(u16::to_le_bytes).collect();
        data.truncate(4096);
        data[WIM_XML_ENTRY..WIM_XML_ENTRY + 8].copy_from_slice(&(xml.len() as u64 | 1 << 56).to_le_bytes());
        data.extend(xml);
        let found = read_iso(&make_iso(&scratch, "year", &[("sources/install.wim", &data)])).unwrap();
        assert_eq!(
            found.edition, "Windows 11 IoT Enterprise LTSC 2024",
            "Setup matches the image's own name"
        );
        assert_eq!(found.title, "Windows 11 IoT Enterprise LTSC, build 26100");
        assert_eq!(edition_key(&found.edition), Some("KBN8V-HFGQ4-MGXVD-347P6-PDQGT"));
    }

    #[test]
    fn an_enterprise_ltsc_disc_is_still_a_plain_vm() {
        let scratch = Scratch::new();
        let data = wim(&["Windows 11 Enterprise LTSC"], 26100, 9);
        let found = read_iso(&make_iso(&scratch, "ltsc", &[("sources/install.wim", &data)])).unwrap();
        assert_eq!(
            (found.kind, found.edition.as_str()),
            (Kind::Plain, ""),
            "no install has been run with it here"
        );
    }

    #[test]
    fn a_disc_with_pro_and_an_ltsc_image_installs_pro() {
        let scratch = Scratch::new();
        let data = wim(&["Windows 11 IoT Enterprise LTSC", "Windows 11 Pro"], 26100, 9);
        let found = read_iso(&make_iso(&scratch, "both", &[("sources/install.wim", &data)])).unwrap();
        assert_eq!(found.edition, "Windows 11 Pro", "Pro is still what a multi-edition disc gives");
    }

    #[test]
    fn anything_else_is_a_plain_vm_whatever_its_name_says() {
        let scratch = Scratch::new();
        let enterprise = wim(&["Windows 11 Enterprise Evaluation"], 26300, 9);
        let ten = wim(&["Windows 11 Pro"], 19045, 9);
        let arm = wim(&["Windows 11 Pro"], 26300, 12);
        let cases: [(&str, &str, &[u8]); 5] = [
            ("windows_11_pro_x64", "boot/grub.cfg", b"linux"),
            ("no-pro", "sources/install.wim", &enterprise),
            ("ten", "sources/install.wim", &ten),
            ("arm", "sources/install.wim", &arm),
            ("broken", "sources/install.wim", b"not a wim"),
        ];
        for (name, path, data) in cases {
            assert_eq!(
                read_iso(&make_iso(&scratch, name, &[(path, data)])).unwrap().kind,
                Kind::Plain,
                "{name}"
            );
        }
    }

    #[test]
    fn where_windows_is_a_plain_vm_its_install_image_is_not_read() {
        let scratch = Scratch::new();
        // Not a WIM at all, so reading it would say it isn't Windows 11 Pro.
        let windows = make_iso(&scratch, "windows", &[("sources/install.wim", b"never read")]);
        let found = read_iso_for_plain_windows(&windows).unwrap();
        assert_eq!((found.kind, found.title.as_str()), (Kind::Plain, "Windows"));
        assert!(found.windows_disc);
        let package = "pool/main/l/linux-signed/linux-image-7.0.0-38-generic_7.0.0-38.38_amd64.deb";
        let files: [(&str, &[u8]); 3] =
            [("casper/vmlinuz", b"kernel"), ("casper/initrd", b"initrd"), (package, b"deb")];
        let ubuntu = make_iso(&scratch, "server", &files);
        assert_eq!(read_iso_for_plain_windows(&ubuntu).unwrap(), read_iso(&ubuntu).unwrap());
    }

    #[test]
    fn the_server_iso_is_known_by_its_kernel() {
        let scratch = Scratch::new();
        let package = "pool/main/l/linux-signed/linux-image-7.0.0-38-generic_7.0.0-38.38_amd64.deb";
        let files: [(&str, &[u8]); 3] =
            [("casper/vmlinuz", b"kernel"), ("casper/initrd", b"initrd"), (package, b"deb")];
        let found = read_iso(&make_iso(&scratch, "server", &files)).unwrap();
        assert_eq!((found.kind, found.edition.as_str()), (Kind::Linux, "linux-image-7.0.0-38-generic"));
    }

    #[test]
    fn a_desktop_iso_says_what_is_needed_instead() {
        let scratch = Scratch::new();
        let path =
            make_iso(&scratch, "desktop", &[("casper/vmlinuz", b"kernel"), ("casper/initrd", b"initrd")]);
        let found = read_iso(&path).unwrap();
        assert_eq!(found.kind, Kind::Plain);
        assert!(found.title.contains("server ISO"));
    }

    #[test]
    fn only_a_generic_kernel_counts() {
        let pool = "pool/main/l/linux-signed";
        let wanted = Some("linux-image-7.0.0-38-generic");
        assert_eq!(kernel_package(&format!("{pool}/linux-image-7.0.0-38-generic_7.0.0_amd64.deb")), wanted);
        assert_eq!(kernel_package(&format!("/{pool}/linux-image-7.0.0-38-generic_7.0.0_amd64.deb")), wanted);
        assert_eq!(kernel_package(&format!("{pool}/linux-image-7.0.0-38-lowlatency_7.0.0_amd64.deb")), None);
        assert_eq!(kernel_package("elsewhere/linux-image-7.0.0-38-generic_7.0.0_amd64.deb"), None);
    }

    #[test]
    fn a_file_that_is_no_disc_is_refused() {
        let scratch = Scratch::new();
        let path = scratch.write("text.iso", "this is not a disc").display().to_string();
        assert!(read_iso(&path).unwrap_err().0.contains("not a disc image"));
    }

    #[test]
    fn a_build_that_is_no_number_is_passed_over() {
        let xml = "<IMAGE INDEX=\"1\"><NAME>x</NAME><BUILD>many</BUILD><ARCH>9</ARCH></IMAGE>";
        assert!(images_in(xml).is_empty());
    }
}

#[cfg(test)]
#[path = "iso_golden.rs"]
mod answers_golden;
