//! Boots the machine: the card's driver onto its disk, or onto a Linux guest's files disk, a network
//! endpoint, the machine created and started through HCS, and then its GPU, in that order, as HCS
//! gives a GPU only to a running machine. It stays until the machine stops, as the machine stops
//! when its last handle closes, or until its time is up.

use super::{absolute, chosen_card, copy_driver_to_disk, linux, machine, need_elevation, print_guest_report};
use crate::Options;
use crate::guest::linux::install_finished;
use crate::machine::Guest;
use crate::printable::printable;
use crate::record::{Record, files_disk};
use guesthouse_hcs::host;
use guesthouse_hcs::network::{self, Endpoint, GUID};
use guesthouse_hcs::schema::{Gpu, ModifySettingRequest, NetworkAdapter};
use guesthouse_hcs::system::{self, ComputeSystem};
use guesthouse_jail::windows::{Handles, Jail, Jailed};
use std::fs::File;
use std::io::{BufRead, BufReader, PipeReader};
use std::os::windows::io::OwnedHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

/// Guesthouse's own network in HCN, which every machine shares and which outlives a run.
const NETWORK_ID: GUID = GUID::from_u128(0x9f3a6c2e_5b1d_4e8a_a7c4_2d6e8b0f1a35);
const NETWORK_NAME: &str = "Guesthouse";
/// How often the machine's state is read while it runs.
const POLL: Duration = Duration::from_secs(1);
/// How long the guest's serial port may take to appear once the machine starts.
const SERIAL_SECONDS: u64 = 30;
/// The jailed reader of the serial port, beside this program.
const RELAY: &str = "guesthouse-serial-relay.exe";
/// Its AppContainer, which Windows keeps between runs.
const RELAY_PROFILE: &str = "guesthouse.serial-relay";
/// While the machine's state can't be read, the reason is told once in this many tries.
const UNREADABLE_REPEAT: u32 = 60;
/// How long a guest asked to shut down at the end of its time has before it's stopped.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(180);
/// Ubuntu's installer speaks on the serial port soon after it starts, but only when the files
/// disk's own menu started it. An install boot this quiet for this long started something else.
const INSTALL_SILENCE: Duration = Duration::from_secs(300);

pub fn boot(options: &Options) -> Result<(), String> {
    need_elevation()?;
    let disk = absolute(options.disk.as_deref().expect("parse asks for a disk"))?;
    if !disk.is_file() {
        return Err(format!("There is no disk at {}. prepare makes one.", disk.display()));
    }
    let card = chosen_card(options)?;
    // A machine left from an earlier run still has the disk open, so it goes before the disk is
    // attached for the driver.
    if let Some(earlier) = ComputeSystem::open(&options.name) {
        println!("A machine called {} is still there from an earlier run. Stopping it.", options.name);
        if let Err(error) = earlier.terminate() {
            eprintln!("{error}");
        }
    }
    let record = Record::load(&disk)?;
    match record.guest {
        Guest::Windows => {
            if let Some(card) = &card {
                copy_driver_to_disk(&disk, card)?;
            }
        }
        Guest::Linux => {
            linux::refresh_disk(&files_disk(&disk), card.as_ref())?;
            if !record.installed {
                println!("This boot installs Ubuntu. The machine turns itself off when it's done.");
            }
        }
    }
    let stop = host::catch_ctrl_c().map_err(|error| error.to_string())?;

    let (_endpoint, adapter) = if options.network { plug_in()? } else { (None, None) };
    let machine = machine(options, disk, &record, adapter);
    system::ensure_guest_state_file(&machine.guest_state_path()).map_err(|error| error.to_string())?;
    system::ensure_runtime_state_file(&machine.runtime_state_path()).map_err(|error| error.to_string())?;
    let mut opened = vec![machine.disk.clone(), machine.guest_state_path(), machine.runtime_state_path()];
    opened.extend(machine.more_disks.iter().map(|disk| PathBuf::from(&disk.path)));
    for path in &opened {
        system::grant_vm_access(&machine.name, path).map_err(|error| error.to_string())?;
    }

    let document = machine.document();
    let running = ComputeSystem::create(&machine.name, &document).map_err(|error| {
        let sent = serde_json::to_string_pretty(&document).unwrap_or_default();
        format!("{error}\n\nThe document sent:\n{sent}")
    })?;
    running.start().map_err(|error| error.to_string())?;
    println!("{} started.", machine.name);

    if let Some(card) = &card {
        let change = ModifySettingRequest::gpu(&Gpu::list([card.interface_path.as_str()]));
        match running.modify(&change) {
            Ok(()) => println!("It has {} by GPU-PV.", card.name),
            Err(error) => eprintln!("It runs without its card, as HCS refused it: {error}"),
        }
    }

    let pipe = machine.serial_pipe();
    let unjailed = !options.jail;
    let disk = machine.disk.clone();
    let heard = Arc::new(AtomicBool::new(false));
    let heard_here = Arc::clone(&heard);
    let installing = record.guest == Guest::Linux && !record.installed;
    let guest = record.guest;
    // The guest's line saying its install is done is when the installer stops being attached.
    let on_line = move |line: &str| {
        heard_here.store(true, Ordering::SeqCst);
        if install_finished(line) && record.guest == Guest::Linux && !record.installed {
            let installed = Record { installed: true, ..record.clone() };
            match installed.save(&disk) {
                Ok(()) => println!("The install is done. The next boot starts the installed system."),
                Err(error) => eprintln!("{error}"),
            }
        }
    };
    thread::spawn(move || print_serial(&pipe, unjailed, on_line));
    println!("Waiting for the guest. Ctrl+C asks it to shut down.");
    let limit = options.for_minutes.map(|minutes| Duration::from_secs(minutes * 60));
    let silence = installing.then_some((INSTALL_SILENCE, heard));
    let waited = wait(&running, stop, limit, silence);
    // The report a Windows guest writes on its own disk can be read once the machine has let go.
    drop(running);
    if guest == Guest::Windows {
        print_guest_report(&machine.disk);
    }
    waited
}

