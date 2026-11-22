//! What AMD's INF puts in Windows' system folders for the media runtime, so a program that loads AMF
//! by name finds it. A guest has only the driver's folder in its host driver store.
//!
//! A normal install copies `amfrt64.dll` to System32 and `amfrt32.dll` to SysWOW64, and the INF says
//! so twice: in the sections it copies from, and in the `CopyToVmOverwrite` values it keeps for a
//! virtual machine that is lent the driver. The second names the files a guest needs, and nothing
//! else of what the INF copies, so that is what this reads. Names come from the user's own driver,
//! and a later driver that renames or adds a file is followed without a change here.

use super::amd_package::display_inf;
use super::guest_files::{GuestFile, file_list};
use super::inf::latin1;
use std::fs;
use std::path::Path;

/// The INF's value name for the media runtime, in the key that lists what a guest is given.
const RUNTIME_VALUE: &str = "amdadvancedmediaframework";
/// The keys by the folder the file goes to: 64-bit programs find System32, and 32-bit ones SysWOW64.
const KEYS: [(&str, &str); 2] = [("copytovmoverwrite", "System32"), ("copytovmoverwritewow64", "SysWOW64")];

/// The media runtime's files the INF in `text` has a guest given, as the folder, the file in the
/// driver's folder and the name it gets there.
pub fn runtime_files(text: &str) -> Vec<GuestFile> {
    let mut found = Vec::new();
    for line in text.lines().map(str::trim) {
        let fields: Vec<&str> = line.split(',').map(|field| field.trim().trim_matches('"')).collect();
        let [register, key, value, _kind, from, to] = fields[..] else { continue };
        let folder = KEYS.iter().find(|(wanted, _)| key.eq_ignore_ascii_case(wanted));
        if !register.eq_ignore_ascii_case("hkr") || !value.eq_ignore_ascii_case(RUNTIME_VALUE) {
            continue;
        }
        let (Some((_, folder)), Some(source)) = (folder, from.rsplit('\\').next()) else { continue };
        found.push(GuestFile {
            folder: (*folder).to_owned(),
            source: source.to_owned(),
            name: to.to_owned(),
        });
    }
    found
}

/// The list the guest's script reads for the driver whose user-mode folder is `user_driver`, in the
/// form of [`file_list`], or why there is none. A file the driver's folder lacks is left out, as the
/// guest would be told to copy what isn't there.
pub fn runtime_file_list(user_driver: &Path) -> Result<String, String> {
    let inf = display_inf(user_driver).ok_or("AMD's driver was unpacked without its INF")?;
    let text =
        fs::read(&inf).map(|data| latin1(&data)).map_err(|why| format!("its INF can't be read: {why}"))?;
    let files: Vec<GuestFile> =
        runtime_files(&text).into_iter().filter(|file| user_driver.join(&file.source).is_file()).collect();
    if files.is_empty() {
        return Err("AMD's driver has no media runtime to give the guest".to_owned());
    }
    Ok(file_list(&files))
}

#[cfg(test)]
pub mod stand_in {
    use crate::setup::amd_guest_folder::stand_in::lay_out_driver;
    use crate::setup::amd_package::stand_in::{INF_NAME, inf, lay_out_package};
    use crate::testing::Scratch;
    use std::path::PathBuf;

    /// The lines of AMD's INF about the media runtime, with the OpenCL loader's beside them as it
    /// has them.
    pub const RUNTIME_LINES: &str = "\
HKR,CopyToVmOverwrite,AMDOpenCLLoader,%REG_MULTI_SZ%,\"B026716\\amd_opencl64.dll\",\"opencl.dll\"
HKR,CopyToVmOverwrite,AMDAdvancedMediaFramework,%REG_MULTI_SZ%,\"B026716\\amfrt64.dll\",\"amfrt64.dll\"
HKR,CopyToVmOverwriteWow64,AMDAdvancedMediaFramework,%REG_MULTI_SZ%,\"B026716\\amfrt32.dll\", \"amfrt32.dll\"
HKR,UMD,CFM_DEF,%REG_SZ%,1
";

