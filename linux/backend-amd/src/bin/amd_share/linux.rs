//! Stand-in clients on the card's DMA engine, to see how amdgpu's scheduler shares it.
//!
//! Heavy clients copy many MiB in each job and keep several jobs on the card, as a GPU-PV guest
//! does. A light client copies a little and keeps one job on the card, as a small app does. Each
//! client opens the render node itself, so each is its own entity in the kernel's scheduler. Every
//! job notes the card's clock when it starts and ends, so the report gives each client's share of
//! the engine's time. `--yield` has the heavy clients give way to others the way the daemon has a
//! guest do it, with the same `Share`, polling as the daemon polls a held submission.
//!
//! `--sibling-hz N` adds a small client that is the first heavy one's own, as a guest's compositor
//! is a game's: its jobs count as the guest's work, through the same `OwnEnds`. Each client waits
//! for its own jobs, so a sibling's end is noted a moment after it, where the daemon reads it from
//! the card's times. The heavy client's next job ends long after, so this doesn't show.
//! `--sibling-foreign` has the sibling count as someone else's, as 0.7.6 took a guest's other
//! processes.

use guesthouse_backend_amd::{
    Context, CpuAccess, DOMAIN_GTT, Device, HW_IP_DMA, Ib, MapFlags, MemoryType, OwnEnds, RunTimes, Share,
    Syncobj, close_buffer, create_buffer, map_buffer, submit_all, wait_eventfd,
};
use std::collections::VecDeque;
use std::process::ExitCode;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const OPCODE_COPY: u32 = 0x1;
const OPCODE_TIMESTAMP: u32 = 13;
const SUB_OPCODE_GLOBAL_TIMESTAMP: u32 = 2 << 8;
const MOST_LINEAR_BYTES: u64 = 0x3f_ff00;
const SLOTS: usize = 64;
const IB_SLOT_BYTES: u64 = 0x4000;
const PAGE: u64 = 4096;
/// How often the daemon looks again at a submission it holds.
const POLL: Duration = Duration::from_micros(20);

#[derive(Clone, Copy)]
struct Settings {
    bytes: u64,
    /// Jobs a second at most, or 0 for as many as it can.
    hz: u64,
    depth: usize,
    yields: bool,
}

#[derive(Default)]
struct Report {
    jobs: u64,
    busy_ns: u64,
    waited_ns: u64,
    paused_ns: u64,
    /// Runs that retired while the client was giving way.
    giving_way: u64,
    waits: Vec<u64>,
}

/// The sibling's settings, and whether it is to count as someone else, as 0.7.6 took it.
type Sibling = Option<(Settings, bool)>;

fn arguments() -> (usize, Settings, Settings, Sibling, Duration) {
    let mut heavy = 2;
    let mut heavy_settings = Settings { bytes: 64 << 20, hz: 0, depth: 4, yields: false };
    let mut light = Settings { bytes: 256 << 10, hz: 0, depth: 1, yields: false };
    let mut sibling = Settings { bytes: 256 << 10, hz: 0, depth: 1, yields: false };
    let mut foreign = false;
    let mut seconds = 5;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut number = || args.next().and_then(|value| value.parse::<u64>().ok()).unwrap_or(0);
        match arg.as_str() {
            "--heavy" => heavy = number() as usize,
            "--heavy-mb" => heavy_settings.bytes = number() << 20,
            "--depth" => heavy_settings.depth = (number() as usize).clamp(1, SLOTS),
            "--light-kb" => light.bytes = number() << 10,
            "--light-hz" => light.hz = number(),
            "--sibling-kb" => sibling.bytes = number() << 10,
            "--sibling-hz" => sibling.hz = number(),
            "--sibling-foreign" => foreign = true,
            "--seconds" => seconds = number(),
            "--yield" => heavy_settings.yields = true,
            other => eprintln!("ignored {other}"),
        }
    }
    let sibling = (sibling.hz > 0).then_some((sibling, foreign));
    (heavy, heavy_settings, light, sibling, Duration::from_secs(seconds))
}

fn ticks_to_ns(ticks: u64, freq_khz: u32) -> u64 {
    (u128::from(ticks) * 1_000_000 / u128::from(freq_khz.max(1))) as u64
}

/// One client's job: the start time, the copy, the end time.
fn job(source: u64, target: u64, bytes: u64, stamps: u64) -> Vec<u32> {
    let mut words =
        vec![OPCODE_TIMESTAMP | SUB_OPCODE_GLOBAL_TIMESTAMP, stamps as u32, (stamps >> 32) as u32];
    let mut done = 0;
    while done < bytes {
        let piece = (bytes - done).min(MOST_LINEAR_BYTES);
        let (from, to) = (source + done, target + done);
        words.extend([
            OPCODE_COPY,
            (piece - 1) as u32,
            0,
            from as u32,
            (from >> 32) as u32,
            to as u32,
            (to >> 32) as u32,
        ]);
        done += piece;
    }
    let end = stamps + 8;
    words.extend([OPCODE_TIMESTAMP | SUB_OPCODE_GLOBAL_TIMESTAMP, end as u32, (end >> 32) as u32]);
    while !words.len().is_multiple_of(8) {
        words.push(0);
    }
    words
}

