//! The commands, over `guesthouse-host-windows`: its events printed as the tool always has, and
//! Ctrl+C as the stop flag.

use crate::{Command, Options};
use guesthouse_hcs::gpu;
use guesthouse_hcs::host;
use guesthouse_hcs::schema::{Gpu, ModifySettingRequest};
use guesthouse_hcs::serial::pipe_tag;
use guesthouse_host_windows::events::{Event, Sink};
use guesthouse_host_windows::record::Record;
use guesthouse_host_windows::windows::{absolute, boot, chosen_card, machine, prepare};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// How often Ctrl+C is passed on to the boot's stop flag.
const CTRL_C_POLL: Duration = Duration::from_millis(200);

pub fn run(options: &Options) -> Result<(), String> {
    match options.command {
        Command::Gpus => list_cards(),
        Command::Document => print_document(options),
        Command::Prepare => prepare::prepare(&options.prepare_settings(), &Printed),
        Command::Boot => run_boot(options),
    }
}

/// Prints each event: notes and the guest's lines on standard output, problems on standard error.
struct Printed;

impl Sink for Printed {
    fn event(&self, event: Event) {
        match event {
            Event::Note(text) => println!("{text}"),
            Event::Problem(text) => eprintln!("{text}"),
            Event::Guest(text) => println!("guest: {text}"),
            Event::Report(text) => println!("report: {text}"),
            // The notes already say each step, so the state isn't printed twice.
            Event::State(_) => {}
        }
    }
}

fn run_boot(options: &Options) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    let ctrl_c = host::catch_ctrl_c().map_err(|error| error.to_string())?;
    let passed = Arc::clone(&stop);
    thread::spawn(move || {
        loop {
            thread::sleep(CTRL_C_POLL);
            if ctrl_c.swap(false, Ordering::SeqCst) {
                println!("Ctrl+C: asking once asks the guest to shut down, and again stops it at once.");
                passed.store(true, Ordering::SeqCst);
            }
        }
    });
    boot::boot(&options.boot_settings(), Arc::new(Printed), stop)
}

fn list_cards() -> Result<(), String> {
    let cards = gpu::cards().map_err(|error| error.to_string())?;
    if cards.is_empty() {
        println!("No card on this host can be shared with a guest.");
    }
    let default = gpu::default_card(&cards).map(|card| card.interface_path.clone());
    for (number, card) in cards.iter().enumerate() {
        let chosen = if default.as_ref() == Some(&card.interface_path) { " (the default)" } else { "" };
        println!("{number}: {}{chosen}", card.name);
        println!("   device:    {}", card.instance_id);
        println!("   interface: {}", card.interface_path);
        match card.memory {
            Some(bytes) => println!("   memory:    {} MB of its own", bytes >> 20),
            None => println!("   memory:    (its driver doesn't say)"),
        }
        match &card.driver_folder {
            Some(folder) => println!("   driver:    {}", folder.display()),
            None => println!("   driver:    (its folder could not be found)"),
        }
    }
    Ok(())
}

fn print_document(options: &Options) -> Result<(), String> {
    let settings = options.boot_settings();
    let disk = absolute(&settings.disk)?;
    let record = Record::load(&disk)?;
    // Each boot names its pipes afresh, as this does.
    let tag = pipe_tag().map_err(|error| error.to_string())?;
    let machine = machine(&settings, disk, &record, None, tag);
    println!("{}", serde_json::to_string_pretty(&machine.document()).expect("a document always serialises"));
    if settings.network {
        eprintln!("(boot adds a network card here, plugged into an endpoint it makes then.)");
    }
    if let Some(card) = chosen_card(&settings.gpu)? {
        println!("\nThen, once the machine runs:");
        let change = ModifySettingRequest::gpu(&Gpu::list([card.interface_path.as_str()]));
        println!("{}", serde_json::to_string_pretty(&change).expect("a change always serialises"));
    }
    Ok(())
}
