//! Files the app fetches itself, so a machine needs nothing found by hand: Ubuntu's installer disc, and
//! an Android machine's image. Each is downloaded once into the fetched parts' folder, going on from where
//! a stopped download left off, and is only kept if its SHA-256 is the one its publisher gives.

use crate::failure::{Failure, Result};
use crate::paths::Paths;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

const POLL: Duration = Duration::from_millis(500);
/// The partial download beside where the file goes.
const PARTIAL: &str = "part";
/// The SHA-256 a finished file was checked against, beside it, so a later use needn't read it again.
const CHECKED: &str = "sha256";
const MIB: u64 = 1024 * 1024;

/// Ubuntu's release folder for the version the Ubuntu install is made for, and its checksum list.
const UBUNTU_RELEASE: &str = "https://releases.ubuntu.com/26.04/";
const UBUNTU_SUMS: &str = "SHA256SUMS";
/// The server disc's name begins and ends so, with the point release between.
const UBUNTU_PREFIX: &str = "ubuntu-26.04";
const UBUNTU_SUFFIX: &str = "-live-server-amd64.iso";
const UBUNTU_FOLDER: &str = "ubuntu";

fn said(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

fn curl() -> Command {
    let mut command = Command::new("curl");
    command.args(["--fail", "--location", "--silent", "--show-error", "--retry", "3"]);
    command.stdin(Stdio::null());
    command
}

/// A small text file's contents, such as a checksum list.
pub fn text_at(url: &str) -> Result<String> {
    let output =
        curl().arg(url).output().map_err(|error| Failure(format!("curl could not be started: {error}")))?;
    if !output.status.success() {
        return Err(Failure(format!("{url} could not be read: {}", said(&output))));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A file's size as its server says it, if it does.
pub fn size_at(url: &str) -> Option<u64> {
    let output = curl().args(["--head", url]).output().ok()?;
    let headers = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    // After redirects, the last length given is the file's.
    headers
        .lines()
        .filter_map(|line| line.strip_prefix("content-length:"))
        .filter_map(|value| value.trim().parse().ok())
        .next_back()
}

pub fn sha256(file: &Path) -> Result<String> {
    let output = Command::new("sha256sum").arg(file).stdin(Stdio::null()).output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().next().map(str::to_owned).ok_or_else(|| Failure(said(&output)))
}

/// A size in GiB or MiB, as said before a download.
pub fn size_text(bytes: u64) -> String {
    if bytes >= 1024 * MIB {
        format!("{:.1} GB", bytes as f64 / (1024 * MIB) as f64)
    } else {
        format!("{} MB", bytes.div_ceil(MIB))
    }
}

/// Downloads `url` to `to`, going on from what an earlier try left, and tells `step` how far it is as
/// "`what`: N%".
fn download(url: &str, to: &Path, bytes: Option<u64>, what: &str, step: &dyn Fn(&str)) -> Result<()> {
    let mut child = curl()
        .args(["--continue-at", "-", "--output"])
        .arg(to)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failure(format!("curl could not be started: {error}")))?;
    let mut shown = None;
    while child.try_wait()?.is_none() {
        let have = fs::metadata(to).map_or(0, |found| found.len());
        let text = match bytes {
            Some(total) => format!("{what}: {}%", (have * 100 / total.max(1)).min(100)),
            None => format!("{what}: {}", size_text(have)),
        };
        if shown.as_ref() != Some(&text) {
            step(&text);
            shown = Some(text);
        }
        sleep(POLL);
    }
    let ended = child.wait_with_output()?;
    if ended.status.success() {
        return Ok(());
    }
    Err(Failure(format!("{what} stopped: {}", said(&ended))))
}

/// Whether `file` is there and was already checked against `wanted`.
fn already_checked(file: &Path, wanted: &str) -> bool {
    file.is_file() && fs::read_to_string(file.with_extension(CHECKED)).is_ok_and(|sum| sum.trim() == wanted)
}

/// Fetches `url` to `file` unless it's already there and checked, and keeps it only if its SHA-256 is
/// `wanted`.
pub fn fetch_checked(url: &str, wanted: &str, file: &Path, what: &str, step: &dyn Fn(&str)) -> Result<()> {
    if already_checked(file, wanted) {
        return Ok(());
    }
    fs::create_dir_all(file.parent().unwrap_or(Path::new("")))?;
    let partial = file.with_extension(PARTIAL);
    step(what);
    download(url, &partial, size_at(url), what, step)?;
    step("Checking the download");
    if sha256(&partial)? != wanted {
        let _ = fs::remove_file(&partial);
        return Err(Failure(format!("{what} gave a file that isn't the one it should be. Try again.")));
    }
    fs::rename(&partial, file)?;
    fs::write(file.with_extension(CHECKED), wanted)?;
    Ok(())
}

/// The newest point release's server disc in Ubuntu's checksum list, and its SHA-256.
fn newest_server_disc(sums: &str) -> Option<(String, String)> {
    let mut discs: Vec<(String, String)> = sums
        .lines()
        .filter_map(|line| {
            let (sum, name) = line.split_once(' ')?;
            let name = name.trim().trim_start_matches('*');
            (name.starts_with(UBUNTU_PREFIX) && name.ends_with(UBUNTU_SUFFIX))
                .then(|| (name.to_owned(), sum.to_owned()))
        })
        .collect();
    // 26.04 sorts before 26.04.1, which sorts before 26.04.2.
    discs.sort_by(|one, other| {
        let point = |name: &str| {
            let middle = &name[UBUNTU_PREFIX.len()..name.len() - UBUNTU_SUFFIX.len()];
            middle.trim_start_matches('.').parse::<u32>().unwrap_or(0)
        };
        point(&one.0).cmp(&point(&other.0))
    });
    discs.pop()
}

/// Ubuntu's server disc that an Ubuntu machine installs from, fetched if it isn't here.
pub struct UbuntuDisc {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub file: PathBuf,
}

/// The disc an Ubuntu machine would download now: what Ubuntu publishes, and where it's kept here.
pub fn ubuntu_disc(paths: &Paths) -> Result<UbuntuDisc> {
    let sums = text_at(&format!("{UBUNTU_RELEASE}{UBUNTU_SUMS}"))?;
    let (name, sha256) = newest_server_disc(&sums)
        .ok_or_else(|| Failure::new("Ubuntu's release page lists no server disc for 26.04."))?;
    Ok(UbuntuDisc {
        url: format!("{UBUNTU_RELEASE}{name}"),
        file: paths.parts.join(UBUNTU_FOLDER).join(&name),
        name,
        sha256,
    })
}

/// An Ubuntu disc already downloaded and checked, without asking the network.
pub fn ubuntu_disc_here(paths: &Paths) -> Option<PathBuf> {
    let folder = paths.parts.join(UBUNTU_FOLDER);
    let mut found: Vec<PathBuf> = fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "iso") && path.with_extension(CHECKED).is_file()
        })
        .collect();
    found.sort();
    found.pop()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn the_newest_server_disc_is_taken_from_ubuntu_s_list() {
        let sums = "aa *ubuntu-26.04-desktop-amd64.iso\nbb *ubuntu-26.04-live-server-amd64.iso\n\
                    cc *ubuntu-26.04.1-live-server-amd64.iso\ndd *ubuntu-26.04.1-wsl-amd64.wsl\n";
        assert_eq!(
            newest_server_disc(sums),
            Some(("ubuntu-26.04.1-live-server-amd64.iso".to_owned(), "cc".to_owned()))
        );
        assert_eq!(newest_server_disc("aa *ubuntu-26.04-desktop-amd64.iso\n"), None);
    }

    #[test]
    fn a_file_is_kept_only_if_it_s_the_one_it_should_be() {
        let scratch = Scratch::new();
        let source = scratch.path("source.iso");
        fs::write(&source, "disc").unwrap();
        let url = format!("file://{}", source.display());
        let wanted = sha256(&source).unwrap();
        let file = scratch.path("parts/ubuntu/disc.iso");
        assert!(fetch_checked(&url, "not-it", &file, "Downloading", &|_| {}).is_err());
        assert!(!file.exists() && !file.with_extension(PARTIAL).exists());
        fetch_checked(&url, &wanted, &file, "Downloading", &|_| {}).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "disc");
        fs::remove_file(&source).unwrap();
        fetch_checked(&url, &wanted, &file, "Downloading", &|_| {}).unwrap();
    }
}
