//! What the log holds when xe ends a run early or bans a queue: which run it was, the batch it
//! ran, what moved in its address space meanwhile, and the faults xe saved for the VM. Each of
//! these is built as text first, so a test can read it, and logged at warning level, so it stays
//! when the log is cut down to warnings.
//!
//! What an empty fault list means is set out at [`crate::xe::device::Xe::vm_faults`].

use super::CardWork;
use super::queues::Run;
use crate::protocol::bytes::dword;
use guesthouse_backend::{HostServices, SpaceId};
use std::time::Instant;

/// The most bytes of a banned run's batch and the buffer it chains to that are written to the log
/// together.
pub(super) const MOST_BATCH_BYTES: u64 = 64 * 1024;

/// Dwords on a line of the dump, which is 64 bytes.
const DWORDS_PER_LINE: usize = 16;

/// Starts each line, as the other lines of this backend do.
const PREFIX: &str = "intel work:";

impl CardWork {
    /// The lines that say what became of a banned queue's run.
    pub(super) fn ban_lines(
        &mut self,
        host: &mut dyn HostServices,
        queue: u32,
        run: &Run,
        lost_with_it: usize,
        still_in_flight: usize,
        now: Instant,
    ) -> Vec<String> {
        self.ban_lines_showing(
            host,
            queue,
            run,
            (lost_with_it, still_in_flight),
            now,
            crate::watch::dumps_on(),
        )
    }

    /// [`Self::ban_lines`], with the batch's bytes shown only if `dumps`.
    fn ban_lines_showing(
        &mut self,
        host: &mut dyn HostServices,
        queue: u32,
        run: &Run,
        (lost_with_it, still_in_flight): (usize, usize),
        now: Instant,
        dumps: bool,
    ) -> Vec<String> {
        let Some(space) = self.queues.get(&queue).map(|queue| queue.space) else { return Vec::new() };
        let held_for = run.sent.map_or_else(
            || "for a time not known".to_string(),
            |sent| format!("for {} ms", now.saturating_duration_since(sent).as_millis()),
        );
        let which =
            if run.point == 0 { "its ring".to_string() } else { format!("run {} of the queue", run.point) };
        let mut lines = vec![format!(
            "{PREFIX} queue {queue:#x} in space {space:?} was banned by xe: {which}, which brings fence \
             {:#x} to value {:#x}, was in flight {held_for}; {lost_with_it} more run(s) were lost with it \
             and {still_in_flight} still wait behind it",
            run.fence, run.value
        )];
        lines.extend(self.batch_lines(host, space, run, dumps));
        lines.extend(self.change_lines(space, run));
        lines.extend(self.fault_lines(space));
        lines
    }

    /// What is logged when xe cancels a submission because its queue was banned or reset earlier.
    /// There is no run in flight to speak of, so only what is known without reading the batch.
    pub(super) fn explain_cancel(&mut self, queue: u32, space: SpaceId, attempted: Option<Run>) {
        let batch = attempted.map_or_else(String::new, |run| {
            format!(", sent with its batch at {:#x} for {:#x} bytes", run.batch, run.length)
        });
        log::warn!("{PREFIX} queue {queue:#x} in space {space:?}: xe cancelled a submission{batch}");
        for line in self.fault_lines(space) {
            log::warn!("{line}");
        }
    }

