//! Live readings from the host's card, for the guest's performance queries.
//!
//! A reading can take many milliseconds, and the message path must never wait for one. So the card's
//! sensors publisher, a process of its own that serves no guest and runs in no jail, reads the card
//! on a schedule and publishes a snapshot in a small shared file, and the message path only looks at
//! the newest snapshot. A daemon opens that file for reading only. Each daemon on a card starts a
//! publisher for it, and the one holding the writer's turn reads the card. If its daemon goes, the
//! turn passes to the next.
//!
//! The readings come through the vendor's documented management library, which only the publisher
//! loads. Without it there are no snapshots and everything else works as before. The snapshot's file
//! is the `guesthouse-sensors` crate's, so the app can read the same figures.
//!
//! The publisher also reads each process's part of the card once a second, for the app's share
//! panel, and publishes it in a table beside the snapshot: one reading for the card, however many
//! guests are on it.

mod library;
mod processes;
mod sampler;

pub use guesthouse_sensors::{SharedSnapshot, Snapshot, SnapshotReader};
pub use processes::{ProcessMemory, ProcessSource, Sample, Samples, table};
pub use sampler::{FastReadings, Sampler, SensorSource, SlowReadings};

use crate::rm::listing::{place_for, read_listing};
use guesthouse_backend::PciAddress;
use guesthouse_sensors::card_processes::{SharedProcessTable, table_path_for};
use guesthouse_sensors::words::wait_for_writers_turn;
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

/// Reads the card and publishes, for as long as this process runs.
fn read_forever(snapshot: &SharedSnapshot, processes: Option<&SharedProcessTable>, source: library::Library) {
    let mut sampler = Sampler::new(source, Instant::now());
    let mut next_table = Instant::now();
    loop {
        let reading = sampler.step(Instant::now(), milliseconds_since_epoch());
        if let Some(reading) = reading {
            snapshot.publish(&reading);
        }
        if let Some(shared) = processes.filter(|_| Instant::now() >= next_table) {
            publish_table(sampler.source_mut(), shared);
            next_table = Instant::now() + TABLE_INTERVAL;
        }
        let table_due =
            processes.map_or(Duration::MAX, |_| next_table.saturating_duration_since(Instant::now()));
        thread::sleep(sampler.time_to_next(Instant::now()).min(table_due));
    }
}

/// The library for the card at `card`, a PCI address, for a program that reads each process's part
/// of the card once, as a check on a real card. `None` if it can't be loaded.
pub fn process_source(card: &str) -> Option<impl ProcessSource> {
    library::Library::open(library::LIBRARY_NAME, Some(card))
}

/// The snapshot the guests on the card at `card` share, opened for reading only. The file is made,
/// empty, before the daemon's jail, which grants it no more than reading.
pub fn open_readings(card: Option<&PciAddress>) -> io::Result<SharedSnapshot> {
    SharedSnapshot::open_to_read(&snapshot_path_for(card.map(PciAddress::to_string).as_deref()))
}

/// The publisher for the card at `card`, or with none the one setup chose: waits for the writer's
/// turn, then reads the card and publishes its snapshot and table of processes for as long as this
/// process runs. Returns only if the card's readings can't be had.
pub fn publish(card: Option<&PciAddress>) -> io::Result<()> {
    let key = card.map(PciAddress::to_string);
    let snapshot_path = snapshot_path_for(key.as_deref());
    let _turn = wait_for_writers_turn(&snapshot_path)?;
    let snapshot = SharedSnapshot::open(&snapshot_path)?;
    // Without it the guest's figures are as they were, and only the app's share panel goes without.
    let processes = SharedProcessTable::open(&table_path_for(key.as_deref()))
        .inspect_err(|error| log::info!("no table of the card's processes: {error}"))
        .ok();
    // The library finds the card by the address the driver lists it at, as the daemon opens it.
    let place = place_for(&read_listing(), card).map_err(io::Error::other)?;
    let address = place.address.map(|address| address.to_string());
    let Some(source) = library::Library::open(library::LIBRARY_NAME, address.as_deref()) else {
        return Err(io::Error::other("the management library isn't usable here"));
    };
    read_forever(&snapshot, processes.as_ref(), source);
    Ok(())
}