/// An endpoint on Guesthouse's network, deleted when dropped, and the card that plugs into it.
fn plug_in() -> Result<(Option<Endpoint>, Option<NetworkAdapter>), String> {
    network::ensure_ics_network(&NETWORK_ID, NETWORK_NAME).map_err(|error| error.to_string())?;
    let id = host::new_guid().map_err(|error| error.to_string())?;
    let mac = network::mac_from(&id);
    let endpoint = Endpoint::create(&NETWORK_ID, id, &mac).map_err(|error| error.to_string())?;
    let adapter = NetworkAdapter { endpoint_id: endpoint.id_string(), mac_address: mac };
    Ok((Some(endpoint), Some(adapter)))
}

/// Waits for the machine to stop. Returning closes the last handle, which stops the machine, so only
/// an answer that says it has stopped, or a second Ctrl+C, ends this: an answer that can't be read
/// is told and waited past.
fn wait(
    running: &ComputeSystem,
    stop: &AtomicBool,
    limit: Option<Duration>,
    silence: Option<(Duration, Arc<AtomicBool>)>,
) -> Result<(), String> {
    let started = Instant::now();
    let mut asked = false;
    let mut first_answer = true;
    let mut unreadable = 0u32;
    loop {
        thread::sleep(POLL);
        // When its time is up the guest is asked to shut down, as Ctrl+C would, and stopped if it
        // hasn't after a grace.
        if let Some(limit) = limit {
            let elapsed = started.elapsed();
            if (!asked && elapsed >= limit) || (asked && elapsed >= limit + SHUTDOWN_GRACE) {
                println!("Its time is up.");
                stop.store(true, Ordering::SeqCst);
            }
        }
        if let Some((quiet, heard)) = &silence
            && !heard.load(Ordering::SeqCst)
            && started.elapsed() >= *quiet
        {
            println!(
                "The installer has said nothing on the serial port, which it does only when the files \
                 disk's menu starts it, so the firmware or GRUB started something else. Stopping it."
            );
            if let Err(error) = running.terminate() {
                eprintln!("{error}");
            }
            return Ok(());
        }
        if stop.swap(false, Ordering::SeqCst) {
            if asked {
                println!("Stopping it now.");
                if let Err(error) = running.terminate() {
                    eprintln!("{error}");
                }
                return Ok(());
            } else {
                asked = true;
                println!("Asking the guest to shut down. Ctrl+C again stops it at once.");
                // The call waits for the guest, so it has a handle and a thread of its own.
                let name = running.id().to_owned();
                thread::spawn(move || {
                    if let Some(machine) = ComputeSystem::open(&name)
                        && let Err(error) = machine.shut_down()
                    {
                        eprintln!("{error}");
                    }
                });
            }
        }
        match running.properties() {
            Ok(properties) => {
                if first_answer {
                    println!("HCS says: {}", properties.raw);
                    first_answer = false;
                }
                unreadable = 0;
                if properties.stopped() {
                    println!("The machine stopped: {} {}", properties.state, properties.exit_type);
                    return Ok(());
                }
            }
            Err(error) => {
                if unreadable.is_multiple_of(UNREADABLE_REPEAT) {
                    eprintln!("The machine's state can't be read, so it's left running: {error}");
                }
                unreadable += 1;
            }
        }
    }
}

/// Prints each line the guest writes to COM1 until the port closes. The port is read by the relay
/// in its jail, as those are bytes the guest chose, unless `unjailed` says to read it here.
fn print_serial(pipe: &str, unjailed: bool, mut on_line: impl FnMut(&str)) {
    let started = Instant::now();
    let port = loop {
        match File::open(pipe) {
            Ok(port) => break port,
            Err(_) if started.elapsed() < Duration::from_secs(SERIAL_SECONDS) => thread::sleep(POLL),
            Err(error) => {
                eprintln!("The guest's serial port at {pipe} could not be opened: {error}");
                return;
            }
        }
    };
    if unjailed {
        println!("Reading the guest's serial port with no jail, as --no-jail asks.");
        return print_lines(port, &mut on_line);
    }
    match relay(port) {
        Ok((jailed, lines)) => {
            print_lines(lines, &mut on_line);
            let _ = jailed.wait();
        }
        Err(error) => eprintln!(
            "The guest's serial port is left unread, as its reader couldn't be jailed: {error}. \
             --no-jail reads it anyway."
        ),
    }
}

/// The serial relay, started in its jail with the port as its input, and the end of its output.
fn relay(port: File) -> std::io::Result<(Jailed, PipeReader)> {
    let program = std::env::current_exe()?.with_file_name(RELAY);
    let (lines, relay_output) = std::io::pipe()?;
    let handles = Handles {
        stdin: Some(OwnedHandle::from(port)),
        stderr: Some(OwnedHandle::from(relay_output.try_clone()?)),
        stdout: Some(OwnedHandle::from(relay_output)),
    };
    let jailed = Jail::new(RELAY_PROFILE).spawn(&program, &[], handles)?;
    Ok((jailed, lines))
}

/// Prints each line, made printable, as the guest's, and hands it to `on_line`.
fn print_lines(source: impl std::io::Read, on_line: &mut impl FnMut(&str)) {
    let mut source = BufReader::new(source);
    let mut line = Vec::new();
    while source.read_until(b'\n', &mut line).is_ok_and(|read| read > 0) {
        let text = printable(&line);
        println!("guest: {text}");
        on_line(&text);
        line.clear();
    }
}
