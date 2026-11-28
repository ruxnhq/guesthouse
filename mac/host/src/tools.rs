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
//! A build signed with a Developer ID (`signed`, read from the code signature at run time) uses
//! the third place only: the copy inside its own bundle is run, and no setting, environment
//! variable or request names a program.
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
    /// Only the bundled copy is run: what Settings holds is told to the person and passed over.
    pub bundle_only: bool,
}

impl Places {
    /// The environment and the lab's build as they are on this Mac; the caller adds what
    /// Settings holds and where the bundle keeps its QEMU.
    ///
    /// A `signed` build has the bundled copy only: the environment and the lab's builds are not
    /// looked at, and a setting is passed over.
    pub fn on_this_mac(
        tool: Tool,
        setting: Option<String>,
        bundled: Option<PathBuf>,
        signed: bool,
    ) -> Places {
        if signed {
            return Places { setting, bundled, bundle_only: true, ..Places::default() };
        }
        Places {
            setting,
            environment: std::env::var_os(tool.environment())
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            bundled,
            lab: std::env::var_os("HOME")
                .map(|home| crate::lab::builds(tool, Path::new(&home)))
                .unwrap_or_default(),
            bundle_only: false,
        }
    }
}

/// The folder of programs in the app bundle that `exe`, a program in the bundle's `Contents/MacOS`,
/// belongs to: `Contents/Helpers`. None for a program that sits anywhere else.
pub fn helpers_beside(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents").then(|| contents.join("Helpers"))
}

/// The programs and folders the agent starts with, from what the environment names. A `signed`
/// agent names the copies in its own bundle (`exe` is its own program) and nothing else.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Programs {
    pub qemu: Option<PathBuf>,
    pub rmhost: Option<PathBuf>,
    pub pvg_host: Option<PathBuf>,
    pub driver_dir: Option<PathBuf>,
    pub firmware_dir: Option<PathBuf>,
    /// Install with a stand-in network and guest.
    pub fake_install: bool,
}

