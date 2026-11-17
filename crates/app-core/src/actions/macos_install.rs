//! A macOS machine's install, as far as the app can take it: the boot image, Apple's installer, and a
//! first start into it. From there macOS is installed by hand in its window, as on a real Mac that
//! was wiped.
//!
//! Each part is made under another name and moved into place, and one that's there is kept. So an
//! install that's run again goes on where it stopped, and the machine stays the Mac it was first
//! made as: a second boot image would be another Mac to Apple.

use super::Task;
use super::macos::start;
use crate::failure::{Failure, Result};
use crate::macos_part::{self, KEXT_SET, KEXTS_FOLDER, REIMS_UTIL};
use crate::macos_setup::{FIRMWARE_CODE_FILE, FIRMWARE_VARS_FILE, MACOS_VERSION};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the boot image and its firmware are made, before they're the machine's.
const BOOT_PARTS: &str = "boot.part";
/// Apple's installers, kept for the next machine. A download that stopped goes on from here.
const INSTALLERS: &str = "macos-installers";
const PART: &str = "part";
const NO_RESULT: &str = "reims-util ended without saying what it made.";

/// The `--add-kext` words for the guest's drivers, in the order their list has them, each with the
/// kernel versions it loads on if the list names any. The list is OpenCore's own keys, one
/// `[[kext]]` a driver.
fn kext_words(list: &str, folder: &Path) -> Vec<String> {
    let value = |line: &str, key: &str| {
        let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?.trim();
        Some(rest.trim_matches('"').to_owned())
    };
    let mut kexts: Vec<[String; 3]> = Vec::new();
    for line in list.lines().map(str::trim) {
        if line == "[[kext]]" {
            kexts.push(Default::default());
        } else if let Some(kext) = kexts.last_mut() {
            for (at, key) in ["BundlePath", "MinKernel", "MaxKernel"].into_iter().enumerate() {
                if let Some(found) = value(line, key) {
                    kext[at] = found;
                }
            }
        }
    }
    let mut words = Vec::new();
    for [bundle, least, most] in kexts.into_iter().filter(|kext| !kext[0].is_empty()) {
        let mut word = folder.join(bundle).display().to_string();
        match (least.as_str(), most.as_str()) {
            ("", "") => {}
            (least, "") => word += &format!(",{least}"),
            ("", most) => word += &format!(",0.0.0,{most}"),
            (least, most) => word += &format!(",{least},{most}"),
        }
        words.extend(["--add-kext".to_owned(), word]);
    }
    words
}

/// Runs reims-util, telling the task how far each step is, and gives back what it says it made.
fn reims_util(task: &Task, doing: &str, words: &[String]) -> Result<Value> {
    let mut child = Command::new(macos_part::file(&task.context.paths, REIMS_UTIL))
        .arg("--json")
        .args(words)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let output = child.stdout.take().ok_or_else(|| Failure::new(NO_RESULT))?;
    let (mut made, mut refusal, mut shown) = (None, None, None);
    for line in BufReader::new(output).lines() {
        let Ok(event) = serde_json::from_str::<Value>(&line?) else { continue };
        match event["event"].as_str() {
            Some("progress") => {
                let (done, total) = (event["done"].as_u64(), event["total"].as_u64());
                let percent =
                    done.zip(total).filter(|(_, total)| *total > 0).map(|(done, total)| done * 100 / total);
                if percent.is_some() && percent != shown {
                    shown = percent;
                    task.step(&format!("{doing}: {}%", percent.unwrap_or(0)));
                }
            }
            Some("done") => made = Some(event["result"].clone()),
            Some("error") => refusal = event["message"].as_str().map(str::to_owned),
            _ => {}
        }
    }
    let mut said = String::new();
    if let Some(mut errors) = child.stderr.take() {
        let _ = errors.read_to_string(&mut said);
    }
    let ended = child.wait()?;
    match (made, refusal) {
        (Some(made), None) if ended.success() => Ok(made),
        (_, Some(refusal)) => Err(Failure(refusal)),
        _ if said.trim().is_empty() => Err(Failure::new(NO_RESULT)),
        _ => Err(Failure(said.trim().to_owned())),
    }
}

