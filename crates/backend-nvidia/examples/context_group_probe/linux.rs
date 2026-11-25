use guesthouse_backend_nvidia::channel::{Channel, ChannelPlace, Engine, Seat};
use guesthouse_backend_nvidia::profile::{BLACKWELL, HostBranch, host_branch, host_branch_of_version};
use guesthouse_backend_nvidia::pushbuffer::{increasing, ring_entry};
use guesthouse_backend_nvidia::rm::device::{AddressSpace, Device};
use guesthouse_backend_nvidia::rm::linux::LinuxRm;
use std::time::{Duration, Instant};

const RANGE: (u64, u64) = (0x400_0000, 0x1_0000_0000);
const AREA_SIZE: usize = 0x20000;
const FIRST_AREA: u64 = 0x800_0000;
const RING_ENTRIES: u32 = 1024;
/// Past the control block and the prelude, which [`Channel`] keeps at 0x10000 and 0x11000.
const SEMAPHORES: usize = 0x13000;
const COMMANDS: usize = 0x14000;
const COMMAND_SLOT: usize = 0x40;
const CONTEXT: u32 = 0x4000_0040;

const HOST_SUBCHANNEL: u32 = 6;
const SEMAPHORE_METHODS: u32 = 0x5C;
const RELEASE: u32 = 1;
/// Wait until the value is there or past it, without letting the channel group go: the guest
/// driver's own wait.
const ACQUIRE: u32 = 3;
/// The same, giving the GPU to another group while it waits.
const ACQUIRE_SWITCHING_GROUP: u32 = ACQUIRE | 1 << 12;

struct Probed {
    name: &'static str,
    channel: Channel,
    put: u32,
    slots: usize,
}

impl Probed {
    fn semaphore(&self, index: usize) -> u64 {
        self.channel.area_address() + (SEMAPHORES + index * 4) as u64
    }

    fn seen(&self, index: usize) -> u32 {
        self.channel.area().load(SEMAPHORES + index * 4)
    }

    /// Adds one ring entry running `words`, and rings.
    fn run(&mut self, device: &mut Device<LinuxRm>, words: &[u32]) {
        let at = COMMANDS + self.slots * COMMAND_SLOT;
        assert!(words.len() * 4 <= COMMAND_SLOT && at + COMMAND_SLOT <= AREA_SIZE, "out of command room");
        self.slots += 1;
        self.channel.area().store_words(at, words);
        self.add(device, ring_entry(self.channel.area_address() + at as u64, words.len() as u32));
    }

    fn add(&mut self, device: &mut Device<LinuxRm>, entry: u64) {
        self.channel.area().store_u64(self.put as usize * 8, entry);
        self.put += 1;
        self.channel.advance(device, self.put);
    }
}

fn semaphore(address: u64, value: u32, operation: u32) -> [u32; 6] {
    [
        increasing(HOST_SUBCHANNEL, SEMAPHORE_METHODS, 5),
        address as u32,
        (address >> 32) as u32,
        value,
        0,
        operation,
    ]
}

fn wait_for(what: impl Fn() -> bool, limit: Duration) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if what() {
            return Some(start.elapsed());
        }
        std::hint::spin_loop();
    }
    None
}

fn make(
    device: &mut Device<LinuxRm>,
    sizes: &'static HostBranch,
    space: &AddressSpace,
    (grouped, compute_first): (bool, bool),
) -> Vec<Probed> {
    let mut seats = [
        ("drawing", Seat::Drawing),
        ("compute 1", Seat::Compute { index: 1, prefer_lower: false }),
        ("compute 2", Seat::Compute { index: 2, prefer_lower: true }),
    ];
    if compute_first {
        seats.rotate_left(1);
    }
    let mut made = Vec::new();
    for (slot, (name, seat)) in seats.into_iter().enumerate() {
        let area_address = FIRST_AREA + (slot * AREA_SIZE) as u64;
        let place = ChannelPlace {
            ring_address: area_address,
            ring_entries: RING_ENTRIES,
            area_address,
            area_size: AREA_SIZE,
        };
        let channel = if grouped {
            Channel::new_in_context(device, &BLACKWELL, sizes, space, place, CONTEXT, seat)
        } else {
            Channel::new(device, &BLACKWELL, sizes, space, place, Engine::Graphics)
        };
        let channel = channel.unwrap_or_else(|error| panic!("{name}: {error}"));
        println!(
            "{name}: channel {:#x}, token {:#x}, bound {:?}",
            channel.handle(),
            channel.token(),
            channel.only_bound()
        );
        made.push(Probed { name, channel, put: 0, slots: 0 });
    }
    made
}

