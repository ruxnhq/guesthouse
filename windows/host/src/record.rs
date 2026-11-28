//! What `prepare` knows of a machine that `boot` needs too, kept in a small file beside its disk:
//! which system it runs, and for a Linux guest, its installer and whether it has installed.

use crate::machine::Guest;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub guest: Guest,
    /// The ISO a Linux guest installs from, attached until it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installer: Option<PathBuf>,
    #[serde(default)]
    pub installed: bool,
    /// The machine's network card's address, the same at every boot, so the guest's network
    /// settings and its DHCP lease keep to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    /// The fingerprint of the remoteman host in a Windows guest, as the guest last said it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

impl Record {
    pub fn path(disk: &Path) -> PathBuf {
        disk.with_extension("guesthouse.json")
    }

    /// The disk's record. A disk with none is taken for a Windows guest, as every disk was before
    /// there were records.
    pub fn load(disk: &Path) -> Result<Self, String> {
        let path = Record::path(disk);
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display())),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Record::default()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }

    pub fn save(&self, disk: &Path) -> Result<(), String> {
        let path = Record::path(disk);
        let text = serde_json::to_string_pretty(self).expect("a record always serialises");
        // Written whole beside the record and then put in its place, so a reader finds the old one or
        // the new one, and a record is never half written.
        let mut new = path.clone().into_os_string();
        new.push(".new");
        let new = PathBuf::from(new);
        let put = |error: std::io::Error| format!("{}: {error}", path.display());
        fs::write(&new, text + "\n").map_err(put)?;
        fs::rename(&new, &path).map_err(|error| {
            let _ = fs::remove_file(&new);
            put(error)
        })
    }

    /// Keeps `fingerprint` as the guest's screen, once in a boot and only if the record doesn't
    /// have it. `kept` says whether this boot has made its one change already. True when the record
    /// was written.
    pub fn keep_screen(disk: &Path, fingerprint: String, kept: &mut bool) -> Result<bool, String> {
        if std::mem::replace(kept, true) {
            return Ok(false);
        }
        let record = Record::load(disk)?;
        if record.screen.as_deref() == Some(fingerprint.as_str()) {
            return Ok(false);
        }
        Record { screen: Some(fingerprint), ..record }.save(disk)?;
        Ok(true)
    }
}

/// A Linux guest's files disk, beside its own: its installer's boot files and answer file, and the
/// host's GPU files the guest mounts at /usr/lib/wsl.
pub fn files_disk(disk: &Path) -> PathBuf {
    disk.with_extension("files.vhdx")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_reads_back_as_it_was_written() {
        let folder = std::env::temp_dir().join(format!("gh-record-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let disk = folder.join("ubuntu.vhdx");
        assert_eq!(Record::load(&disk).unwrap(), Record::default(), "no record is a Windows guest");
        let record = Record {
            guest: Guest::Linux,
            installer: Some(folder.join("u.iso")),
            installed: false,
            mac: Some("00-15-5D-00-00-01".into()),
            screen: Some("ab".repeat(32)),
        };
        record.save(&disk).unwrap();
        assert_eq!(Record::load(&disk).unwrap(), record);
        assert!(fs::read_to_string(Record::path(&disk)).unwrap().contains("\"guest\": \"linux\""));
        let _ = fs::remove_dir_all(folder);
    }

    #[cfg(unix)]
    #[test]
    fn a_save_puts_a_whole_file_in_place_of_the_old_one() {
        use std::os::unix::fs::MetadataExt;
        let folder = std::env::temp_dir().join(format!("gh-record-whole-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let disk = folder.join("w.vhdx");
        Record::default().save(&disk).unwrap();
        let first = fs::metadata(Record::path(&disk)).unwrap().ino();
        Record { installed: true, ..Record::default() }.save(&disk).unwrap();
        assert_ne!(
            fs::metadata(Record::path(&disk)).unwrap().ino(),
            first,
            "the old file is never written over"
        );
        assert!(Record::load(&disk).unwrap().installed);
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1, "no half-made file is left");
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_boot_keeps_the_screens_fingerprint_once_and_only_when_it_is_new() {
        let folder = std::env::temp_dir().join(format!("gh-record-screen-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let disk = folder.join("s.vhdx");
        Record { installed: true, ..Record::default() }.save(&disk).unwrap();
        let (one, two) = ("ab".repeat(32), "cd".repeat(32));
        let mut kept = false;
        assert_eq!(Record::keep_screen(&disk, one.clone(), &mut kept), Ok(true));
        assert_eq!(Record::keep_screen(&disk, two.clone(), &mut kept), Ok(false), "once in a boot");
        assert_eq!(Record::load(&disk).unwrap().screen, Some(one.clone()));
        assert!(Record::load(&disk).unwrap().installed, "what else the record holds stays");
        let mut next_boot = false;
        assert_eq!(Record::keep_screen(&disk, one, &mut next_boot), Ok(false), "the same one needs no write");
        assert_eq!(Record::keep_screen(&disk, two.clone(), &mut false), Ok(true));
        assert_eq!(Record::load(&disk).unwrap().screen, Some(two));
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn the_files_disk_sits_beside_the_guests_own() {
        assert_eq!(files_disk(Path::new(r"D:\vms\ubuntu.vhdx")), Path::new(r"D:\vms\ubuntu.files.vhdx"));
    }
}