struct OnCard {
    slot: usize,
    done: Syncobj,
    submitted_ns: u64,
}

/// The work of one guest's clients, when a client has a sibling.
type Guest = Arc<Mutex<OwnEnds>>;

fn client(
    name: String,
    settings: Settings,
    run_for: Duration,
    start: Arc<Barrier>,
    guest: Option<Guest>,
) -> Report {
    let device = Device::open(None).expect("no AMD render node");
    let info = device.info().clone();
    let align = info.va_alignment.max(PAGE);
    let rwx =
        MapFlags { readable: true, writable: true, executable: true, memory_type: MemoryType::Uncached };
    let size = settings.bytes.next_multiple_of(align);
    let ib_size = (SLOTS as u64 * IB_SLOT_BYTES).next_multiple_of(align);
    let stamp_size = (SLOTS as u64 * 16).next_multiple_of(align);
    let make = |bytes| create_buffer(&device, bytes, align, DOMAIN_GTT, CpuAccess::Required).unwrap();
    let (source, target, ibs, stamps) = (make(size), make(size), make(ib_size), make(stamp_size));
    let base = info.va_start + 0x100_0000;
    let (source_va, target_va) = (base, base + size);
    let ib_va = target_va + size;
    let stamp_va = ib_va + ib_size;
    map_buffer(&device, source, source_va, 0, size, rwx).unwrap();
    map_buffer(&device, target, target_va, 0, size, rwx).unwrap();
    map_buffer(&device, ibs, ib_va, 0, ib_size, rwx).unwrap();
    map_buffer(&device, stamps, stamp_va, 0, stamp_size, rwx).unwrap();
    let mut ib_cpu = device.map_cpu(ibs, ib_size as usize).unwrap();
    let stamp_cpu = device.map_cpu(stamps, stamp_size as usize).unwrap();
    let lengths: Vec<u32> = (0..SLOTS)
        .map(|slot| {
            let words = job(source_va, target_va, settings.bytes, stamp_va + slot as u64 * 16);
            assert!((words.len() * 4) as u64 <= IB_SLOT_BYTES, "a job's commands fit its slot");
            for (index, word) in words.iter().enumerate() {
                ib_cpu.write_u32(slot * IB_SLOT_BYTES as usize + index * 4, *word);
            }
            (words.len() * 4) as u32
        })
        .collect();
    let read_stamp = |slot: usize, which: usize| {
        let at = slot * 16 + which * 8;
        let ticks = u64::from(stamp_cpu.read_u32(at)) | u64::from(stamp_cpu.read_u32(at + 4)) << 32;
        ticks_to_ns(ticks, info.counter_freq_khz)
    };
    let context = Context::create(&device).unwrap();
    let handles = [source, target, ibs, stamps];
    let mut report = Report::default();
    let mut on_card: VecDeque<OnCard> = VecDeque::new();
    let mut next_slot = 0;
    let mut last_end = 0u64;
    let mut share = Share::default();
    start.wait();
    let began = Instant::now();
    while began.elapsed() < run_for {
        loop {
            let now = device.gpu_clock().unwrap();
            if on_card.len() >= settings.depth {
                break;
            }
            if settings.yields && !share.may_send(now, on_card.len()) {
                if !on_card.is_empty() {
                    break;
                }
                let paused = Instant::now();
                thread::sleep(POLL);
                report.paused_ns += paused.elapsed().as_nanos() as u64;
                continue;
            }
            let slot = next_slot;
            next_slot = (next_slot + 1) % SLOTS;
            let done = Syncobj::create(&device).unwrap();
            let ib = Ib { address: ib_va + slot as u64 * IB_SLOT_BYTES, bytes: lengths[slot] };
            submit_all(&device, &context, HW_IP_DMA, &[ib], &handles, None, &done).unwrap();
            let submitted_ns = device.gpu_clock().unwrap();
            on_card.push_back(OnCard { slot, done, submitted_ns });
        }
        let oldest = on_card.pop_front().unwrap();
        let event = oldest.done.arm_eventfd(&device).unwrap();
        assert!(wait_eventfd(&event, 5000).unwrap(), "{name}: a job didn't finish");
        oldest.done.destroy(&device);
        let (started, ended) = (read_stamp(oldest.slot, 0), read_stamp(oldest.slot, 1));
        let took = ended.saturating_sub(started);
        // What ran on the engine between this client's sending the job, or its last job's end, and
        // this job's start, was someone else's.
        let waited = started.saturating_sub(oldest.submitted_ns.max(last_end));
        let mut own_end = last_end;
        if let Some(guest) = &guest {
            let mut guest = guest.lock().unwrap();
            own_end = own_end.max(guest.last_by(HW_IP_DMA, started));
            guest.noted(HW_IP_DMA, ended);
        }
        last_end = ended;
        report.jobs += 1;
        report.busy_ns += took;
        report.waited_ns += waited;
        report.waits.push(waited);
        share.retired(RunTimes { sent: oldest.submitted_ns, started, ended }, own_end);
        report.giving_way += u64::from(share.contended(ended));
        // Paced after its job is in, so a sibling's end is noted as soon as it can be.
        if settings.hz > 0 {
            let due = began + Duration::from_secs_f64(report.jobs as f64 / settings.hz as f64);
            if let Some(left) = due.checked_duration_since(Instant::now()) {
                thread::sleep(left);
            }
        }
    }
    for left in on_card {
        let event = left.done.arm_eventfd(&device).unwrap();
        let _ = wait_eventfd(&event, 5000);
        left.done.destroy(&device);
    }
    context.destroy(&device);
    for handle in handles {
        close_buffer(&device, handle);
    }
    report
}

