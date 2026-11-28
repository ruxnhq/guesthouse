//! A machine's part of a card whose driver counts per process, as NVIDIA's does, from the table the
//! card's daemon publishes. A machine is its processes there: its daemon for a Windows guest, and its
//! QEMU, which holds a Linux guest's objects on the card through its forwarding device.

use super::look::GpuTime;
use guesthouse_sensors::card_processes::{Process, ProcessTable, SharedProcessTable, Use, table_path_for};
use std::collections::BTreeMap;

/// The driver's name for the shader cores, which draw and compute, and the names the panel already
/// knows the encoder and decoder by.
pub const SHADER_CORES: &str = "sm";
pub const ENCODER: &str = "enc";
pub const DECODER: &str = "dec";
const WHOLE: u32 = 100;

/// The table for the card at `card`, if its daemon has published one in the last few seconds.
pub fn read_table(card: Option<&str>) -> Option<ProcessTable> {
    SharedProcessTable::open_to_read(&table_path_for(card)).ok()?.latest()
}

/// What the table says of the processes `pids`: the time each engine spent on them, and the video
/// memory the driver charges them with. A process the table doesn't have adds nothing, and if it has
/// none of them, nothing is known of the machine.
pub fn machine_part(table: Option<&ProcessTable>, pids: &[u32]) -> (GpuTime, Option<u64>) {
    let Some(table) = table else { return (GpuTime::NotRead, None) };
    let mut wanted = pids.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    let found: Vec<&Process> = wanted.iter().filter_map(|pid| table.process(*pid)).collect();
    if found.is_empty() {
        return (GpuTime::NotRead, None);
    }
    let bytes = found.iter().filter_map(|process| process.bytes).reduce(u64::saturating_add);
    // A table that says its samples weren't read is not believed to hold any.
    let used: Vec<Use> =
        found.iter().filter_map(|process| process.used).filter(|_| table.samples_read).collect();
    // Work of two processes can overlap, and the sum can pass all of the time.
    let sum = |part: fn(&Use) -> u32| used.iter().map(part).sum::<u32>().min(WHOLE);
    let time = if !used.is_empty() {
        GpuTime::Engines(BTreeMap::from([
            (SHADER_CORES.to_owned(), sum(|used| used.sm)),
            (ENCODER.to_owned(), sum(|used| used.encoder)),
            (DECODER.to_owned(), sum(|used| used.decoder)),
        ]))
    } else if table.samples_read {
        GpuTime::NoneSeen
    } else {
        GpuTime::NotRead
    };
    (time, bytes)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const DAEMON: u32 = 2_462_537;
    pub const QEMU: u32 = 2_474_849;
    const MIB: u64 = 1 << 20;

    /// The table as a Windows guest's daemon was seen on an RTX 5090 under a light load: sm 12%,
    /// encoder and decoder idle, and 1848 MiB charged by the driver. `idle` is the same guest with
    /// nothing running, which the driver lists at 940 MiB with no sample.
    pub fn measured(idle: bool) -> ProcessTable {
        let used = (!idle).then_some(Use { sm: 12, encoder: 0, decoder: 0 });
        let bytes = if idle { 940 * MIB } else { 1848 * MIB };
        ProcessTable {
            taken_at: 0,
            samples_read: true,
            processes_read: true,
            left_out: false,
            processes: vec![
                Process { pid: DAEMON, used, bytes: Some(bytes) },
                Process {
                    pid: 56_207,
                    used: Some(Use { sm: 2, encoder: 0, decoder: 0 }),
                    bytes: Some(252 * MIB),
                },
            ],
        }
    }

    fn engines(sm: u32, encoder: u32, decoder: u32) -> GpuTime {
        GpuTime::Engines(BTreeMap::from([
            (SHADER_CORES.to_owned(), sm),
            (ENCODER.to_owned(), encoder),
            (DECODER.to_owned(), decoder),
        ]))
    }

    #[test]
    fn a_guest_under_load_has_its_sample_and_the_drivers_charge() {
        let (time, bytes) = machine_part(Some(&measured(false)), &[DAEMON, QEMU]);
        assert_eq!(time, engines(12, 0, 0));
        assert_eq!(bytes, Some(1848 * MIB), "a QEMU with nothing on the card adds nothing");
    }

    #[test]
    fn an_idle_guest_the_driver_lists_with_no_sample_used_none() {
        let (time, bytes) = machine_part(Some(&measured(true)), &[DAEMON, QEMU]);
        assert_eq!((time, bytes), (GpuTime::NoneSeen, Some(940 * MIB)));
    }

    #[test]
    fn with_no_samples_read_a_listed_guest_is_not_read() {
        let table = ProcessTable { samples_read: false, ..measured(true) };
        assert_eq!(machine_part(Some(&table), &[DAEMON]), (GpuTime::NotRead, Some(940 * MIB)));
    }

    #[test]
    fn a_machine_with_no_process_in_the_table_or_no_table_is_not_read() {
        assert_eq!(machine_part(Some(&measured(false)), &[QEMU]), (GpuTime::NotRead, None));
        assert_eq!(machine_part(None, &[DAEMON]), (GpuTime::NotRead, None));
    }

    #[test]
    fn a_machines_processes_add_up_to_a_whole_at_most() {
        let mut table = measured(false);
        table.processes.push(Process {
            pid: QEMU,
            used: Some(Use { sm: 95, encoder: 3, decoder: 1 }),
            bytes: Some(MIB),
        });
        let (time, bytes) = machine_part(Some(&table), &[DAEMON, QEMU, DAEMON]);
        assert_eq!(time, engines(100, 3, 1));
        assert_eq!(bytes, Some(1849 * MIB), "a pid named twice counts once");
    }
}
