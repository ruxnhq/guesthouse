//! The words typed after `guesthouse-vm`, by hand like the other programs here.

use super::{Problem, USAGE};
use std::fmt;

const NO_WAIT: &str = "--no-wait";
const YES: &str = "--yes";
/// The options of `new` that take a value.
const NEW_OPTIONS: [&str; 13] = [
    "--iso",
    "--kind",
    "--disk",
    "--disk-folder",
    "--cores",
    "--ram",
    "--user",
    "--amd-card",
    "--amd-driver",
    "--intel-driver",
    "--remote-desktop",
    "--ssh",
    "--set-up-for",
];
const SETUP_OPTIONS: [&str; 2] = ["--driver", "--card"];
const MINUTES: &str = "--minutes";

/// What `new` was told. Whatever it wasn't told, the New Machine dialog's choice stands.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct NewOptions<'a> {
    pub iso: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub disk: Option<&'a str>,
    pub disk_folder: Option<&'a str>,
    pub cores: Option<&'a str>,
    pub ram: Option<&'a str>,
    pub user: Option<&'a str>,
    pub amd_card: Option<&'a str>,
    /// AMD's installer, which the dialog asks for when the card is AMD's and has no driver yet.
    pub amd_driver: Option<&'a str>,
    /// Intel's installer, which the dialog asks for when the card is Intel's.
    pub intel_driver: Option<&'a str>,
    /// On or off, for a Windows machine.
    pub remote_desktop: Option<&'a str>,
    pub ssh: Option<&'a str>,
    /// A recipe, by its key, its file's name or its name.
    pub set_up_for: Option<&'a str>,
    /// Goes ahead with a recipe of the user's own, whose steps `new` shows first.
    pub yes: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Forwarding<'a> {
    Add { protocol: &'a str, host: &'a str, guest: &'a str },
    Remove { protocol: &'a str, host: &'a str, guest: &'a str },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Command<'a> {
    Help,
    List,
    Status(&'a str),
    Show(&'a str),
    Start {
        name: &'a str,
        wait: bool,
    },
    Install {
        name: &'a str,
        wait: bool,
    },
    Recipes,
    RunRecipe {
        name: &'a str,
        wait: bool,
    },
    Stop(&'a str),
    Off(&'a str),
    Log(&'a str),
    Standalone(&'a str),
    Idle {
        name: &'a str,
        minutes: Option<&'a str>,
    },
    New {
        name: &'a str,
        options: NewOptions<'a>,
    },
    Set {
        name: &'a str,
        key: &'a str,
        value: &'a str,
    },
    Forward {
        name: &'a str,
        change: Forwarding<'a>,
    },
    Setup {
        driver: Option<&'a str>,
        card: Option<&'a str>,
    },
    Remove {
        name: &'a str,
        yes: bool,
    },
    Snapshots(&'a str),
    /// Back to the named snapshot, or with none to the one taken before a start that failed.
    GoBack {
        name: &'a str,
        snapshot: Option<&'a str>,
        yes: bool,
    },
    TakeSnapshot {
        name: &'a str,
        snapshot: &'a str,
    },
    DeleteSnapshot {
        name: &'a str,
        snapshot: &'a str,
        yes: bool,
    },
}

pub(super) fn mistake(why: impl fmt::Display) -> Problem {
    Problem::Usage(format!("guesthouse-vm: {why}\n\n{USAGE}"))
}

/// The words that aren't options, and the options with what they were given.
struct Split<'a> {
    words: Vec<&'a str>,
    values: Vec<(&'a str, &'a str)>,
    switches: Vec<&'a str>,
}

impl<'a> Split<'a> {
    fn new(
        command: &str,
        rest: &'a [String],
        valued: &[&str],
        switches: &[&str],
    ) -> std::result::Result<Self, Problem> {
        let mut split = Self { words: Vec::new(), values: Vec::new(), switches: Vec::new() };
        let mut rest = rest.iter().map(String::as_str);
        while let Some(word) = rest.next() {
            if valued.contains(&word) {
                let value = rest.next().ok_or_else(|| mistake(format!("{word} needs a value.")))?;
                split.values.push((word, value));
            } else if switches.contains(&word) {
                split.switches.push(word);
            } else if word.starts_with("--") {
                return Err(mistake(format!("{command} has no option {word}.")));
            } else {
                split.words.push(word);
            }
        }
        Ok(split)
    }

    fn value(&self, option: &str) -> Option<&'a str> {
        self.values.iter().find(|(name, _)| *name == option).map(|(_, value)| *value)
    }

    fn has(&self, switch: &str) -> bool {
        self.switches.contains(&switch)
    }

    /// Exactly `count` words, or says what is missing or too much.
    fn exactly(&self, command: &str, what: &str, count: usize) -> std::result::Result<&[&'a str], Problem> {
        match self.words.len() {
            found if found == count => Ok(&self.words),
            found if found < count => Err(mistake(format!("{command} needs {what}."))),
            _ if count == 0 => Err(mistake(format!("{command} takes no words, only options."))),
            _ => Err(mistake(format!("{command} takes only {what}."))),
        }
    }
}

pub(super) fn parse(arguments: &[String]) -> std::result::Result<Command<'_>, Problem> {
    let Some((command, rest)) = arguments.split_first() else { return Err(Problem::Usage(USAGE.to_owned())) };
    let command = command.as_str();
    let machine = "a machine";
    match command {
        "help" | "-h" | "--help" => Ok(Command::Help),
        "list" => {
            Split::new(command, rest, &[], &[])?.exactly(command, "", 0)?;
            Ok(Command::List)
        }
        "status" | "show" | "stop" | "off" | "log" | "command" => {
            let split = Split::new(command, rest, &[], &[])?;
            let name = split.exactly(command, machine, 1)?[0];
            Ok(match command {
                "status" => Command::Status(name),
                "show" => Command::Show(name),
                "stop" => Command::Stop(name),
                "off" => Command::Off(name),
                "command" => Command::Standalone(name),
                _ => Command::Log(name),
            })
        }
        "recipes" => {
            Split::new(command, rest, &[], &[])?.exactly(command, "", 0)?;
            Ok(Command::Recipes)
        }
        "recipe" => {
            let split = Split::new(command, rest, &[], &[NO_WAIT])?;
            Ok(Command::RunRecipe { name: split.exactly(command, machine, 1)?[0], wait: !split.has(NO_WAIT) })
        }
        "idle" => {
            let split = Split::new(command, rest, &[MINUTES], &[])?;
            Ok(Command::Idle { name: split.exactly(command, machine, 1)?[0], minutes: split.value(MINUTES) })
        }
        "start" | "install" => {
            let split = Split::new(command, rest, &[], &[NO_WAIT])?;
            let (name, wait) = (split.exactly(command, machine, 1)?[0], !split.has(NO_WAIT));
            Ok(if command == "start" {
                Command::Start { name, wait }
            } else {
                Command::Install { name, wait }
            })
        }
        "snapshots" => {
            let split = Split::new(command, rest, &[], &[])?;
            Ok(Command::Snapshots(split.exactly(command, machine, 1)?[0]))
        }
        "go-back" => {
            let split = Split::new(command, rest, &[], &[YES])?;
            let words = match split.words.len() {
                2 => &split.words[..],
                _ => split.exactly(command, machine, 1)?,
            };
            Ok(Command::GoBack { name: words[0], snapshot: words.get(1).copied(), yes: split.has(YES) })
        }
        "take-snapshot" => {
            let split = Split::new(command, rest, &[], &[])?;
            let [name, snapshot] = split.exactly(command, "a machine and a name", 2)? else {
                unreachable!("exactly two words")
            };
            Ok(Command::TakeSnapshot { name, snapshot })
        }
        "delete-snapshot" => {
            let split = Split::new(command, rest, &[], &[YES])?;
            let [name, snapshot] = split.exactly(command, "a machine and a snapshot", 2)? else {
                unreachable!("exactly two words")
            };
            Ok(Command::DeleteSnapshot { name, snapshot, yes: split.has(YES) })
        }
        "remove" => {
            let split = Split::new(command, rest, &[], &[YES])?;
            Ok(Command::Remove { name: split.exactly(command, machine, 1)?[0], yes: split.has(YES) })
        }
        "new" => {
            let split = Split::new(command, rest, &NEW_OPTIONS, &[YES])?;
            let name = split.exactly(command, "a name for the machine", 1)?[0];
            let options = NewOptions {
                iso: split.value("--iso"),
                kind: split.value("--kind"),
                disk: split.value("--disk"),
                disk_folder: split.value("--disk-folder"),
                cores: split.value("--cores"),
                ram: split.value("--ram"),
                user: split.value("--user"),
                amd_card: split.value("--amd-card"),
                amd_driver: split.value("--amd-driver"),
                intel_driver: split.value("--intel-driver"),
                remote_desktop: split.value("--remote-desktop"),
                ssh: split.value("--ssh"),
                set_up_for: split.value("--set-up-for"),
                yes: split.has(YES),
            };
            Ok(Command::New { name, options })
        }
        "set" => {
            let split = Split::new(command, rest, &[], &[])?;
            let [name, key, value] = split.exactly(command, "a machine, a setting and a value", 3)? else {
                unreachable!("exactly three words")
            };
            Ok(Command::Set { name, key, value })
        }
        "forward" => {
            let split = Split::new(command, rest, &[], &[])?;
            let [name, what, protocol, host, guest] =
                split.exactly(command, "a machine, add or remove, a protocol, and two ports", 5)?
            else {
                unreachable!("exactly five words")
            };
            let change = match *what {
                "add" => Forwarding::Add { protocol, host, guest },
                "remove" => Forwarding::Remove { protocol, host, guest },
                other => return Err(mistake(format!("forward is add or remove, not {other}."))),
            };
            Ok(Command::Forward { name, change })
        }
        "setup" => {
            let split = Split::new(command, rest, &SETUP_OPTIONS, &[])?;
            split.exactly(command, "", 0)?;
            Ok(Command::Setup { driver: split.value("--driver"), card: split.value("--card") })
        }
        other => Err(mistake(format!("there is no command called {other}."))),
    }
}