pub fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let grouped = match mode.as_str() {
        "grouped" | "grouped-compute-first" => true,
        "alone" => false,
        _ => {
            eprintln!("usage: context_group_probe grouped|grouped-compute-first|alone [waits] [switching]");
            std::process::exit(2);
        }
    };
    let waits: u32 = std::env::args().nth(2).and_then(|count| count.parse().ok()).unwrap_or(200);
    let acquire = match std::env::args().nth(3).as_deref() {
        Some("switching") => ACQUIRE_SWITCHING_GROUP,
        _ => ACQUIRE,
    };
    let version = std::fs::read_to_string("/sys/module/nvidia/version").expect("the NVIDIA module's version");
    let version = version.trim();
    let sizes = host_branch(host_branch_of_version(version)).expect("a profile for this driver branch");
    let rm = LinuxRm::open_card(version, sizes, 0).expect("the card's device files open");
    let mut device = Device::open(rm, sizes).expect("client, device and subdevice");
    let space = device.create_address_space(&[RANGE]).expect("address space");
    println!("pid {} client {:#x} driver {version} mode {mode}", std::process::id(), device.client);

    let mut channels = make(&mut device, sizes, &space, (grouped, mode == "grouped-compute-first"));
    // Each channel runs its prelude and a release, which shows it was started.
    for probed in &mut channels {
        let prelude = probed.channel.prelude_entry();
        probed.add(&mut device, prelude);
        let words = semaphore(probed.semaphore(0), 0x1111, RELEASE);
        probed.run(&mut device, &words);
        let name = probed.name;
        match wait_for(|| probed.seen(0) == 0x1111, Duration::from_secs(2)) {
            Some(took) => println!("{name}: its first release landed after {} µs", took.as_micros()),
            None => {
                println!("{name}: its first release did NOT land in 2 s, so the channel isn't running");
                std::process::exit(1);
            }
        }
    }

    // A compute channel waits for a value the drawing channel releases a little later, then
    // releases one of its own. The time from the drawing channel's doorbell to that is the wait.
    let mut times = Vec::new();
    for round in 1..=waits {
        let drawing_at =
            channels.iter().position(|probed| probed.name == "drawing").expect("a drawing channel");
        channels.swap(0, drawing_at);
        let (drawing, rest) = channels.split_first_mut().expect("three channels");
        let waiter = &mut rest[(round % 2) as usize];
        let mut words = semaphore(drawing.semaphore(1), round, acquire).to_vec();
        words.extend(semaphore(waiter.semaphore(2), round, RELEASE));
        waiter.run(&mut device, &words);
        std::thread::sleep(Duration::from_micros(300));
        if waiter.seen(2) == round {
            println!("round {round}: the wait did not hold");
            std::process::exit(1);
        }
        let words = semaphore(drawing.semaphore(1), round, RELEASE);
        drawing.run(&mut device, &words);
        match wait_for(|| waiter.seen(2) == round, Duration::from_secs(2)) {
            Some(took) => times.push(took.as_micros() as u64),
            None => {
                println!("round {round}: {} never got past its wait", waiter.name);
                std::process::exit(1);
            }
        }
        std::thread::sleep(Duration::from_micros(700));
    }
    times.sort_unstable();
    if !times.is_empty() {
        let at = |part: usize| times[(times.len() - 1) * part / 100];
        println!(
            "{mode}: {} waits, release to the waiter's own release: median {} µs, 90% {} µs, max {} µs",
            times.len(),
            at(50),
            at(90),
            at(100)
        );
    }
    for probed in channels.into_iter().rev() {
        let name = probed.name;
        if let Err(error) = probed.channel.close(&mut device, &space) {
            println!("{name}: closing failed: {error}");
        }
    }
    println!("closed, groups left in the books: {}", device.context_groups.len());
}
