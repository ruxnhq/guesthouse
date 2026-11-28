// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;
use crate::install::fake::FakeNetwork;
use std::fs;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gh-drv-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest_with(files: &[(&str, &str)]) -> String {
    let entries: Vec<String> = files
        .iter()
        .map(|(name, sum)| format!(r#"{{"name":"p-{name}","as":"{name}","sha256":"{sum}"}}"#))
        .collect();
    format!(r#"{{"version":"1","files":[{}]}}"#, entries.join(","))
}

const SUM: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn a_manifest_must_name_each_needed_file_once_with_a_sha256() {
    let whole: Vec<(&str, &str)> = REQUIRED.iter().map(|n| (*n, SUM)).collect();
    assert!(Manifest::parse(&manifest_with(&whole)).is_ok());
    assert!(Manifest::parse(&manifest_with(&whole[1..])).unwrap_err().contains("install.ps1"));
    let mut twice = whole.clone();
    twice.push(("ruxnwddm.sys", SUM));
    assert!(Manifest::parse(&manifest_with(&twice)).unwrap_err().contains("2 times"));
    let mut short = whole.clone();
    short[0].1 = "abc";
    assert!(Manifest::parse(&manifest_with(&short)).unwrap_err().contains("SHA-256"));
    assert!(Manifest::parse("not json").is_err());
}

#[test]
fn a_name_that_leaves_its_folder_or_a_private_key_is_refused() {
    let mut files: Vec<(&str, &str)> = REQUIRED.iter().map(|n| (*n, SUM)).collect();
    files.push(("../escape", SUM));
    assert!(Manifest::parse(&manifest_with(&files)).unwrap_err().contains("not a file name"));
    let mut files: Vec<(&str, &str)> = REQUIRED.iter().map(|n| (*n, SUM)).collect();
    files.push(("ruxn-test.pfx", SUM));
    assert!(Manifest::parse(&manifest_with(&files)).unwrap_err().contains("private key"));
}

#[test]
fn a_fetched_package_is_whole_checked_and_found_again() {
    let downloads = scratch("fetch");
    let folder = fetch_package(&mut FakeNetwork, &downloads).unwrap();
    let files = package_files(&folder).unwrap();
    assert_eq!(files.len(), REQUIRED.len());
    assert_eq!(kept_package(&downloads), Some(folder.clone()));
    // A file changed after it was kept makes the package not whole.
    fs::write(folder.join("ruxnwddm.sys"), "changed").unwrap();
    assert_eq!(kept_package(&downloads), None);
    let _ = fs::remove_dir_all(downloads);
}

#[test]
fn a_folder_of_the_labs_is_held_to_the_same_files() {
    let dir = scratch("lab");
    assert!(package_files(&dir.join("none")).unwrap_err().contains("Build them first"));
    for name in &REQUIRED[1..] {
        fs::write(dir.join(name), "x").unwrap();
    }
    assert!(package_files(&dir).unwrap_err().contains("install.ps1"));
    fs::write(dir.join(SCRIPT), "x").unwrap();
    assert_eq!(package_files(&dir).unwrap().len(), REQUIRED.len());
    fs::write(dir.join("lab.pfx"), "x").unwrap();
    assert!(package_files(&dir).unwrap_err().contains("private key"));
    let _ = fs::remove_dir_all(dir);
}

fn keep_version(downloads: &Path, version: &str) {
    let folder = downloads.join("windows-driver").join(version);
    fs::create_dir_all(&folder).unwrap();
    let mut files = Vec::new();
    for name in REQUIRED {
        fs::write(folder.join(name), name).unwrap();
        files.push(format!(
            r#"{{"name":"p-{name}","as":"{name}","sha256":"{}"}}"#,
            crate::install::fetch::sha256_of(&folder.join(name)).unwrap()
        ));
    }
    let manifest = format!(r#"{{"version":"{version}","files":[{}]}}"#, files.join(","));
    fs::write(folder.join(MANIFEST_NAME), manifest).unwrap();
}

#[test]
fn the_newest_kept_package_is_the_highest_number_not_the_last_in_the_alphabet() {
    let downloads = scratch("newest");
    for version in ["0.3.9", "0.3.36", "0.10.0", "0.9.5"] {
        keep_version(&downloads, version);
    }
    let newest = kept_package(&downloads).unwrap();
    assert_eq!(newest.file_name().unwrap(), "0.10.0");
    // And a text sort would have said 0.9.5.
    fs::remove_dir_all(newest).unwrap();
    assert_eq!(kept_package(&downloads).unwrap().file_name().unwrap(), "0.9.5");
    let _ = fs::remove_dir_all(downloads);
}

/// The file site, as a folder: the files a release published, served at the base URL by name.
struct Published(PathBuf);

impl Published {
    fn path(&self, url: &str) -> Result<PathBuf, FetchError> {
        let name = url
            .strip_prefix(WINDOWS_BASE_URL)
            .ok_or_else(|| FetchError::new(format!("{url} is not under {WINDOWS_BASE_URL}")))?;
        Ok(self.0.join(name))
    }
}

impl Transport for Published {
    fn head(&mut self, url: &str) -> Result<fetch::Head, FetchError> {
        let length = fs::metadata(self.path(url)?).map_err(|e| FetchError::new(e.to_string()))?.len();
        Ok(fetch::Head { length: Some(length), ranges: true })
    }

    fn get_from(
        &mut self,
        url: &str,
        from: u64,
        out: &mut dyn std::io::Write,
        _progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), FetchError> {
        let bytes = fs::read(self.path(url)?).map_err(|e| FetchError::new(e.to_string()))?;
        out.write_all(&bytes[from as usize..]).map_err(|e| FetchError::new(e.to_string()))
    }

    fn text(&mut self, url: &str) -> Result<String, FetchError> {
        fs::read_to_string(self.path(url)?).map_err(|e| FetchError::new(e.to_string()))
    }
}

/// The release script that lays a package out, in this repository.
fn package_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../guest/drivers/windows-pvg/release/package.sh")
}

/// What `release/package.sh` makes is what the app fetches, by the names it publishes under, with
/// each file's SHA-256 in the manifest and in the `.sha256` beside it. With
/// GUESTHOUSE_TEST_PACKAGE_DIR naming a folder package.sh made (a real build, signed or not), that one
/// is used instead of stand-in files.
#[test]
fn a_package_the_release_script_made_is_what_the_app_fetches() {
    let script = package_script();
    let dir = scratch("package");
    let published = match std::env::var_os("GUESTHOUSE_TEST_PACKAGE_DIR") {
        Some(real) => PathBuf::from(real),
        None => {
            if !script.is_file() {
                eprintln!("the release script is not in this checkout; skipped");
                return;
            }
            let signed = dir.join("signed");
            fs::create_dir_all(&signed).unwrap();
            for name in &REQUIRED[1..] {
                fs::write(signed.join(name), format!("the file {name}")).unwrap();
            }
            fs::write(signed.join("ruxnwddm-full.inf"), "[Version]\nDriverVer = 11/26/2026,0.3.39.0\n")
                .unwrap();
            let out = dir.join("out");
            let made = std::process::Command::new("sh").arg(&script).arg(&signed).arg(&out).output().unwrap();
            assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
            out
        }
    };
    let text = fs::read_to_string(published.join(MANIFEST_NAME)).unwrap();
    let manifest = Manifest::parse(&text).unwrap();
    let version = manifest.version.clone();
    assert!(guesthouse_update_check::is_version(&version), "{version}");
    for entry in &manifest.files {
        // The name it is published under, and the hash file beside it: exactly the names the file
        // site takes (notes in the Worker's rule), and the same bytes the manifest says.
        assert_eq!(entry.name, format!("guesthouse-windows-driver-{version}-{}", entry.package_name));
        let sum = fetch::sha256_of(&published.join(&entry.name)).unwrap();
        assert_eq!(sum, entry.sha256, "{}", entry.name);
        let beside = fs::read_to_string(published.join(format!("{}.sha256", entry.name))).unwrap();
        assert_eq!(beside, format!("{sum}  {}\n", entry.name));
    }
    // The app's own fetch, with the folder standing for the file site.
    let downloads = dir.join("downloads");
    let folder = fetch_package(&mut Published(published.clone()), &downloads).unwrap();
    assert_eq!(package_files(&folder).unwrap().len(), REQUIRED.len());
    assert_eq!(kept_package(&downloads), Some(folder));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn the_linux_manifest_the_package_scripts_write_is_one_the_app_takes() {
    // As guest/drivers/linux-pvg/package/lib.sh writes it (pvg_write_manifest), for version 0.1.7: each
    // file published as guesthouse-linux-driver-V-FILE, with FILE its name in the package.
    let sum = "38d05ebe74bb28e8402705f407ce20f16f1c611c0716062f52d911dd089cbf1d";
    let files = [
        "install.sh",
        "pvg-dkms_0.1.7_all.deb",
        "pvg-mesa_0.1.7_arm64.deb",
        "guesthouse-desktop_0.1.7_all.deb",
        "guesthouse-gnome_0.1.7_all.deb",
        "guesthouse-plasma_0.1.7_all.deb",
    ];
    let lines: Vec<String> = files
        .iter()
        .map(|f| format!("  {{\"name\": \"guesthouse-linux-driver-0.1.7-{f}\", \"as\": \"{f}\", \"sha256\": \"{sum}\"}}"))
        .collect();
    let text = format!("{{\"version\": \"0.1.7\", \"files\": [\n{}\n]}}\n", lines.join(",\n"));
    let manifest = Manifest::parse_for(&LINUX, &text).unwrap();
    assert_eq!(manifest.version, "0.1.7");
    assert_eq!(manifest.files.len(), files.len());
    assert_eq!(manifest.files[0].package_name, LINUX_SCRIPT);
    assert_eq!(manifest.files[2].name, "guesthouse-linux-driver-0.1.7-pvg-mesa_0.1.7_arm64.deb");
    assert_eq!(LINUX.manifest, "guesthouse-linux-driver-latest.json");
    assert_eq!(LINUX.base, "https://files.ruxn.com/p/guesthouse-linux-driver/");
}

#[test]
fn both_packages_are_fetched_from_a_site_that_gives_no_length() {
    use crate::install::fake::FakeSite;
    let downloads = scratch("nolength");
    let mut site = FakeSite { no_length: true };
    let windows = fetch_package_of(&WINDOWS, &mut site, &downloads).unwrap();
    assert_eq!(kept_package_of(&WINDOWS, &downloads), Some(windows));
    let linux = fetch_package_of(&LINUX, &mut site, &downloads).unwrap();
    assert_eq!(kept_package_of(&LINUX, &downloads), Some(linux.clone()));
    assert!(fs::read_to_string(linux.join(LINUX_SCRIPT)).unwrap().contains("pretend"));
    let _ = fs::remove_dir_all(downloads);
}

#[test]
fn a_manifest_may_give_each_files_size_and_the_first_ones_did_not() {
    let without = manifest_with(&REQUIRED.iter().map(|n| (*n, SUM)).collect::<Vec<_>>());
    let parsed = Manifest::parse(&without).unwrap();
    assert!(parsed.files.iter().all(|e| e.size.is_none()));
    let with = without.replace(r#""sha256":"#, r#""size":123,"sha256":"#);
    let parsed = Manifest::parse(&with).unwrap();
    assert!(parsed.files.iter().all(|e| e.size == Some(123)));
}