    /// A driver as unpacked, whose INF has the media runtime lines and whose folder has the runtime
    /// files `files`. Returns the user-mode driver's folder.
    pub fn driver_with_runtime(scratch: &Scratch, files: &[&str]) -> PathBuf {
        lay_out_driver(scratch, "Display/WT6A_INF", "B026716", false);
        lay_out_package(scratch, "Display/WT6A_INF", "B026716");
        let text = format!("{}{RUNTIME_LINES}", inf("B026716", &[0x747E]));
        scratch.write(&format!("Display/WT6A_INF/{INF_NAME}"), text);
        for name in files {
            scratch.write(&format!("Display/WT6A_INF/B026716/{name}"), name);
        }
        scratch.path("Display/WT6A_INF/B026716")
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::{RUNTIME_LINES, driver_with_runtime};
    use super::*;
    use crate::setup::amd_guest_folder::stand_in::lay_out_driver;
    use crate::testing::Scratch;

    fn place(file: &GuestFile) -> (&str, &str, &str) {
        (&file.folder, &file.source, &file.name)
    }

    #[test]
    fn the_runtime_is_what_the_inf_gives_a_guest_for_each_folder_and_not_the_opencl_loader() {
        let found = runtime_files(RUNTIME_LINES);
        let found: Vec<_> = found.iter().map(place).collect();
        assert_eq!(
            found,
            [("System32", "amfrt64.dll", "amfrt64.dll"), ("SysWOW64", "amfrt32.dll", "amfrt32.dll")]
        );
    }

    #[test]
    fn a_renamed_runtime_is_followed_and_the_case_and_a_space_before_a_name_are_no_matter() {
        let text = "hkr,copytovmoverwrite,amdadvancedmediaframework,%REG_MULTI_SZ%,\"B1\\amfrt64_v2.dll\", \"amfrt64.dll\"";
        let found = runtime_files(text);
        assert_eq!(
            found.iter().map(place).collect::<Vec<_>>(),
            [("System32", "amfrt64_v2.dll", "amfrt64.dll")]
        );
    }

    #[test]
    fn an_inf_that_says_nothing_of_the_runtime_gives_none() {
        assert!(runtime_files("[Version]\nHKR,UMD,CFM_DEF,%REG_SZ%,1\n").is_empty());
    }

    #[test]
    fn the_list_has_each_file_the_driver_folder_has_as_the_guests_script_reads_it() {
        let scratch = Scratch::new();
        let folder = driver_with_runtime(&scratch, &["amfrt64.dll", "amfrt32.dll"]);
        let list = runtime_file_list(&folder).unwrap();
        assert_eq!(list, "System32\tamfrt64.dll\tamfrt64.dll\nSysWOW64\tamfrt32.dll\tamfrt32.dll\n");
    }

    #[test]
    fn a_file_the_driver_folder_lacks_is_left_out_and_none_at_all_is_said() {
        let scratch = Scratch::new();
        let folder = driver_with_runtime(&scratch, &["amfrt64.dll"]);
        assert_eq!(runtime_file_list(&folder).unwrap(), "System32\tamfrt64.dll\tamfrt64.dll\n");
        fs::remove_file(folder.join("amfrt64.dll")).unwrap();
        assert!(runtime_file_list(&folder).unwrap_err().contains("no media runtime"));
    }

    #[test]
    fn a_driver_unpacked_without_its_inf_is_said() {
        let scratch = Scratch::new();
        lay_out_driver(&scratch, "Display/WT6A_INF", "B026716", false);
        let folder = scratch.path("Display/WT6A_INF/B026716");
        assert!(runtime_file_list(&folder).unwrap_err().contains("without its INF"));
    }
}