impl Programs {
    pub fn choose(
        signed: bool,
        exe: Option<&Path>,
        env: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Programs {
        if signed {
            let helpers = exe.and_then(helpers_beside);
            let bundled = |tool: Tool| helpers.as_ref().map(|folder| folder.join(tool.file_name()));
            return Programs {
                qemu: bundled(Tool::Qemu),
                rmhost: bundled(Tool::Rmhost),
                pvg_host: bundled(Tool::PvgHost),
                ..Programs::default()
            };
        }
        let path = |name: &str| env(name).filter(|value| !value.is_empty()).map(PathBuf::from);
        Programs {
            qemu: path(Tool::Qemu.environment()),
            rmhost: path(Tool::Rmhost.environment()),
            pvg_host: path(Tool::PvgHost.environment()),
            driver_dir: path("GUESTHOUSE_DRIVER_DIR"),
            firmware_dir: path("GUESTHOUSE_FIRMWARE_DIR"),
            fake_install: env("GUESTHOUSE_INSTALL_FAKE").is_some_and(|value| !value.is_empty()),
        }
    }
}

/// What a request names, kept only by a build that is not `signed`: a signed one runs the
/// programs of its own bundle whatever a request says.
pub fn requested<T>(signed: bool, named: Option<T>) -> Option<T> {
    if signed { None } else { named }
}

/// The program to run, or a sentence saying what is missing.
pub fn locate(tool: Tool, places: &Places) -> Result<Found, String> {
    // A path from Settings that is not there is not an error to stop on: the others may serve.
    // It is told to the person, so they know why their choice was passed over.
    let mut passed_over = None;
    if let Some(setting) = places.setting.as_deref().filter(|path| !path.is_empty()) {
        let name = tool.file_name();
        if places.bundle_only {
            passed_over = Some(format!(
                "This build runs the {name} that comes inside Guesthouse, not the one chosen in Settings. "
            ));
        } else if Path::new(setting).is_file() {
            return Ok(Found {
                path: PathBuf::from(setting),
                source: Source::Setting,
                why: "You chose it in Settings.".into(),
            });
        } else {
            passed_over = Some(format!("The chosen {name} is not there ({setting}). "));
        }
    }
    let prefix = passed_over.unwrap_or_default();

    let environment = format!("Set by {} in the environment.", tool.environment());
    let candidates = [
        (&places.environment, Source::Environment, environment.as_str()),
        (&places.bundled, Source::Bundled, "The copy that comes inside Guesthouse."),
    ];
    // A bundle-only build looks at the bundled copy and nothing else.
    let skip = usize::from(places.bundle_only);
    let lab = places.lab.iter().filter(|_| !places.bundle_only).map(|path| {
        (Some(path), Source::Lab, "The lab's own build, used because it is there. Only the lab's Mac has it.")
    });
    let candidates =
        candidates.into_iter().skip(skip).map(|(path, source, why)| (path.as_ref(), source, why)).chain(lab);
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

    #[test]
    fn a_signed_build_runs_the_bundled_copy_and_nothing_else() {
        let dir = Scratch::new("signed");
        let places = Places {
            setting: Some(dir.file("chosen").display().to_string()),
            environment: Some(dir.file("fake")),
            bundled: Some(dir.file("bundled")),
            lab: vec![dir.file("lab")],
            bundle_only: true,
        };
        let found = locate(Tool::Qemu, &places).unwrap();
        assert_eq!(found.source, Source::Bundled);
        assert_eq!(found.path, dir.0.join("bundled"));
        assert!(
            found.why.starts_with("This build runs the qemu-system-aarch64 that comes inside Guesthouse")
        );
        let without = Places { bundled: None, ..places };
        assert!(locate(Tool::Qemu, &without).is_err());
    }

    #[test]
    fn a_signed_build_looks_at_no_environment_variable() {
        let places = Places::on_this_mac(Tool::Qemu, None, Some(PathBuf::from("/b/qemu")), true);
        assert!(places.bundle_only && places.environment.is_none() && places.lab.is_empty());
        assert_eq!(places.bundled, Some(PathBuf::from("/b/qemu")));
    }

    fn environment(name: &str) -> Option<std::ffi::OsString> {
        let value = match name {
            "GUESTHOUSE_QEMU" => "/e/qemu",
            "GUESTHOUSE_RMHOST" => "/e/rmhost",
            "GUESTHOUSE_PVG_HOST" => "/e/pvg",
            "GUESTHOUSE_DRIVER_DIR" => "/e/driver",
            "GUESTHOUSE_FIRMWARE_DIR" => "/e/firmware",
            "GUESTHOUSE_INSTALL_FAKE" => "1",
            _ => return None,
        };
        Some(value.into())
    }

    #[test]
    fn a_signed_agent_starts_with_its_bundle_s_programs_only() {
        let exe = Path::new("/Applications/Guesthouse.app/Contents/MacOS/guesthouse-agent");
        let chosen = Programs::choose(true, Some(exe), environment);
        let helpers = Path::new("/Applications/Guesthouse.app/Contents/Helpers");
        assert_eq!(
            chosen,
            Programs {
                qemu: Some(helpers.join("qemu-system-aarch64")),
                rmhost: Some(helpers.join("rmhost")),
                pvg_host: Some(helpers.join("guesthouse-pvg-host")),
                ..Programs::default()
            }
        );
    }

    #[test]
    fn a_signed_agent_outside_a_bundle_has_no_programs() {
        assert_eq!(
            Programs::choose(true, Some(Path::new("/tmp/guesthouse-agent")), environment),
            Programs::default()
        );
        assert_eq!(Programs::choose(true, None, environment), Programs::default());
    }

    #[test]
    fn an_agent_that_is_not_signed_takes_the_environment() {
        let chosen = Programs::choose(false, None, environment);
        assert_eq!(chosen.qemu, Some(PathBuf::from("/e/qemu")));
        assert_eq!(chosen.rmhost, Some(PathBuf::from("/e/rmhost")));
        assert_eq!(chosen.pvg_host, Some(PathBuf::from("/e/pvg")));
        assert_eq!(chosen.driver_dir, Some(PathBuf::from("/e/driver")));
        assert_eq!(chosen.firmware_dir, Some(PathBuf::from("/e/firmware")));
        assert!(chosen.fake_install);
    }

    #[test]
    fn a_request_names_a_program_only_to_a_build_that_is_not_signed() {
        assert_eq!(requested(false, Some("/q")), Some("/q"));
        assert_eq!(requested(true, Some("/q")), None);
    }
}
