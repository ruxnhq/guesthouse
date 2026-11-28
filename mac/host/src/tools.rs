// SPDX-License-Identifier: GPL-2.0-or-later
//! Finding the programs the app runs, so a person never has to type a path: QEMU, which runs
//! the machines, and rmhost, which shares a machine's screen with other devices.
//!
//! The places, in the order they are tried:
//!
//! 1. the path chosen in Settings, when there is one and it is there (QEMU only);
//! 2. an environment variable, for trying the app with a stand-in (`GUESTHOUSE_QEMU`,
//!    `GUESTHOUSE_RMHOST`);
//! 3. the copy inside the app bundle, which is how a release ships it;
//! 4. the lab's own builds, newest first, only as a convenience while the app is developed on the
//!    lab's Mac (one marked list further down, to be deleted for the public repo).
//!
//! Every answer says in a sentence why that one was chosen, because Settings shows it.

use std::path::{Path, PathBuf};

/// A program the app runs that is not part of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Qemu,
    Rmhost,
    /// The graphics process of the guarded path.
    PvgHost,
    /// A QEMU that has the `pvg-fwd` device, for the guarded path.
    PvgQemu,
}

impl Tool {
    /// What the program is called by a person, with a clause saying what it does.
    pub fn described(self) -> &'static str {
        match self {
            Tool::Qemu => "QEMU, the program that runs the machines",
            Tool::Rmhost => "rmhost, the program that shares a machine's screen with other devices",
            Tool::PvgHost => "guesthouse-pvg-host, the protected graphics process",
            Tool::PvgQemu => "a QEMU with the protected graphics device (pvg-fwd)",
        }
    }

    /// The program's file name, the same wherever it sits.
    pub fn file_name(self) -> &'static str {
        match self {
            Tool::Qemu | Tool::PvgQemu => "qemu-system-aarch64",
            Tool::Rmhost => "rmhost",
            Tool::PvgHost => "guesthouse-pvg-host",
        }
    }

    fn environment(self) -> &'static str {
        match self {
            Tool::Qemu => "GUESTHOUSE_QEMU",
            Tool::Rmhost => "GUESTHOUSE_RMHOST",
            Tool::PvgHost => "GUESTHOUSE_PVG_HOST",
            Tool::PvgQemu => "GUESTHOUSE_PVG_QEMU",
        }
    }
}

/// Which of the places a QEMU was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Setting,
    Environment,
    Bundled,
    /// The lab's build: development only.
    Lab,
}

impl Source {
    /// The word the app shows and tests against.
    pub fn name(self) -> &'static str {
        match self {
            Source::Setting => "setting",
            Source::Environment => "environment",
            Source::Bundled => "bundled",
            Source::Lab => "lab",
        }
    }
}

/// The QEMU that will run, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub source: Source,
    /// One sentence for Settings.
    pub why: String,
}

/// The places to look in, passed in so tests can point them at a scratch folder.
#[derive(Debug, Default, Clone)]
pub struct Places {
    /// What Settings holds; empty or absent when the person chose nothing.
    pub setting: Option<String>,
    pub environment: Option<PathBuf>,
    /// The QEMU inside the app bundle, where one would be.
    pub bundled: Option<PathBuf>,
    /// The lab's builds, newest first; empty off the lab's Mac.
    pub lab: Vec<PathBuf>,
}

impl Places {
    /// The environment and the lab's build as they are on this Mac; the caller adds what
    /// Settings holds and where the bundle keeps its QEMU.
    pub fn on_this_mac(tool: Tool, setting: Option<String>, bundled: Option<PathBuf>) -> Places {
        Places {
            setting,
            environment: std::env::var_os(tool.environment())
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            bundled,
            lab: std::env::var_os("HOME")
                .map(|home| crate::lab::builds(tool, Path::new(&home)))
                .unwrap_or_default(),
        }
    }
}

/// The program to run, or a sentence saying what is missing.
pub fn locate(tool: Tool, places: &Places) -> Result<Found, String> {
    // A path from Settings that is not there is not an error to stop on: the others may serve.
    // It is told to the person, so they know why their choice was passed over.
    let mut passed_over = None;
    if let Some(setting) = places.setting.as_deref().filter(|path| !path.is_empty()) {
        if Path::new(setting).is_file() {
            return Ok(Found {
                path: PathBuf::from(setting),
                source: Source::Setting,
                why: "You chose it in Settings.".into(),
            });
        }
        passed_over = Some(format!("The chosen {} is not there ({setting}). ", tool.file_name()));
    }
    let prefix = passed_over.unwrap_or_default();

    let environment = format!("Set by {} in the environment.", tool.environment());
    let candidates = [
        (&places.environment, Source::Environment, environment.as_str()),
        (&places.bundled, Source::Bundled, "The copy that comes inside Guesthouse."),
    ];
    let lab = places.lab.iter().map(|path| {
        (Some(path), Source::Lab, "The lab's own build, used because it is there. Only the lab's Mac has it.")
    });
    let candidates =
        candidates.into_iter().map(|(path, source, why)| (path.as_ref(), source, why)).chain(lab);
    for (path, source, why) in candidates {
        if let Some(path) = path.filter(|path| path.is_file()) {
            return Ok(Found { path: path.clone(), source, why: format!("{prefix}{why}") });
        }
    }
    let choose = if matches!(tool, Tool::Qemu | Tool::PvgQemu) { " Choose it in Settings." } else { "" };
    Err(format!("{prefix}Guesthouse cannot find {}.{choose}", tool.described()))
}

