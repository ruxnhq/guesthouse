//! The daemon's process behaviour as its user meets it: the arguments it takes, the variables it
//! reads first, the lines it writes before it serves and the codes it exits with. These run the built
//! program. They hold what it does today, so the shell around it can move into a crate other hosts can
//! use without anyone seeing a difference.
//!
//! A build with no backend can only refuse, so the folder it serves and the order of its start are
//! seen with the `stand-in` feature alone, which serves sockets with no GPU behind them.

use std::process::{Command, Output};

const DAEMON: &str = env!("CARGO_BIN_EXE_guesthouse-daemon");

/// The daemon run to its end with only the variables given, as anything a person has set in their
/// own shell could change what it does.
fn run(arguments: &[&str], variables: &[(&str, &std::ffi::OsStr)]) -> Output {
    let mut command = Command::new(DAEMON);
    command.args(arguments).env_clear().envs(variables.iter().copied());
    // Windows can't start a program with no system root.
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    command.output().unwrap()
}

#[cfg(not(target_os = "linux"))]
#[test]
fn off_linux_it_says_it_runs_on_linux_only() {
    let output = run(&[], &[]);
    assert_eq!(String::from_utf8_lossy(&output.stderr), "guesthouse-daemon runs on Linux only.\n");
    assert!(output.stdout.is_empty());
    assert_eq!(output.status.code(), Some(1));
    // Whatever it is given.
    let output = run(&["--help", "folder"], &[("GUESTHOUSE_SLOT", "x".as_ref())]);
    assert_eq!(String::from_utf8_lossy(&output.stderr), "guesthouse-daemon runs on Linux only.\n");
    assert_eq!(output.status.code(), Some(1));
}

#[cfg(target_os = "linux")]
mod linux {
    use super::run;
    use std::ffi::OsStr;
    use std::process::Output;

    fn stderr(output: &Output) -> &str {
        std::str::from_utf8(&output.stderr).unwrap()
    }

    fn slot(value: &str) -> (&'static str, &OsStr) {
        ("GUESTHOUSE_SLOT", value.as_ref())
    }

    #[test]
    fn a_bad_slot_stops_it_with_the_value_it_was_given() {
        for value in ["x", "4", "-1", "+1", " 1", "1 ", "256", "", "٣"] {
            let output = run(&[], &[slot(value)]);
            assert_eq!(
                stderr(&output),
                format!("GUESTHOUSE_SLOT must be a number below 4, not {value:?}\n"),
                "{value:?}"
            );
            assert!(output.stdout.is_empty(), "{value:?}");
            assert_eq!(output.status.code(), Some(1), "{value:?}");
        }
    }

    /// The slot is read before the arguments or the backend are looked at, and with the logger
    /// already started, which writes nothing of its own here, verbose or not.
    #[test]
    fn the_slot_is_read_before_anything_else() {
        let verbose = ("GUESTHOUSE_VERBOSE", OsStr::new("1"));
        let output = run(&["--help", "two"], &[slot("x"), verbose]);
        assert_eq!(stderr(&output), "GUESTHOUSE_SLOT must be a number below 4, not \"x\"\n");
        assert_eq!(output.status.code(), Some(1));
    }