    /// The batch's dwords in hex, and the buffer it chains to, up to [`MOST_BATCH_BYTES`] in all.
    fn batch_lines(
        &mut self,
        host: &mut dyn HostServices,
        space: SpaceId,
        run: &Run,
        dumps: bool,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        if !dumps {
            lines.push(format!(
                "{PREFIX}   the batch at {:#x}, {:#x} bytes; its contents are not logged",
                run.batch, run.length
            ));
            if let Some(chain) = run.chain {
                lines.push(format!("{PREFIX}   it chains to {chain:#x}"));
            }
            return lines;
        }
        let first = run.length.min(MOST_BATCH_BYTES);
        let cut = if first < run.length { format!(", the first {first:#x} of it") } else { String::new() };
        match self.read_gpu(host, space, run.batch, first) {
            Ok(bytes) => {
                lines
                    .push(format!("{PREFIX}   the batch at {:#x}, {:#x} bytes{cut}:", run.batch, run.length));
                lines.extend(dump(run.batch, &bytes));
            }
            Err(error) => lines.push(format!(
                "{PREFIX}   the batch at {:#x}, {:#x} bytes, can't be shown now: {error}",
                run.batch, run.length
            )),
        }
        let Some(chain) = run.chain else { return lines };
        let room = MOST_BATCH_BYTES - first;
        let mapped = self.spaces.get(&space).and_then(|held| held.mapped_from(chain));
        let length = mapped.map_or(0, |mapped| mapped.min(room)) & !3;
        if length == 0 {
            lines.push(format!(
                "{PREFIX}   it chains to {chain:#x}, which isn't shown: {}",
                if room == 0 { "the batch took all the room" } else { "nothing is mapped there" }
            ));
            return lines;
        }
        match self.read_gpu(host, space, chain, length) {
            Ok(mut bytes) => {
                // The chained buffer's length isn't known, so what is shown is what lies mapped,
                // without the zeros that fill the end of it.
                while bytes.len() >= 4 && bytes[bytes.len() - 4..] == [0; 4] {
                    bytes.truncate(bytes.len() - 4);
                }
                lines.push(format!(
                    "{PREFIX}   it chains to {chain:#x}, whose length isn't known; {:#x} bytes of what is \
                     mapped there, without zeros at the end:",
                    bytes.len()
                ));
                lines.extend(dump(chain, &bytes));
            }
            Err(error) => {
                lines.push(format!("{PREFIX}   it chains to {chain:#x}, which can't be shown: {error}"))
            }
        }
        lines
    }

    /// The changes to the space's mappings made since the run was sent.
    fn change_lines(&self, space: SpaceId, run: &Run) -> Vec<String> {
        let Some(held) = self.spaces.get(&space) else { return Vec::new() };
        let since: Vec<_> = held.changes.iter().filter(|change| change.number > run.changes_before).collect();
        let dropped = held.changes_made.saturating_sub(run.changes_before) > since.len() as u64;
        if since.is_empty() && !dropped {
            return vec![format!("{PREFIX}   no mapping of its space changed while the run was in flight")];
        }
        let mut lines = vec![format!(
            "{PREFIX}   mappings of its space that changed while the run was in flight ({} of them{}):",
            held.changes_made - run.changes_before,
            if dropped { ", the oldest left out" } else { "" }
        )];
        for change in since {
            lines.push(format!(
                "{PREFIX}     {} {:#x} for {:#x} bytes, {} {}",
                if change.now_mapped { "mapped" } else { "unmapped" },
                change.address,
                change.length,
                if change.now_mapped { "now backed by" } else { "was backed by" },
                change.backing
            ));
        }
        lines
    }

    /// What xe lists as faulted in each VM of a space.
    pub(super) fn fault_lines(&mut self, space: SpaceId) -> Vec<String> {
        let Some(held) = self.spaces.get(&space) else { return Vec::new() };
        let vms =
            [(held.vm, "its VM"), (held.ring.as_ref().map_or(0, |ring| ring.vm), "its long-running VM")];
        let mut lines = Vec::new();
        for (vm, what) in vms.into_iter().filter(|&(vm, _)| vm != 0) {
            match self.xe.vm_faults(vm) {
                Ok(faults) if faults.is_empty() => lines.push(format!(
                    "{PREFIX}   xe lists no fault for {what} (VM {vm}): a hang leaves none, and a card or \
                     kernel that can't report page faults leaves none either"
                )),
                Ok(faults) => {
                    lines
                        .push(format!("{PREFIX}   xe lists {} fault(s) for {what} (VM {vm}):", faults.len()));
                    lines.extend(faults.iter().map(|fault| format!("{PREFIX}     fault at {fault}")));
                }
                Err(error) => {
                    lines.push(format!(
                        "{PREFIX}   asking xe for the faults of {what} (VM {vm}) failed: {error}"
                    ));
                }
            }
        }
        lines
    }
}

