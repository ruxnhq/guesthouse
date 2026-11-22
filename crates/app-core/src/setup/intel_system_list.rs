//! The list of what Intel's INF puts in Windows' system folders, which setup writes beside the driver
//! it unpacks (see `intel_driver::unpack_intel`), and which the guest's scripts place files from at
//! install and at each start.
//!
//! A list written before the app gave System32 the 64-bit C++ runtime lacks it, and the folder
//! counts as unpacked all the same, so nothing asks for Intel's installer again. Every start then
//! found nothing missing, and Intel's 64-bit OpenCL loader, which imports the runtime, could not load
//! in a guest whose Windows has no Visual C++ runtime of its own. The 32-bit loader could, as the INF
//! itself puts the 32-bit runtime in SysWOW64. Such a list is completed from the 64-bit runtime the
//! folder holds, which the INF keeps there.

use super::intel::SYSTEM_LIST;
use std::fs;
use std::path::Path;

/// Where the 64-bit runtime goes, and the files of it a 64-bit loader can import. A folder has the
/// ones Intel ships; the others are passed over.
const RUNTIME_FOLDER: &str = "System32";
const RUNTIME_64: [&str; 3] = ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"];

/// The folder and the name a line of the list gives a file in Windows.
fn placed(line: &str) -> Option<(&str, &str)> {
    let mut fields = line.split('\t');
    let folder = fields.next()?;
    let name = fields.nth(1)?;
    Some((folder, name.trim_end()))
}

/// The list with the 64-bit runtime in it, for the runtime files `folder` holds. An empty list
/// stays empty, as the app says there is nothing to place.
fn completed(list: &str, folder: &Path) -> String {
    if list.trim().is_empty() {
        return list.to_owned();
    }
    let has = |wanted: &str| {
        list.lines().filter_map(placed).any(|(place, name)| {
            place.eq_ignore_ascii_case(RUNTIME_FOLDER) && name.eq_ignore_ascii_case(wanted)
        })
    };
    let mut list = list.to_owned();
    for name in RUNTIME_64.into_iter().filter(|name| !has(name) && folder.join(name).is_file()) {
        if !list.is_empty() && !list.ends_with('\n') {
            list.push('\n');
        }
        list.push_str(&format!("{RUNTIME_FOLDER}\t{name}\t{name}\n"));
    }
    list
}

/// The list in `folder`, completed, or `None` if there is none. One that was completed is written
/// back, so the guest's install script finds it whole too.
pub fn read_system_list(folder: &Path) -> Option<String> {
    let path = folder.join(SYSTEM_LIST);
    let list = fs::read_to_string(&path).ok()?;
    let whole = completed(&list, folder);
    if whole != list {
        // A list that can't be written back is still placed from at this start.
        let _ = fs::write(&path, &whole);
    }
    Some(whole)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    /// A list as an app from before the 64-bit runtime wrote it.
    const OLD_LIST: &str = "System32\tvulkan-1-64.dll\tvulkan-1.dll\n\
                            System32\tIntel_OpenCL_ICD64.dll\tOpenCL.dll\n\
                            SysWOW64\tmsvcp140_32.dll\tmsvcp140.dll\n\
                            SysWOW64\tvcruntime140_32.dll\tvcruntime140.dll\n";

    fn folder_with(scratch: &Scratch, list: &str, files: &[&str]) -> std::path::PathBuf {
        scratch.write(&format!("Graphics/{SYSTEM_LIST}"), list);
        for name in files {
            scratch.write(&format!("Graphics/{name}"), "runtime");
        }
        scratch.path("Graphics")
    }

    #[test]
    fn an_old_list_gets_the_64_bit_runtime_the_folder_holds_and_is_written_back() {
        let scratch = Scratch::new();
        let folder = folder_with(&scratch, OLD_LIST, &["msvcp140.dll", "vcruntime140.dll"]);
        let list = read_system_list(&folder).unwrap();
        let wanted = format!(
            "{OLD_LIST}System32\tmsvcp140.dll\tmsvcp140.dll\nSystem32\tvcruntime140.dll\tvcruntime140.dll\n"
        );
        assert_eq!(list, wanted, "the 32-bit runtime in SysWOW64 is not the 64-bit one");
        assert_eq!(fs::read_to_string(folder.join(SYSTEM_LIST)).unwrap(), wanted);
    }

    #[test]
    fn a_list_that_has_the_runtime_is_left_as_it_is() {
        let scratch = Scratch::new();
        let list = format!(
            "{OLD_LIST}System32\tMSVCP140.dll\tmsvcp140.dll\nSystem32\tvcruntime140.dll\tvcruntime140.dll"
        );
        let folder = folder_with(&scratch, &list, &["msvcp140.dll", "vcruntime140.dll"]);
        assert_eq!(read_system_list(&folder).unwrap(), list, "names in any case, and no last newline");
    }

    #[test]
    fn only_the_runtime_files_in_the_folder_are_added_and_a_list_without_a_last_newline_stays_lines() {
        let scratch = Scratch::new();
        let folder = folder_with(&scratch, OLD_LIST.trim_end(), &["vcruntime140.dll"]);
        let list = read_system_list(&folder).unwrap();
        assert!(list.ends_with("vcruntime140.dll\nSystem32\tvcruntime140.dll\tvcruntime140.dll\n"), "{list}");
        assert!(!list.contains("System32\tmsvcp140.dll"), "{list}");
        assert!(!list.contains("vcruntime140_1"), "{list}");
    }

    #[test]
    fn a_folder_with_no_list_has_none_and_an_empty_list_stays_empty() {
        let scratch = Scratch::new();
        scratch.write("Graphics/msvcp140.dll", "runtime");
        assert_eq!(read_system_list(&scratch.path("Graphics")), None);
        let folder = folder_with(&scratch, "", &[]);
        assert_eq!(read_system_list(&folder).as_deref(), Some(""));
    }
}