fn run(
    heavy: usize,
    heavy_settings: Settings,
    light: Option<Settings>,
    sibling: Sibling,
    run_for: Duration,
) -> Vec<(String, Report)> {
    let guest: Option<Guest> = sibling.map(|_| Arc::default());
    let heavy_guest = guest.clone().filter(|_| !sibling.is_some_and(|(_, foreign)| foreign));
    let mut clients: Vec<(String, Settings, Option<Guest>)> = (0..heavy)
        .map(|index| {
            let own = if index == 0 { heavy_guest.clone() } else { None };
            (format!("heavy {}", index + 1), heavy_settings, own)
        })
        .collect();
    if let Some((sibling, _)) = sibling {
        clients.push(("sibling".to_string(), sibling, guest.clone()));
    }
    if let Some(light) = light {
        clients.push(("light".to_string(), light, None));
    }
    let start = Arc::new(Barrier::new(clients.len()));
    let threads: Vec<_> = clients
        .into_iter()
        .map(|(name, settings, guest)| {
            let start = Arc::clone(&start);
            let label = name.clone();
            (label, thread::spawn(move || client(name, settings, run_for, start, guest)))
        })
        .collect();
    threads.into_iter().map(|(name, thread)| (name, thread.join().unwrap())).collect()
}

fn print(title: &str, results: &[(String, Report)], run_for: Duration) {
    println!("{title}");
    let total = run_for.as_nanos() as f64;
    for (name, report) in results {
        let mean_wait =
            if report.jobs == 0 { 0.0 } else { report.waited_ns as f64 / report.jobs as f64 / 1e6 };
        println!(
            "  {name:8} {:7.1} jobs/s  {:5.1} % of the engine  waited {mean_wait:6.2} ms a job  paused {:5.1} %  \
             giving way {:5.1} %",
            report.jobs as f64 / run_for.as_secs_f64(),
            report.busy_ns as f64 / total * 100.0,
            report.paused_ns as f64 / total * 100.0,
            report.giving_way as f64 / report.jobs.max(1) as f64 * 100.0,
        );
        let mut waits = report.waits.clone();
        waits.sort_unstable();
        let at =
            |part: f64| waits.get(((waits.len() as f64 - 1.0) * part) as usize).copied().unwrap_or(0) / 1000;
        println!("           waits in us: p10 {} p50 {} p90 {} p99 {}", at(0.1), at(0.5), at(0.9), at(0.99));
    }
}

pub fn main() -> ExitCode {
    let (heavy, heavy_settings, light, sibling, run_for) = arguments();
    let Ok(device) = Device::open(None) else {
        eprintln!("no AMD render node");
        return ExitCode::FAILURE;
    };
    println!(
        "card {:#06x}, clock {} kHz, scheduler policy {}",
        device.info().device_id,
        device.info().counter_freq_khz,
        std::fs::read_to_string("/sys/module/gpu_sched/parameters/sched_policy").unwrap_or_default().trim()
    );
    drop(device);
    print("light alone", &run(0, heavy_settings, Some(light), None, run_for), run_for);
    print("one heavy alone", &run(1, heavy_settings, None, None, run_for), run_for);
    print(&format!("{heavy} heavy alone"), &run(heavy, heavy_settings, None, None, run_for), run_for);
    print(
        &format!("{heavy} heavy and light"),
        &run(heavy, heavy_settings, Some(light), None, run_for),
        run_for,
    );
    if heavy > 1 {
        print("one heavy and light", &run(1, heavy_settings, Some(light), None, run_for), run_for);
    }
    if sibling.is_some() {
        print("one heavy and its sibling", &run(1, heavy_settings, None, sibling, run_for), run_for);
        let all = run(1, heavy_settings, Some(light), sibling, run_for);
        print("one heavy, its sibling and light", &all, run_for);
    }
    ExitCode::SUCCESS
}
