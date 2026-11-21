//! Boots the machine: the card's driver onto its disk, a network endpoint, the machine created and
//! started through HCS, and then its GPU, in that order, as HCS gives a GPU only to a running
//! machine. It stays until the machine stops, as the machine stops when its last handle closes.

use super::{absolute, chosen_card, copy_driver_to_disk, machine, need_elevation};
use crate::Options;
use guesthouse_hcs::host;
use guesthouse_hcs::network::{self, Endpoint, GUID};
use guesthouse_hcs::schema::{Gpu, ModifySettingRequest, NetworkAdapter};
use guesthouse_hcs::system::{self, ComputeSystem};
use std::fs::File;
use std::io::{BufRead, BufReader};
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

pub fn boot(options: &Options) -> Result<(), String> {
    need_elevation()?;
    let disk = absolute(options.disk.as_deref().expect("parse asks for a disk"))?;
    if !disk.is_file() {
        return Err(format!("There is no disk at {}. prepare makes one.", disk.display()));
    }
    let card = chosen_card(options)?;
    if let Some(card) = &card {
        copy_driver_to_disk(&disk, card)?;
    }
    let stop = host::catch_ctrl_c().map_err(|error| error.to_string())?;

    let (_endpoint, adapter) = if options.network { plug_in()? } else { (None, None) };
    let machine = machine(options, disk, adapter);
    system::ensure_guest_state_file(&machine.guest_state_path()).map_err(|error| error.to_string())?;
    system::ensure_runtime_state_file(&machine.runtime_state_path()).map_err(|error| error.to_string())?;
    for path in [&machine.disk, &machine.guest_state_path(), &machine.runtime_state_path()] {
        system::grant_vm_access(&machine.name, path).map_err(|error| error.to_string())?;
    }

    if let Some(earlier) = ComputeSystem::open(&machine.name) {
        println!("A machine called {} is still there from an earlier run. Stopping it.", machine.name);
        let _ = earlier.terminate();
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
    thread::spawn(move || print_serial(&pipe));
    println!("Waiting for the guest. Ctrl+C asks it to shut down.");
    wait(&running, stop)
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

fn wait(running: &ComputeSystem, stop: &std::sync::atomic::AtomicBool) -> Result<(), String> {
    let mut asked = false;
    loop {
        thread::sleep(POLL);
        if stop.swap(false, Ordering::SeqCst) {
            if asked {
                println!("Stopping it now.");
                running.terminate().map_err(|error| error.to_string())?;
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
            Ok(properties) if properties.running() => {}
            Ok(properties) => {
                println!("The machine stopped: {} {}", properties.state, properties.exit_type);
                return Ok(());
            }
            // Once the machine has gone, HCS may no longer answer for it at all.
            Err(error) => {
                println!("The machine has gone: {error}");
                return Ok(());
            }
        }
    }
}

/// Prints each line the guest writes to COM1 until the port closes.
fn print_serial(pipe: &str) {
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
    let mut port = BufReader::new(port);
    let mut line = Vec::new();
    // Read as bytes, as a guest may write anything, and one bad line shouldn't end the rest.
    while port.read_until(b'\n', &mut line).is_ok_and(|read| read > 0) {
        println!("guest: {}", String::from_utf8_lossy(&line).trim_end());
        line.clear();
    }
}
