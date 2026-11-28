// SPDX-License-Identifier: GPL-2.0-or-later
//! The machine an install makes: its folder and `machine.json`, written before anything is
//! downloaded so the library shows it from the start. The disk and the firmware are the make-disk
//! step's.

use super::plan::Guest;
use crate::create;
use crate::store::MACHINE_FILE;
use std::fs;
use std::path::Path;

/// What a new machine is given unless the person chose otherwise: the design's own figures for
/// Windows, and what Ubuntu needs for a desktop.
pub fn default_sizes(guest: Guest, host_cores: u32) -> (u32, u64, u64) {
    let cores = host_cores.clamp(1, 4);
    match guest {
        Guest::Windows => (cores, 6144, 64),
        Guest::Ubuntu => (cores, 4096, 32),
    }
}

/// Makes the machine's folder and file, and returns its id. The name must be free and not empty.
pub fn create_machine(
    machines: &Path,
    name: &str,
    guest: Guest,
    cores: u32,
    memory_mb: u64,
) -> Result<String, String> {
    if let Some(problem) = create::name_problem(machines, None, name) {
        return Err(problem);
    }
    let id = create::pick_id(machines, name)?;
    let folder = machines.join(&id);
    fs::create_dir_all(&folder).map_err(|e| format!("Cannot make {}: {e}", folder.display()))?;
    let settings = serde_json::json!({
        "name": name.trim(),
        "kind": guest.machine_kind(),
        "cores": cores,
        "memory_mb": memory_mb,
        "disk": "disk.img",
        "firmware_code": "code.fd",
        "firmware_vars": "vars.fd",
        "graphics": "guarded",
    });
    fs::write(
        folder.join(MACHINE_FILE),
        serde_json::to_string_pretty(&settings).expect("settings are JSON") + "\n",
    )
    .map_err(|e| format!("Cannot write {MACHINE_FILE}: {e}"))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;

    #[test]
    fn a_machine_is_made_with_the_files_it_will_have_and_its_name_is_kept_unique() {
        let root = std::env::temp_dir().join(format!("gh-install-machine-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let machines = root.join("Machines");
        fs::create_dir_all(&machines).unwrap();
        let id = create_machine(&machines, "Windows 11", Guest::Windows, 4, 6144).unwrap();
        assert_eq!(id, "windows-11");
        let listing = store::list(&machines).unwrap();
        let machine = &listing.machines[0];
        assert_eq!(
            (machine.name.as_str(), machine.kind.as_str(), machine.graphics.as_str()),
            ("Windows 11", "windows", "guarded")
        );
        assert!(machine.startable);
        assert!(
            create_machine(&machines, "Windows 11", Guest::Windows, 4, 6144)
                .unwrap_err()
                .contains("already exists")
        );
        assert!(create_machine(&machines, " ", Guest::Ubuntu, 4, 4096).is_err());
        assert_eq!(
            store::find(&machines, &create_machine(&machines, "Ubuntu", Guest::Ubuntu, 2, 4096).unwrap())
                .unwrap()
                .kind,
            "linux"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_sizes_follow_the_design_and_the_macs_processors() {
        assert_eq!(default_sizes(Guest::Windows, 10), (4, 6144, 64));
        assert_eq!(default_sizes(Guest::Ubuntu, 2), (2, 4096, 32));
    }
}
