//! Where this PC keeps the programs and libraries a jailed process loads.
//!
//! Most distributions keep all of them under `/usr`. NixOS keeps them in its store and links to them
//! from elsewhere: the system's programs from `/run/current-system/sw`, and the graphics drivers, which
//! have to match the running kernel, from `/run/opengl-driver`. A jail grants every one of these, and
//! Landlock and bubblewrap both pass over the ones a PC doesn't have, so one plan fits both kinds.

use crate::{Grant, Plan};

/// Read, and run programs and load libraries from them.
pub const PROGRAM_FOLDERS: [&str; 4] = ["/usr", "/nix/store", "/run/current-system/sw", "/run/opengl-driver"];

/// NixOS's `/etc` is mostly links into this, which leads into the store in turn. In namespaces a file
/// of `/etc` is bound as what it links to, but a folder such as `/etc/qemu` keeps its links, and they
/// need this to lead anywhere.
pub const LINKED_SETTINGS: &str = "/etc/static";

/// Grants the system's programs and libraries.
pub fn grant_programs(plan: &mut Plan) {
    for folder in PROGRAM_FOLDERS {
        plan.grant(Grant::Run, folder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn every_folder_may_be_run_from() {
        let mut plan = Plan::default();
        grant_programs(&mut plan);
        for folder in PROGRAM_FOLDERS {
            assert!(plan.paths.contains(&(Grant::Run, Path::new(folder).to_owned())), "{folder}");
        }
    }
}
