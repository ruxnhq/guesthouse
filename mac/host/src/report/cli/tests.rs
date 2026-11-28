// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;
use std::cell::RefCell;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A terminal that answers from a script and remembers what was said.
struct Script {
    answers: Vec<String>,
    said: RefCell<Vec<String>>,
    asked: RefCell<Vec<String>>,
}

impl Script {
    fn new(answers: &[&str]) -> Script {
        Script {
            answers: answers.iter().rev().map(|s| s.to_string()).collect(),
            said: Default::default(),
            asked: Default::default(),
        }
    }
    fn said(&self) -> String {
        self.said.borrow().join("\n")
    }
}

impl Terminal for Script {
    fn ask(&mut self, question: &str) -> String {
        self.asked.borrow_mut().push(question.to_owned());
        self.answers.pop().unwrap_or_default()
    }
    fn say(&mut self, text: &str) {
        self.said.borrow_mut().push(text.to_owned());
    }
}

fn world(name: &str) -> (PathBuf, Places, PathBuf) {
    let root = std::env::temp_dir().join(format!("gh-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let support = root.join("support");
    fs::create_dir_all(support.join("Machines/demo")).unwrap();
    fs::create_dir_all(support.join("Run/demo")).unwrap();
    fs::write(support.join("Machines/demo/machine.json"), r#"{"name":"Demo","kind":"windows"}"#).unwrap();
    fs::write(support.join("Run/demo/qemu.log"), "started\n").unwrap();
    let out = root.join("out");
    fs::create_dir_all(&out).unwrap();
    (root.clone(), Places { support, home: root }, out)
}

#[test]
fn the_options_are_the_linux_tools_where_they_apply() {
    let options = read_options(args(&[
        "--machine",
        "Demo",
        "--kind",
        "crashed",
        "--description",
        "x",
        "--contact",
        "me",
        "--no-guest-serial",
        "--save-only",
    ]))
    .unwrap();
    assert_eq!(options.machine.as_deref(), Some("Demo"));
    assert_eq!(options.kind, ProblemKind::Crashed);
    assert!(options.no_guest_serial && options.save_only);
    assert!(read_options(args(&["--kind", "broken"])).unwrap_err().contains("The kinds are wont-start"));
    assert!(read_options(args(&["--machine"])).unwrap_err().contains("needs a value"));
    assert!(read_options(args(&["--nope"])).unwrap_err().contains("isn't an option"));
    assert!(
        read_options(args(&["--help"])).unwrap_err().starts_with("guesthouse-report: sends a bug report")
    );
}

#[test]
fn nothing_is_sent_unless_yes_is_typed() {
    for answer in ["", "y", "Y", "no", "YES please"] {
        let (root, places, out) = world("no");
        let sent = RefCell::new(0);
        let mut term = Script::new(&["", answer]);
        let sender = |_: &Path, _: &str| {
            *sent.borrow_mut() += 1;
            Ok("0a1b2c3d4e".to_owned())
        };
        let options = Options { machine: Some("demo".into()), ..Options::default() };
        let _ = run(options, &places, "0.1.0", &out, &mut term, &sender);
        assert_eq!(*sent.borrow(), 0, "{answer:?} sent it");
        assert!(term.said().contains("Nothing was sent."));
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn it_shows_what_it_holds_before_asking_then_sends_on_yes_and_says_the_id() {
    let (root, places, out) = world("yes");
    let sent: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let mut term = Script::new(&["It went black", "yes"]);
    let sender = |archive: &Path, header: &str| {
        sent.borrow_mut().push(format!(
            "{} {}",
            archive.extension().unwrap().to_string_lossy(),
            !header.is_empty()
        ));
        Ok("0a1b2c3d4e".to_owned())
    };
    let options = Options { machine: Some("Demo".into()), ..Options::default() };
    let _ = run(options, &places, "0.1.0", &out, &mut term, &sender);
    let said = term.said();
    assert!(said.find("The report holds:").unwrap() < said.find("Sending gives this file").unwrap());
    assert_eq!(term.asked.borrow().len(), 2, "the description, then the yes");
    assert!(term.asked.borrow()[1].contains("Type yes to send"));
    assert_eq!(sent.borrow().as_slice(), ["gz true"]);
    assert!(said.contains("Sent. The report's ID is 0a1b2c3d4e."));
    assert!(said.contains("issues/new?title=Report%200a1b2c3d4e"));
    assert!(
        fs::read_dir(&out).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".tar.gz")),
        "a copy is saved here"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn save_only_writes_the_file_and_asks_nothing_about_sending() {
    let (root, places, out) = world("save");
    let mut term = Script::new(&[]);
    let options = Options { save_only: true, description: Some("d".into()), ..Options::default() };
    let _ = run(options, &places, "0.1.0", &out, &mut term, &|_, _| panic!("nothing is sent"));
    assert!(term.asked.borrow().is_empty());
    assert!(term.said().contains("Written to"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_failed_send_says_why_and_where_the_file_is() {
    let (root, places, out) = world("fail");
    let mut term = Script::new(&["", "yes"]);
    let sender = |_: &Path, _: &str| Err("The report server couldn't be reached: offline".to_owned());
    let _ = run(Options::default(), &places, "0.1.0", &out, &mut term, &sender);
    let said = term.said();
    assert!(said.contains("It wasn't sent. The report server couldn't be reached: offline"));
    assert!(said.contains("The report is still in"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_unknown_machine_is_refused_by_name() {
    let (root, places, _) = world("unknown");
    assert!(choose(&places.support, Some("Nope")).unwrap_err().contains("The machines are: Demo."));
    assert_eq!(choose(&places.support, Some("DEMO")).unwrap().unwrap().0, "demo");
    let _ = fs::remove_dir_all(root);
}