/// `locate`, and then a look at what the program found can do: a graphics process that cannot
/// publish frames, or a QEMU with no `pvg-fwd`, is refused with a sentence that says so, since
/// starting with it would give a machine with no screen or a message nobody can act on.
pub fn locate_checked(tool: Tool, places: &Places) -> Result<Found, String> {
    let found = locate(tool, places)?;
    check(tool, &found.path)?;
    Ok(found)
}

/// What a program found for `tool` has to be able to do, or the sentence saying it cannot.
pub fn check(tool: Tool, program: &Path) -> Result<(), String> {
    let path = program.display();
    match tool {
        Tool::PvgHost if !crate::probe::pvg_host_publishes_frames(program) => Err(format!(
            "The graphics process at {path} is an older build that cannot publish the screen (it has no --frames). Build the newer one."
        )),
        Tool::PvgQemu if !crate::probe::qemu_has_device(program, "pvg-fwd") => Err(format!(
            "The QEMU at {path} has no protected graphics device (pvg-fwd). Choose one that has it in Settings, or use QEMU's own device in the machine's Hardware tab."
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A folder with some executables in it, removed afterwards.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path = std::env::temp_dir().join(format!("guesthouse-qemu-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }

        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, "").unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn nothing_found_says_what_is_missing() {
        let error = locate(Tool::Qemu, &Places::default()).unwrap_err();
        assert!(error.contains("cannot find QEMU"));
        assert!(error.contains("Settings"));
    }

    #[test]
    fn settings_come_first() {
        let dir = Scratch::new("setting");
        let chosen = dir.file("chosen");
        let places = Places {
            setting: Some(chosen.display().to_string()),
            bundled: Some(dir.file("bundled")),
            lab: vec![dir.file("lab")],
            ..Places::default()
        };
        let found = locate(Tool::Qemu, &places).unwrap();
        assert_eq!(found.source, Source::Setting);
        assert_eq!(found.path, chosen);
    }

    #[test]
    fn bundled_comes_before_the_lab() {
        let dir = Scratch::new("bundled");
        let places =
            Places { bundled: Some(dir.file("bundled")), lab: vec![dir.file("lab")], ..Places::default() };
        assert_eq!(locate(Tool::Qemu, &places).unwrap().source, Source::Bundled);
    }

    #[test]
    fn environment_comes_before_the_bundle() {
        let dir = Scratch::new("env");
        let places = Places {
            environment: Some(dir.file("fake")),
            bundled: Some(dir.file("bundled")),
            ..Places::default()
        };
        assert_eq!(locate(Tool::Qemu, &places).unwrap().source, Source::Environment);
    }

    #[test]
    fn the_lab_build_is_used_last_and_says_so() {
        let dir = Scratch::new("lab");
        let places = Places { lab: vec![dir.file("lab")], ..Places::default() };
        let found = locate(Tool::Qemu, &places).unwrap();
        assert_eq!(found.source, Source::Lab);
        assert!(found.why.contains("lab"));
    }

    #[test]
    fn the_newer_lab_graphics_process_comes_before_the_older_one() {
        let dir = Scratch::new("pvg");
        let (newer, older) = (dir.file("newer"), dir.file("older"));
        let mut places =
            Places { lab: vec![dir.0.join("absent"), newer.clone(), older.clone()], ..Places::default() };
        assert_eq!(locate(Tool::PvgHost, &places).unwrap().path, newer);
        places.lab.remove(1);
        assert_eq!(locate(Tool::PvgHost, &places).unwrap().path, older);
    }

    #[test]
    fn a_missing_setting_is_told_and_passed_over() {
        let dir = Scratch::new("missing");
        let places = Places {
            setting: Some("/no/such/qemu".into()),
            bundled: Some(dir.file("bundled")),
            ..Places::default()
        };
        let found = locate(Tool::Qemu, &places).unwrap();
        assert_eq!(found.source, Source::Bundled);
        assert!(found.why.starts_with("The chosen qemu-system-aarch64 is not there (/no/such/qemu)."));
    }

    #[test]
    fn a_missing_setting_and_nothing_else_is_an_error_that_names_the_setting() {
        let places = Places { setting: Some("/no/such/qemu".into()), ..Places::default() };
        let error = locate(Tool::Qemu, &places).unwrap_err();
        assert!(error.contains("/no/such/qemu"));
    }

    #[test]
    fn rmhost_is_found_like_qemu_and_its_absence_has_its_own_sentence() {
        let dir = Scratch::new("rmhost");
        let places = Places { bundled: Some(dir.file("rmhost")), ..Places::default() };
        assert_eq!(locate(Tool::Rmhost, &places).unwrap().source, Source::Bundled);
        let error = locate(Tool::Rmhost, &Places::default()).unwrap_err();
        assert!(error.contains("rmhost"));
        assert!(!error.contains("Settings"));
    }

    #[test]
    fn an_empty_setting_counts_as_none() {
        let dir = Scratch::new("empty");
        let places =
            Places { setting: Some(String::new()), bundled: Some(dir.file("bundled")), ..Places::default() };
        let found = locate(Tool::Qemu, &places).unwrap();
        assert_eq!(found.source, Source::Bundled);
        assert!(!found.why.contains("not there"));
    }
}