    /// A card's sensors publisher stays while the daemon that started it holds its input open, and
    /// ends once that closes, as when the daemon ends. It needs no slot.
    #[test]
    fn a_sensors_publisher_ends_with_its_daemon() {
        use std::io::Read;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let mut publisher = Command::new(super::DAEMON)
            .args(["--publish-sensors", "intel", "0000:ff:1f.7"])
            .env_clear()
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut said = String::new();
        let mut stderr = publisher.stderr.take().unwrap();
        let mut byte = [0u8; 1];
        while !said.ends_with('\n') && stderr.read(&mut byte).unwrap() == 1 {
            said.push(char::from(byte[0]));
        }
        assert!(said.contains("no host sensors"), "{said}");
        assert!(publisher.try_wait().unwrap().is_none(), "it waits for its daemon");
        drop(publisher.stdin.take());
        let end = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = publisher.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < end, "it went on after its daemon");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success());
    }

    #[test]
    fn a_sensors_publisher_for_a_maker_without_readings_is_refused() {
        let output = run(&["--publish-sensors", "amd"], &[]);
        assert_eq!(stderr(&output), "not a card maker with readings\n");
        assert_eq!(output.status.code(), Some(1));
    }

    #[cfg(not(any(feature = "stand-in", feature = "nvidia", feature = "amd", feature = "intel")))]
    mod no_backend {
        use super::{OsStr, run, slot, stderr};
        use std::os::unix::ffi::OsStrExt;

        const NO_BACKEND: &str = "This build has no GPU backend in it, so it cannot run a guest.\n";

        #[test]
        fn a_build_with_no_backend_says_so_and_exits_1() {
            for variables in [vec![], vec![slot("0")], vec![slot("3")]] {
                let output = run(&[], &variables);
                assert_eq!(stderr(&output), NO_BACKEND);
                assert!(output.stdout.is_empty());
                assert_eq!(output.status.code(), Some(1));
            }
        }

        /// Any first argument is a folder, so there is no usage or version to print, and whatever
        /// else follows it is never read.
        #[test]
        fn arguments_that_look_like_flags_are_not_flags() {
            for arguments in [&["--help"][..], &["--version"], &["-h"], &["--socket", "x"], &["a", "b", "c"]]
            {
                let output = run(arguments, &[]);
                assert_eq!(stderr(&output), NO_BACKEND, "{arguments:?}");
                assert!(output.stdout.is_empty(), "{arguments:?}");
                assert_eq!(output.status.code(), Some(1), "{arguments:?}");
            }
        }

        /// Odd but so: a value that isn't UTF-8 is taken as no value, which is slot 0, and leading
        /// zeros are allowed.
        #[test]
        fn some_slot_values_that_look_wrong_are_taken() {
            let not_utf8 = OsStr::from_bytes(b"\xff");
            for value in [not_utf8, OsStr::new("01"), OsStr::new("003")] {
                let output = run(&[], &[("GUESTHOUSE_SLOT", value)]);
                assert_eq!(stderr(&output), NO_BACKEND, "{value:?}");
                assert_eq!(output.status.code(), Some(1), "{value:?}");
            }
        }

        /// `GUESTHOUSE_VERBOSE` set to anything, even nothing, turns the verbose log on, which has no
        /// line of its own here: the refusal is no log line, so it has no level and no time.
        #[test]
        fn the_refusal_is_a_bare_line_verbose_or_not() {
            for verbose in ["", "0", "1"] {
                let output = run(&[], &[("GUESTHOUSE_VERBOSE", verbose.as_ref())]);
                assert_eq!(stderr(&output), NO_BACKEND, "{verbose:?}");
                assert_eq!(output.status.code(), Some(1), "{verbose:?}");
            }
        }
    }

    /// The stand-in serves for real, so these see which folder it serves and in what order it starts.
    #[cfg(all(feature = "stand-in", not(any(feature = "nvidia", feature = "amd", feature = "intel"))))]
    mod stand_in {
        use super::{OsStr, run, slot, stderr};
        use std::fs;
        use std::path::{Path, PathBuf};
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        const PATIENCE: Duration = Duration::from_secs(30);

        /// An empty folder of the test's own, which the daemon is started in.
        fn scratch(name: &str) -> PathBuf {
            let folder =
                std::env::temp_dir().join(format!("guesthouse-daemon-process-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&folder);
            fs::create_dir_all(&folder).unwrap();
            folder
        }

        /// Starts the daemon in `folder`, with the jail off so its lines don't depend on the kernel,
        /// waits until all its sockets are in `run_folder`, stops it and gives what it wrote.
        fn serve(
            folder: &Path,
            arguments: &[&str],
            variables: &[(&str, &OsStr)],
            run_folder: &Path,
        ) -> String {
            let log = folder.join("daemon.err");
            let mut daemon = Command::new(super::super::DAEMON)
                .args(arguments)
                .current_dir(folder)
                .env_clear()
                .env("GUESTHOUSE_JAIL", "off")
                .envs(variables.iter().copied())
                .stdout(Stdio::null())
                .stderr(fs::File::create(&log).unwrap())
                .spawn()
                .unwrap();
            let end = Instant::now() + PATIENCE;
            while !run_folder.join("frames.sock").exists() {
                let gone = daemon.try_wait().unwrap();
                assert!(
                    gone.is_none() && Instant::now() < end,
                    "{gone:?}: {}",
                    fs::read_to_string(&log).unwrap()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = daemon.kill();
            let _ = daemon.wait();
            for socket in ["global.sock", "vgpu.sock", "frames.sock"] {
                assert!(run_folder.join(socket).exists(), "{socket}");
            }
            fs::read_to_string(&log).unwrap()
        }

        fn started(run_folder: &str) -> String {
            format!(
                "WARN  no guest memory was handed over\n\
                 WARN  not jailed, as GUESTHOUSE_JAIL is off\n\
                 WARN  serving {run_folder} with a stand-in GPU: this will not run a guest\n"
            )
        }

        #[test]
        fn an_argument_that_looks_like_a_flag_is_the_project_folder() {
            let folder = scratch("flag");
            let lines =
                serve(&folder, &["--socket", "ignored"], &[slot("2")], &folder.join("--socket/run-2"));
            assert_eq!(lines, started("--socket/run-2"));
            assert!(!folder.join("ignored").exists());
            fs::remove_dir_all(folder).unwrap();
        }

        #[test]
        fn with_no_argument_the_project_folder_is_the_current_one() {
            let folder = scratch("none");
            assert_eq!(serve(&folder, &[], &[], &folder.join("run")), started("./run"));
            fs::remove_dir_all(folder).unwrap();
        }

        /// The verbose log has the time after the level: masked here, as it is the clock's.
        #[test]
        fn the_verbose_start_has_times() {
            // Set to anything, even nothing or 0, it is on.
            for value in ["", "0"] {
                let folder = scratch(&format!("verbose{value}"));
                let verbose = ("GUESTHOUSE_VERBOSE", OsStr::new(value));
                let lines =
                    serve(&folder, &["project"], &[slot("1"), verbose], &folder.join("project/run-1"));
                assert_eq!(masked(&lines), VERBOSE_START, "{value:?}");
                fs::remove_dir_all(folder).unwrap();
            }
        }

        const VERBOSE_START: &str = "WARN  [    T.TTT] no guest memory was handed over\n\
                                     WARN  [    T.TTT] not jailed, as GUESTHOUSE_JAIL is off\n\
                                     WARN  [    T.TTT] serving project/run-1 with a stand-in GPU: this will not \
                                     run a guest\n";

        /// The lines with their times masked, after checking each has one.
        fn masked(lines: &str) -> String {
            lines
                .lines()
                .map(|line| {
                    let (level, rest) = line.split_at(6);
                    let (time, message) = rest.split_at(12);
                    assert!(time.starts_with('[') && time.ends_with("] "), "{line}");
                    let seconds = time[1..10].trim_start();
                    assert!(seconds.len() >= 5 && seconds.parse::<f64>().is_ok(), "{line}");
                    assert_eq!(seconds.split_once('.').unwrap().1.len(), 3, "{line}");
                    format!("{level}[    T.TTT] {message}\n")
                })
                .collect()
        }

        /// The memory is looked at before the run folder is made, so a mistake there leaves none.
        #[test]
        fn a_bad_memory_setting_stops_it_before_the_run_folder() {
            let folder = scratch("memory");
            let project = folder.join("project");
            let ram = ("GUESTHOUSE_RAM_FILE", folder.join("ram").into_os_string());
            let output = run(&[project.to_str().unwrap()], &[(ram.0, &ram.1)]);
            assert_eq!(
                stderr(&output),
                "GUESTHOUSE_RAM_FILE is set, and GUESTHOUSE_WINDOW_FILE is not. The guest's memory is two files, so \
                 set both or neither.\n"
            );
            assert_eq!(output.status.code(), Some(1));
            assert!(!project.exists() && !folder.join("ram").exists());
            fs::remove_dir_all(folder).unwrap();
        }

        /// The run folder is made after the memory and before the jail: one that can't be made stops
        /// it with the system's words and no level.
        #[test]
        fn a_run_folder_that_cannot_be_made_stops_it_before_the_jail() {
            let folder = scratch("file");
            fs::write(folder.join("project"), "a file").unwrap();
            let project = folder.join("project");
            let output = run(&[project.to_str().unwrap()], &[]);
            assert_eq!(
                stderr(&output),
                "WARN  no guest memory was handed over\nNot a directory (os error 20)\n"
            );
            assert_eq!(output.status.code(), Some(1));
            fs::remove_dir_all(folder).unwrap();
        }
    }
}
