//! AMD's display driver as a package Windows can be told about.
//!
//! The guest loads AMD's user-mode driver from the host driver store, where no package owns it.
//! Anti-cheat that asks Windows which package a module belongs to, as BattlEye does, blocks it there.
//! Once AMD's own INF and catalog are in Windows' driver store, the same files count as AMD's driver.
//! The INF and catalog sit beside the user-mode driver's folder in the unpacked driver, and the INF
//! names its files in that folder.

use super::amd_driver::lists_device;
use super::inf::latin1;
use crate::gpu_identity::AMD_RENDER;
use std::fs;
use std::path::{Path, PathBuf};

const INF: &str = "inf";
const CATALOG: &str = "cat";

/// What Windows needs to take the driver as a package, besides the files the guest has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmdPackageFiles {
    pub inf: PathBuf,
    pub catalog: PathBuf,
    /// The folder the INF names its files in, beside itself.
    pub files_folder: String,
}

/// A file's name, as text.
pub fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

fn has_extension(path: &Path, wanted: &str) -> bool {
    path.is_file() && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case(wanted))
}

/// The display driver's INF and catalog for the user-mode driver's folder `user_driver`, or why
/// Windows can't be given them.
pub fn package_files(user_driver: &Path) -> Result<AmdPackageFiles, String> {
    let files_folder = file_name(user_driver);
    let beside = user_driver.parent().and_then(|driver| fs::read_dir(driver).ok());
    let inf = beside.and_then(|entries| {
        entries.flatten().map(|entry| entry.path()).find(|path| has_extension(path, INF))
    });
    let Some(inf) = inf else {
        return Err("AMD's driver was unpacked without its INF".to_owned());
    };
    let catalog = inf.with_extension(CATALOG);
    if !catalog.is_file() {
        return Err(format!("AMD's driver was unpacked without {}", file_name(&catalog)));
    }
    let text = fs::read(&inf).map(|data| latin1(&data)).unwrap_or_default();
    if !text.contains(&files_folder) {
        return Err(format!("{} doesn't name its files in {files_folder}", file_name(&inf)));
    }
    // A package that lists the device Windows is shown would be put on it, in place of Windows' own
    // render driver, and AMD's kernel driver can't run in a guest.
    if lists_device(&text, AMD_RENDER.device.into()) {
        return Err(format!("{} lists the device this machine shows Windows", file_name(&inf)));
    }
    Ok(AmdPackageFiles { inf, catalog, files_folder })
}

#[cfg(test)]
pub mod stand_in {
    use crate::testing::Scratch;

    pub const INF_NAME: &str = "u0205499.inf";
    pub const CATALOG_NAME: &str = "u0205499.cat";

    /// An INF as AMD's display driver has it, for the files in `folder` and the cards `devices`.
    pub fn inf(folder: &str, devices: &[u16]) -> String {
        let mut text = format!("[Version]\nCatalogFile={CATALOG_NAME}\n[ATI.Mfg.NTamd64]\n");
        for device in devices {
            text += &format!("\"%AMD{device:04X}.1%\" = ati2mtag_X, PCI\\VEN_1002&DEV_{device:04X}&REV_C1\n");
        }
        text + &format!("[SourceDisksNames.amd64]\n1 = %DiskId%,,,.\\{folder}\n")
    }

    /// Writes the INF and catalog in the display driver's folder `driver` of `scratch`.
    pub fn lay_out_package(scratch: &Scratch, driver: &str, folder: &str) {
        scratch.write(&format!("{driver}/{INF_NAME}"), inf(folder, &[0x747E]));
        scratch.write(&format!("{driver}/{CATALOG_NAME}"), "the catalog");
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::{CATALOG_NAME, INF_NAME, inf, lay_out_package};
    use super::*;
    use crate::setup::amd_guest_folder::add_components;
    use crate::setup::amd_guest_folder::stand_in::lay_out_driver;
    use crate::testing::Scratch;

    const DRIVER: &str = "Display/WT6A_INF";
    const FOLDER: &str = "B026716";

    fn laid_out(scratch: &Scratch) -> PathBuf {
        lay_out_driver(scratch, DRIVER, FOLDER, false);
        lay_out_package(scratch, DRIVER, FOLDER);
        scratch.path(&format!("{DRIVER}/{FOLDER}"))
    }

    #[test]
    fn the_package_is_the_inf_and_catalog_beside_the_user_mode_drivers_folder() {
        let scratch = Scratch::new();
        let folder = laid_out(&scratch);
        // The components' own INFs are in folders of their own, and aren't the display driver's.
        add_components(&folder).unwrap();
        let found = package_files(&folder).unwrap();
        assert_eq!(file_name(&found.inf), INF_NAME);
        assert_eq!(file_name(&found.catalog), CATALOG_NAME);
        assert_eq!(found.files_folder, FOLDER);
    }

    #[test]
    fn a_driver_without_its_inf_or_catalog_has_no_package() {
        let scratch = Scratch::new();
        let folder = laid_out(&scratch);
        fs::remove_file(scratch.path(&format!("{DRIVER}/{CATALOG_NAME}"))).unwrap();
        assert!(package_files(&folder).unwrap_err().contains(CATALOG_NAME));
        fs::remove_file(scratch.path(&format!("{DRIVER}/{INF_NAME}"))).unwrap();
        assert!(package_files(&folder).unwrap_err().contains("without its INF"));
    }

    #[test]
    fn an_inf_for_files_somewhere_else_is_not_this_folders() {
        let scratch = Scratch::new();
        let folder = laid_out(&scratch);
        scratch.write(&format!("{DRIVER}/{INF_NAME}"), inf("B000001", &[0x747E]));
        assert!(package_files(&folder).unwrap_err().contains(FOLDER));
    }

    #[test]
    fn a_package_that_would_bind_to_the_device_windows_is_shown_is_refused() {
        let scratch = Scratch::new();
        let folder = laid_out(&scratch);
        scratch.write(&format!("{DRIVER}/{INF_NAME}"), inf(FOLDER, &[0x747E, AMD_RENDER.device]));
        assert!(package_files(&folder).unwrap_err().contains("lists the device"));
    }
}