/// The boot image, with the guest's drivers in it, and the firmware it was made for. The image is
/// moved into place last, so one that's there has its firmware beside it.
fn make_boot_image(task: &Task, folder: &Path) -> Result<()> {
    let opencore = PathBuf::from(&task.machine.opencore);
    if opencore.exists() {
        return Ok(());
    }
    task.step("Making the Mac's boot image");
    let parts = folder.join(BOOT_PARTS);
    let _ = fs::remove_dir_all(&parts);
    fs::create_dir_all(&parts)?;
    let image = parts.join("opencore.qcow2");
    let kexts = macos_part::file(&task.context.paths, KEXTS_FOLDER);
    let list = fs::read_to_string(kexts.join(KEXT_SET))?;
    // A Mac's ROM is its first network card's address.
    let mut words: Vec<String> =
        ["opencore", "-o", &image.to_string_lossy(), "--rom", &task.machine.mac].map(str::to_owned).to_vec();
    words.extend(kext_words(&list, &kexts));
    reims_util(task, "Making the Mac's boot image", &words)?;
    fs::rename(parts.join(FIRMWARE_CODE_FILE), folder.join(FIRMWARE_CODE_FILE))?;
    fs::rename(parts.join(FIRMWARE_VARS_FILE), folder.join(FIRMWARE_VARS_FILE))?;
    fs::rename(image, opencore)?;
    let _ = fs::remove_dir_all(parts);
    Ok(())
}

/// Apple's installer as a disk QEMU reads. Apple serves a compressed image, which is kept as it
/// came for the next machine, and written out in full for this one.
fn make_installer(task: &Task) -> Result<()> {
    let installer = PathBuf::from(&task.machine.iso);
    if installer.exists() {
        return Ok(());
    }
    let doing = "Downloading macOS from Apple";
    task.step(doing);
    let kept = task.context.paths.store.join(INSTALLERS);
    fs::create_dir_all(&kept)?;
    let words = ["online-installer", MACOS_VERSION, "-o", &kept.to_string_lossy()].map(str::to_owned);
    let made = reims_util(task, doing, &words)?;
    let image = made["image"].as_str().ok_or_else(|| Failure::new(NO_RESULT))?;
    task.step("Getting the installer ready");
    let part = installer.with_extension(PART);
    let written = Command::new("qemu-img")
        .args(["convert", "-f", "dmg", "-O", "raw", image])
        .arg(&part)
        .stdin(Stdio::null())
        .output()?;
    if !written.status.success() {
        let _ = fs::remove_file(&part);
        let said = String::from_utf8_lossy(&written.stderr);
        return Err(Failure(format!("The installer could not be made ready: {}", said.trim())));
    }
    fs::rename(part, installer)?;
    Ok(())
}

pub fn install(task: &mut Task) -> Result<()> {
    macos_part::ensure(&task.context.paths, &|doing| task.step(doing))?;
    let folder = task.machine.folder(task.context);
    make_boot_image(task, &folder)?;
    make_installer(task)?;
    // From here it starts like any Mac, into its installer until that's taken out in its settings.
    task.machine.installing = false;
    task.machine.save(task.context)?;
    start(task)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "format = 1\nMinKernel = \"20.0.0\"\nMaxKernel = \"\"\n\n\
                        [[kext]]\nBundlePath = \"Graft.kext\"\nExecutablePath = \"Contents/MacOS/Graft\"\n\
                        MinKernel = \"20.0.0\"\nMaxKernel = \"\"\nComment = \"Binds the kexts\"\n\n\
                        [[kext]]\nBundlePath = \"Old.kext\"\nMinKernel = \"\"\nMaxKernel = \"24.9.9\"\n\n\
                        [[kext]]\nBundlePath = \"Any.kext\"\n";

    #[test]
    fn the_drivers_go_in_their_lists_order_with_the_kernels_they_load_on() {
        let words = kext_words(LIST, Path::new("/p/macos/kexts"));
        assert_eq!(
            words,
            [
                "--add-kext",
                "/p/macos/kexts/Graft.kext,20.0.0",
                "--add-kext",
                "/p/macos/kexts/Old.kext,0.0.0,24.9.9",
                "--add-kext",
                "/p/macos/kexts/Any.kext",
            ]
        );
        assert!(kext_words("format = 1\nMinKernel = \"20.0.0\"\n", Path::new("/k")).is_empty());
    }
}