/// Bytes as lines of hex dwords, each with the address it starts at. Bytes past the last whole
/// dword are left out.
fn dump(address: u64, bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(DWORDS_PER_LINE * 4)
        .enumerate()
        .map(|(index, chunk)| {
            let at = address + (index * DWORDS_PER_LINE * 4) as u64;
            let dwords: Vec<String> =
                (0..chunk.len() / 4).map(|n| format!("{:08x}", dword(chunk, n * 4).unwrap_or(0))).collect();
            format!("{PREFIX}     {at:#014x}: {}", dwords.join(" "))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::Stage;
    use crate::work::spaces;
    use crate::work::testing::{Rig, map_request};
    use crate::xe::batch;
    use crate::xe::fake::Call;
    use crate::xe::plan::BufferUse;
    use guesthouse_backend::{UnmapRequest, WorkStart};
    use std::sync::Mutex;

    /// Every warning logged by any test, with the thread it came from, which is the test's own.
    struct Warnings(Mutex<Vec<(std::thread::ThreadId, String)>>);

    static WARNINGS: Warnings = Warnings(Mutex::new(Vec::new()));

    impl log::Log for Warnings {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Warn
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                self.0.lock().unwrap().push((std::thread::current().id(), record.args().to_string()));
            }
        }

        fn flush(&self) {}
    }

    /// The warnings the calling test has logged so far.
    fn logged() -> Vec<String> {
        let me = std::thread::current().id();
        WARNINGS
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(thread, _)| *thread == me)
            .map(|(_, line)| line.clone())
            .collect()
    }

    fn start_logging() {
        let _ = log::set_logger(&WARNINGS);
        log::set_max_level(log::LevelFilter::Warn);
    }

    /// A rig whose one queue has a run in flight, held back, and that run.
    fn in_flight(rig: &mut Rig, dwords: &[u32], value: u64) -> Run {
        rig.write_batch(dwords);
        rig.xe.hold_runs();
        let work = rig.submission(1, value);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.work.ring_doorbell(doorbell).unwrap();
        rig.work.queues[&1].in_flight[0]
    }

    fn lines(rig: &mut Rig, run: &Run) -> Vec<String> {
        rig.work.ban_lines_showing(
            &mut rig.host,
            1,
            run,
            (0, 0),
            Instant::now() + std::time::Duration::from_millis(250),
            true,
        )
    }

    /// The dwords of each dump line among `said`.
    fn dumped(said: &[String]) -> Vec<&str> {
        said.iter()
            .filter_map(|line| line.strip_prefix(PREFIX))
            .map(str::trim)
            .filter(|line| line.starts_with("0x"))
            .filter_map(|line| line.split_once(": ").map(|(_, dwords)| dwords))
            .collect()
    }

    fn faulting_batch() -> Vec<u32> {
        let mut dwords = batch::store_qword(0x9_0000_0123, 1).to_vec();
        dwords.push(batch::BATCH_END);
        dwords
    }

    #[test]
    fn a_ban_does_not_write_the_batch_bytes_unless_dumps_are_on() {
        let mut rig = Rig::new(Stage::Everything);
        let run = in_flight(&mut rig, &faulting_batch(), 0x77);
        rig.xe.release_runs();
        let said = rig.work.ban_lines_showing(&mut rig.host, 1, &run, (0, 0), Instant::now(), false);
        assert!(said.iter().any(|line| line.contains("0x18 bytes")), "{said:?}");
        assert!(dumped(&said).is_empty(), "{said:?}");
        let batch_end = format!("{:08x}", batch::BATCH_END);
        assert!(said.iter().all(|line| !line.contains(&batch_end)), "{said:?}");
    }

    #[test]
    fn a_ban_names_the_run_the_batch_and_how_long_it_was_in_flight() {
        let mut rig = Rig::new(Stage::Everything);
        let dwords = faulting_batch();
        let run = in_flight(&mut rig, &dwords, 0x77);
        rig.xe.release_runs();
        let said = lines(&mut rig, &run);
        assert!(said[0].contains("run 1 of the queue") && said[0].contains("value 0x77"), "{said:?}");
        assert!(said[0].contains("in flight for 2") || said[0].contains("in flight for 3"), "{said:?}");
        let at = rig.batch_address();
        assert!(said[1].contains(&format!("the batch at {at:#x}, 0x18 bytes")), "{said:?}");
        let shown = said[2].strip_prefix(PREFIX).unwrap().trim();
        let expected: Vec<String> = dwords.iter().map(|dword| format!("{dword:08x}")).collect();
        assert!(shown.ends_with(&expected.join(" ")), "{shown}");
        assert!(shown.starts_with(&format!("{at:#014x}:")), "{shown}");
        assert_eq!(said.iter().filter(|line| line.contains("chains")).count(), 0);
    }

    #[test]
    fn a_fault_is_listed_with_its_address_and_a_hang_says_the_list_is_empty() {
        let mut rig = Rig::new(Stage::Everything);
        rig.xe.set_names_faults(true);
        let run = in_flight(&mut rig, &faulting_batch(), 1);
        rig.xe.release_runs();
        let said = lines(&mut rig, &run).join("\n");
        assert!(said.contains("xe lists 1 fault(s) for its VM"), "{said}");
        assert!(
            said.contains("fault at 0x900000000 (exact to 0x1000 bytes) on a read: nothing mapped"),
            "{said}"
        );
        // A hang: the same card, the same batch, but xe gives the run up instead.
        let mut rig = Rig::new(Stage::Everything);
        rig.xe.set_names_faults(true);
        let run = in_flight(&mut rig, &faulting_batch(), 2);
        rig.xe.lose_held_runs();
        let said = lines(&mut rig, &run).join("\n");
        assert!(
            said.contains("xe lists no fault for its VM") && said.contains("a hang leaves none"),
            "{said}"
        );
        // A card that can't say gives the same answer for a fault.
        let mut rig = Rig::new(Stage::Everything);
        let run = in_flight(&mut rig, &faulting_batch(), 3);
        rig.xe.release_runs();
        assert!(lines(&mut rig, &run).join("\n").contains("xe lists no fault"));
        // And a refused question is said so.
        rig.xe.fail("vm_faults", libc::EINVAL);
        assert!(lines(&mut rig, &run).join("\n").contains("failed:"));
    }

    #[test]
    fn a_batch_that_chains_shows_the_buffer_it_goes_on_in_without_its_zero_fill() {
        let mut rig = Rig::new(Stage::Everything);
        let target = rig.data_address() + 0x800;
        let tail = [batch::store_qword(0x9_0000_0000, 5).as_slice(), &[batch::BATCH_END]].concat();
        rig.write_data(0x800, &batch::bytes(&tail));
        let run = in_flight(&mut rig, &batch::batch_start(target), 4);
        assert_eq!(run.chain, Some(target));
        let said = lines(&mut rig, &run);
        let chained =
            said.iter().position(|line| line.contains(&format!("it chains to {target:#x}"))).unwrap();
        let shown = said[chained + 1].trim();
        let expected: Vec<String> = tail.iter().map(|dword| format!("{dword:08x}")).collect();
        assert!(shown.ends_with(&expected.join(" ")), "{shown}");
        assert!(!shown.ends_with("00000000"), "the zeros at the end are cut: {shown}");
        assert!(said[chained].contains("length isn't known"), "{said:?}");
    }

    #[test]
    fn a_chain_into_nothing_is_said_and_a_batch_that_has_gone_is_said() {
        let mut rig = Rig::new(Stage::Everything);
        let run = in_flight(&mut rig, &batch::batch_start(0x9_0000_0000), 5);
        let said = lines(&mut rig, &run).join("\n");
        assert!(said.contains("which isn't shown: nothing is mapped there"), "{said}");
        let gone = Run { batch: 0x8_0000_0000, chain: None, ..run };
        let said = lines(&mut rig, &gone).join("\n");
        assert!(said.contains("can't be shown now: it isn't inside one mapping"), "{said}");
    }

    #[test]
    fn at_most_sixty_four_kib_of_batch_and_chain_are_shown_together() {
        let mut rig = Rig::new(Stage::Everything);
        let (big, _) = rig.work.allocate(0x2_0000, BufferUse::Card).unwrap();
        let address = 0x2_0000_0000;
        rig.work.map(&map_request(rig.space, big, address, 0, 0x2_0000)).unwrap();
        let junk: Vec<u8> = (0..0x2_0000u32).map(|n| (n % 251 + 1) as u8).collect();
        rig.work.write_for_test(big, &junk);
        let run = in_flight(&mut rig, &[batch::BATCH_END], 6);
        // A batch of 0x18000 bytes, which is more than the cap: its first 64 KiB, nothing chained.
        let long = Run { batch: address, length: 0x1_8000, chain: Some(address), ..run };
        let said = lines(&mut rig, &long);
        assert_eq!(dumped(&said).len(), 0x1_0000 / 64);
        assert!(said[1].contains("the first 0x10000 of it"), "{}", said[1]);
        assert!(said.iter().any(|line| line.contains("the batch took all the room")));
        // A short batch leaves the rest of the room to the chain.
        let short = Run { batch: address, length: 0x1000, chain: Some(address + 0x1000), ..run };
        let said = lines(&mut rig, &short);
        let bytes: usize = dumped(&said).iter().map(|dwords| dwords.split(' ').count() * 4).sum();
        assert_eq!(bytes, 0x1_0000, "the 4 KiB batch and 60 KiB of what it chains to");
    }

    #[test]
    fn mappings_that_moved_while_the_run_was_in_flight_are_listed_and_older_ones_are_not() {
        let mut rig = Rig::new(Stage::Everything);
        // A change before the run is sent is not the run's.
        let (memory, address, length) = (rig.data_memory(), rig.data_address(), rig.data_bytes());
        let space = rig.space;
        rig.work.unmap(&UnmapRequest { space, memory, address, length }).unwrap();
        rig.work.map(&map_request(space, memory, address, 0, length)).unwrap();
        let run = in_flight(&mut rig, &faulting_batch(), 7);
        let said = lines(&mut rig, &run).join("\n");
        assert!(said.contains("no mapping of its space changed"), "{said}");
        rig.work.unmap(&UnmapRequest { space, memory, address, length }).unwrap();
        rig.work.map(&map_request(space, memory, address + 0x10_0000, 0, length)).unwrap();
        let said = lines(&mut rig, &run).join("\n");
        assert!(said.contains("(2 of them)"), "{said}");
        assert!(
            said.contains(&format!(
                "unmapped {address:#x} for {length:#x} bytes, was backed by the guest's own pages"
            )),
            "{said}"
        );
        assert!(
            said.contains(&format!(
                "mapped {:#x} for {length:#x} bytes, now backed by the guest's own pages",
                address + 0x10_0000
            )),
            "{said}"
        );
    }

    #[test]
    fn the_changes_kept_are_the_latest_and_a_run_is_told_when_older_ones_were_left_out() {
        let mut rig = Rig::new(Stage::Everything);
        let run = in_flight(&mut rig, &faulting_batch(), 8);
        let (memory, space) = (rig.data_memory(), rig.space);
        for round in 0..spaces::CHANGES_KEPT as u64 {
            let at = 0x20_0000_0000 + round * 0x1_0000;
            rig.work.map(&map_request(space, memory, at, 0, 0x1000)).unwrap();
            rig.work.unmap(&UnmapRequest { space, memory, address: at, length: 0x1000 }).unwrap();
        }
        let said = lines(&mut rig, &run);
        assert!(said.iter().any(|line| line.contains("the oldest left out")), "{said:?}");
        let listed = said.iter().filter(|line| line.contains("mapped 0x2")).count();
        assert_eq!(listed, spaces::CHANGES_KEPT);
    }

    #[test]
    fn a_ban_found_by_a_poll_is_logged_in_full_at_warning_level() {
        start_logging();
        let mut rig = Rig::new(Stage::Everything);
        rig.xe.set_names_faults(true);
        rig.write_batch(&faulting_batch());
        let work = rig.submission(1, 0xBA11_0001);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.work.ring_doorbell(doorbell).unwrap();
        rig.poll();
        assert!(rig.work.space_failed(rig.space));
        let said = logged().join("\n");
        assert!(said.contains("was banned by xe") && said.contains("run 1 of the queue"), "{said}");
        let at = rig.batch_address();
        assert!(said.contains(&format!("the batch at {at:#x}")), "{said}");
    }

    #[test]
    fn a_banned_ring_is_logged_with_its_batch() {
        start_logging();
        let mut rig = Rig::new(Stage::Everything);
        let mut dwords = batch::store_qword(rig.data_address() + 0x40, 0xBEEF).to_vec();
        dwords.extend([batch::SEMAPHORE_WAIT_HEADER | 1 << 15 | 1 << 12, 1, 0x4000_1000, 0, 0]);
        rig.write_batch(&dwords);
        let work = rig.submission(1, 0xBA11_0002);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.work.ring_doorbell(doorbell).unwrap();
        let Some(Call::Exec { queue: ring, .. }) = rig.xe.calls_named("exec").pop() else { panic!() };
        rig.xe.ban(ring);
        rig.poll();
        rig.poll();
        let said = logged().join("\n");
        assert!(said.contains("its ring") && said.contains("the batch at"), "{said}");
        assert_eq!(said.matches("was banned by xe").count(), 1, "said once, not at every look");
        assert!(rig.host.written.len() == 1);
    }

    #[test]
    fn a_cancelled_submission_logs_the_faults_of_its_space() {
        start_logging();
        let mut rig = Rig::new(Stage::Everything);
        rig.xe.set_names_faults(true);
        let mut dwords = faulting_batch();
        dwords.extend([batch::NOOP; 3]);
        rig.write_batch(&dwords);
        let work = rig.submission(1, 0xBA11_0003);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.xe.fail("exec", libc::ECANCELED);
        assert!(rig.work.ring_doorbell(doorbell).is_err());
        let said = logged().join("\n");
        assert!(said.contains("xe cancelled a submission, sent with its batch at"), "{said}");
    }

    #[test]
    fn a_dump_has_sixteen_dwords_to_a_line_and_leaves_out_a_part_of_one() {
        let bytes: Vec<u8> = (0u8..70).collect();
        let lines = dump(0x1000, &bytes);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("03020100 07060504 0b0a0908 0f0e0d0c 13121110 17161514 1b1a1918 1f1e1d1c 23222120 27262524 2b2a2928 2f2e2d2c 33323130 37363534 3b3a3938 3f3e3d3c"), "{}", lines[0]);
        assert!(lines[1].ends_with("0x000000001040: 43424140"), "{}", lines[1]);
        assert!(!lines[1].contains("4b4a"), "bytes past the last whole dword are left out");
    }
}
