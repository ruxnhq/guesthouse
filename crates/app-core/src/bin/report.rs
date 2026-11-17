//! `guesthouse-report`: gathers a bug report about one machine and this PC into one file, shows what's
//! in it, and sends it to the developer if the tester says yes. It's a program of its own, with no
//! GTK, so it works when the app won't open or never could.

use guesthouse_app_core::context::Context;
use guesthouse_app_core::log::read_log;
use guesthouse_app_core::machine::load_all;
use guesthouse_app_core::report::endpoint::KEPT_DAYS;
use guesthouse_app_core::report::issue_link::{failure_excerpt, issue_link};
use guesthouse_app_core::report::send::send;
use guesthouse_app_core::report::{
    Bundle, Collection, ProblemKind, Request, Scrubber, Words, chosen_machine, collect, latest_machine,
    write_bundle,
};
use rustix::fs::{Access, access};
use std::env;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
guesthouse-report: sends a bug report about Guesthouse to its developer.

It gathers the app's log, one machine's logs from its last runs, its settings, and facts about this
PC, takes your user name, host name and home folder out of them where it finds them, and writes
them into one file here. It shows what's in the file and asks before sending anything.

  --machine NAME      the machine the report is about. Without it, the one that ran last.
  --kind KIND         what went wrong: wont-start, crashed, picture, slow, install-failed or other.
  --description TEXT  what went wrong, in your words. Without it, you're asked.
  --contact TEXT      how the developer can reach you, if you'd like them to. It isn't made public.
  --no-guest-serial   leaves out the guest's serial log, the one thing that comes from inside it.
  --save-only         writes the file and stops, with nothing sent.
  --help              this.
";

#[derive(Default)]
struct Options {
    machine: Option<String>,
    kind: ProblemKind,
    description: Option<String>,
    contact: String,
    no_guest_serial: bool,
    save_only: bool,
}

fn read_options() -> Result<Options, String> {
    let mut options = Options::default();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || arguments.next().ok_or_else(|| format!("{argument} needs a value after it."));
        match argument.as_str() {
            "--machine" => options.machine = Some(value()?),
            "--kind" => {
                let word = value()?;
                options.kind = ProblemKind::from_word(&word)
                    .ok_or_else(|| format!("{word} isn't a kind. {}", kinds()))?;
            }
            "--description" => options.description = Some(value()?),
            "--contact" => options.contact = value()?,
            "--no-guest-serial" => options.no_guest_serial = true,
            "--save-only" => options.save_only = true,
            "--help" | "-h" => return Err(HELP.to_owned()),
            other => return Err(format!("{other} isn't an option here.\n\n{HELP}")),
        }
    }
    Ok(options)
}

fn kinds() -> String {
    let words: Vec<&str> = ProblemKind::ALL.iter().map(|kind| kind.word()).collect();
    format!("The kinds are {}.", words.join(", "))
}

/// The answer to a question on the terminal, or nothing when nobody is there to answer.
fn ask(question: &str) -> String {
    if !io::stdin().is_terminal() {
        return String::new();
    }
    print!("{question}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    line.trim().to_owned()
}

/// Here, if this folder can be written to, and the temp folder if not.
fn output_folder() -> PathBuf {
    let here = env::current_dir().ok().filter(|folder| access(folder, Access::WRITE_OK).is_ok());
    here.unwrap_or_else(env::temp_dir)
}

fn show_contents(collection: &Collection) {
    println!("The report holds:");
    for name in collection.names() {
        println!("  {name}");
    }
    println!("  description.txt, summary.json and manifest.txt");
    if !collection.missing.is_empty() {
        println!("\nNot in it:");
        for missing in &collection.missing {
            println!("  {}: {}", missing.name, missing.reason);
            if let Some(how) = &missing.how {
                println!("    You can get it yourself with: {how}");
            }
        }
    }
    for note in &collection.notes {
        println!("  {note}");
    }
}

fn send_and_say(context: &Context, bundle: &Bundle, description: &str) -> ExitCode {
    println!("Sending...");
    match send(bundle) {
        Ok(id) => {
            println!("Sent. The report's ID is {id}. Mention it wherever you talk about this problem.");
            let log = Scrubber::for_this_pc(&context.paths.home).scrub(&read_log(context));
            let link = issue_link(&id, &bundle.summary, description, &failure_excerpt(&log));
            println!(
                "\nIf you'd like to open a GitHub issue too, this link fills one in for you to check first:\n{link}"
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            println!("It wasn't sent. {}", failure.0);
            println!(
                "The report is still in {}. You can attach it to an issue at \
                 https://github.com/xnixy/guesthouse/issues, or run this again later.",
                bundle.path.display()
            );
            ExitCode::FAILURE
        }
    }
}

fn run(context: &Context, options: Options) -> ExitCode {
    let machine = match &options.machine {
        Some(wanted) => match chosen_machine(context, wanted) {
            Some(machine) => Some(machine),
            None => {
                let (machines, _) = load_all(context);
                let names: Vec<&str> = machines.iter().map(|machine| machine.name.as_str()).collect();
                eprintln!("There's no machine called {wanted}. The machines are: {}.", names.join(", "));
                return ExitCode::FAILURE;
            }
        },
        None => latest_machine(context),
    };
    match &machine {
        Some(machine) => println!("The report is about {}, the {}.", machine.name, machine.kind_name()),
        None => println!(
            "No machine has run since this PC started, so the report is about the app and this PC. --machine picks one."
        ),
    }
    println!("Gathering...");
    let request = Request {
        machine,
        kind: options.kind,
        guest_serial: !options.no_guest_serial,
        app_facts: Vec::new(),
    };
    let collection = collect(context, &request);
    show_contents(&collection);
    println!();
    let description =
        options.description.unwrap_or_else(|| ask("What went wrong? One line, or Enter to skip: "));
    let words = Words { description: description.clone(), contact: options.contact };
    let scrubber = Scrubber::for_this_pc(&context.paths.home);
    let bundle = match write_bundle(&collection, &words, &scrubber, &output_folder()) {
        Ok(bundle) => bundle,
        Err(failure) => {
            eprintln!("The report couldn't be written: {}", failure.0);
            return ExitCode::FAILURE;
        }
    };
    println!("Written to {}", bundle.path.display());
    if options.save_only {
        return ExitCode::SUCCESS;
    }
    println!(
        "\nSending gives this file to Guesthouse's developer. It's read by the developer and their \
         automation tools, and deleted after {KEPT_DAYS} days. Your user name, host name and home folder \
         were taken out where they were found, but logs are free text, so look through it first if that \
         matters to you."
    );
    if !ask("Send? [y/N] ").eq_ignore_ascii_case("y") {
        println!("Nothing was sent.");
        return ExitCode::SUCCESS;
    }
    send_and_say(context, &bundle, &description)
}

fn main() -> ExitCode {
    let options = match read_options() {
        Ok(options) => options,
        Err(text) => {
            if text == HELP {
                print!("{text}");
                return ExitCode::SUCCESS;
            }
            eprintln!("{text}");
            return ExitCode::FAILURE;
        }
    };
    let context = match Context::from_process() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    run(&context, options)
}
