//! Gathers what goes into a report, each part from a list of its own. Nothing is taken by looking
//! through a folder for whatever is there, so nothing gets in that nobody chose.
//!
//! One thing that can't be read never stops the rest. It goes in the manifest instead, with why, and
//! the command that would get it if there is one.

mod app_files;
mod command;
mod crashes;
mod extras;
mod host_checks;
mod host_facts;
mod kernel_log;
mod machine_files;
mod run_logs;
mod running;

pub use machine_files::{chosen_machine, latest_machine};

use super::kind::ProblemKind;
use super::summary::{self, Summary};
use crate::context::Context;
use crate::machine::Machine;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

/// What the tester chose about a report.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// The machine the report is about. None is one with no machine, such as an app that won't open.
    pub machine: Option<Machine>,
    pub kind: ProblemKind,
    /// The guest's serial console, which is the one part of a report that comes from inside the guest.
    pub guest_serial: bool,
    /// Facts only the running app knows, such as the GTK and libadwaita it loaded.
    pub app_facts: Vec<(String, String)>,
}

/// A file of the report, as text, before it's scrubbed or trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Its path in the bundle.
    pub name: String,
    pub text: String,
}

/// Something that would have gone in, and didn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub name: String,
    pub reason: String,
    /// A command the tester could run to get it by hand, if there is one.
    pub how: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Collection {
    pub items: Vec<Item>,
    pub missing: Vec<Missing>,
    /// Lines for the manifest that explain what's there and what isn't.
    pub notes: Vec<String>,
    pub summary: Summary,
}

impl Collection {
    pub fn add(&mut self, name: &str, text: String) {
        self.items.push(Item { name: name.to_owned(), text });
    }

    pub fn miss(&mut self, name: &str, reason: &str, how: Option<&str>) {
        self.missing.push(Missing {
            name: name.to_owned(),
            reason: reason.to_owned(),
            how: how.map(str::to_owned),
        });
    }

    /// A file that should be there, so its absence goes in the manifest.
    fn add_file(&mut self, name: &str, path: &Path) {
        match fs::read(path) {
            Ok(data) => self.add(name, String::from_utf8_lossy(&data).into_owned()),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.miss(name, "there is no such file", None)
            }
            Err(error) => self.miss(name, &format!("it could not be read: {error}"), None),
        }
    }

    /// A file that's only there in some runs, so it being missing says nothing.
    fn add_file_if_there(&mut self, name: &str, path: &Path) {
        if path.exists() {
            self.add_file(name, path);
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(|item| item.name.as_str())
    }
}

/// Everything the request asks for. It reads and runs only things that change nothing.
pub fn collect(context: &Context, request: &Request) -> Collection {
    let mut found = Collection::default();
    found.summary.set(summary::VERSION, crate::updates::VERSION);
    found.summary.set(summary::KIND, request.kind.word());
    app_files::collect(context, &mut found);
    if let Some(machine) = &request.machine {
        found.summary.set(summary::MACHINE, machine.kind_name());
        machine_files::collect(context, machine, &mut found);
        run_logs::collect(context, machine, request, &mut found);
    } else {
        found
            .notes
            .push("No machine was chosen, so no machine's files or logs are in this report.".to_owned());
    }
    host_facts::collect(context, request, &mut found);
    host_checks::collect(context, &mut found);
    kernel_log::collect(&mut found);
    crashes::collect(request.kind, &mut found);
    running::collect(context, request.machine.as_ref(), &mut found);
    extras::collect(context, request, &mut found);
    found
}
