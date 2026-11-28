//! Live readings from the host's card, for the guest's performance queries.
//!
//! A reading can take many milliseconds, and the message path must never wait for one. So one thread
//! on the host reads the card on a schedule and publishes a snapshot, and the message path only
//! looks at the newest snapshot. Several daemons can run on one PC, so the snapshot lives in a
//! small shared file, and a lock on that file decides whose thread does the reading. If that daemon
//! goes, the lock passes to the next.
//!
//! The readings come through the vendor's documented management library, loaded when the daemon
//! starts. Without it there are no snapshots and everything else works as before. The snapshot's
//! file is the `guesthouse-sensors` crate's, so the app can read the same figures.
//!
//! The same thread also reads each process's part of the card once a second, for the app's share
//! panel, and publishes it in a table beside the snapshot: one reading for the card, however many
//! guests are on it, and the other daemons never load the library.

mod library;
mod processes;
mod sampler;

pub use guesthouse_sensors::{SharedSnapshot, Snapshot, SnapshotReader};
pub use processes::{ProcessMemory, ProcessSource, Sample, Samples, table};
pub use sampler::{FastReadings, Sampler, SensorSource, SlowReadings};

use guesthouse_backend::PciAddress;
use guesthouse_sensors::card_processes::{SharedProcessTable, table_path_for};
use guesthouse_sensors::{milliseconds_since_epoch, snapshot_path_for};
use std::io;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The driver samples each process once a second.
const TABLE_INTERVAL: Duration = Duration::from_secs(1);

fn microseconds_since_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_micros() as u64)
}

/// One look at each process on the card, published. The library hands back each process's newest
/// sample whatever is asked, so it is asked for all of them and the table keeps the fresh ones.
fn publish_table(source: &mut impl ProcessSource, shared: &SharedProcessTable) {
    let samples = source.samples_since(0);
    let processes = source.processes();
    shared.publish(&table(&samples, processes.as_deref(), microseconds_since_epoch()));
}

/// Reads and publishes for as long as this process holds the lock, which is until it exits.
fn read_forever(snapshot: SharedSnapshot, processes: Option<SharedProcessTable>, card_uuid: String) {
    if let Err(error) = snapshot.wait_for_lock() {
        log::warn!("no host sensors: {error}");
        return;
    }
    let Some(source) = library::Library::open(library::LIBRARY_NAME, &card_uuid) else {
        log::info!("no host sensors: the management library isn't usable here");
        return;
    };
    let mut sampler = Sampler::new(source, Instant::now());
    let mut next_table = Instant::now();
    loop {
        let reading = sampler.step(Instant::now(), milliseconds_since_epoch());
        if let Some(reading) = reading {
            snapshot.publish(&reading);
        }
        if let Some(shared) = processes.as_ref().filter(|_| Instant::now() >= next_table) {
            publish_table(sampler.source_mut(), shared);
            next_table = Instant::now() + TABLE_INTERVAL;
        }
        let table_due = processes
            .as_ref()
            .map_or(Duration::MAX, |_| next_table.saturating_duration_since(Instant::now()));
        thread::sleep(sampler.time_to_next(Instant::now()).min(table_due));
    }
}

/// The library for the card with this identity, in its text form, for a program that reads each
/// process's part of the card once, as a check on a real card. `None` if it can't be loaded.
pub fn process_source(card_uuid: &str) -> Option<impl ProcessSource> {
    library::Library::open(library::LIBRARY_NAME, card_uuid)
}

/// Opens the snapshot the guests on the card at `card` share, and starts this daemon's thread, which
/// waits its turn to read.
pub fn start(card_uuid: &[u8; 16], card: Option<&PciAddress>) -> io::Result<SharedSnapshot> {
    let snapshot = SharedSnapshot::open(&snapshot_path_for(card.map(PciAddress::to_string).as_deref()))?;
    let for_thread = snapshot.try_clone()?;
    // Opened here, before the jail's filter, as it maps the file. Without it the guest's figures
    // are as they were, and only the app's share panel goes without.
    let processes = SharedProcessTable::open(&table_path_for(card.map(PciAddress::to_string).as_deref()))
        .inspect_err(|error| log::info!("no table of the card's processes: {error}"))
        .ok();
    let card_uuid = library::uuid_text(card_uuid);
    thread::Builder::new()
        .name("host-sensors".to_owned())
        .spawn(|| read_forever(for_thread, processes, card_uuid))?;
    Ok(snapshot)
}
