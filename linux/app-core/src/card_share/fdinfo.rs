//! What one open of a card's node has used, as the kernel tells it in `/proc/PID/fdinfo/FD`. Each
//! open is a client of its own, and a daemon has one for each of the guest's address spaces. The
//! keys follow the kernel's DRM usage stats, and engines are read by the shape of their key, so an
//! engine a driver names is read whatever it's called.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const DRIVER: &str = "drm-driver";
const CLIENT: &str = "drm-client-id";
const CARD: &str = "drm-pdev";
/// Time an engine spent on this client's work, in nanoseconds, as amdgpu and i915 give it.
const ENGINE_TIME: &str = "drm-engine-";
/// How many engines of that name there are, when more than one.
const ENGINE_CAPACITY: &str = "drm-engine-capacity-";
/// Clock cycles an engine spent on this client's work, as xe gives it, beside the cycles that went
/// by on the same clock.
const ENGINE_CYCLES: &str = "drm-cycles-";
const ENGINE_TOTAL_CYCLES: &str = "drm-total-cycles-";
const NANOSECONDS: &str = "ns";
const DESCRIPTORS: &str = "fdinfo";

/// What an engine has done for one client so far. Only the change between two looks says anything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EngineCount {
    /// Nanoseconds or cycles busy, as `clock` says.
    pub busy: u64,
    /// The cycles that went by, for an engine counted in cycles. `None` for one counted in time.
    pub clock: Option<u64>,
    /// How many engines share the name, so that all of them busy is the whole of it.
    pub capacity: u32,
}

/// One client of a card.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Client {
    pub driver: String,
    /// The kernel's number for it, unique on its card for as long as it's open.
    pub id: u64,
    /// The card's PCI address, where the driver says.
    pub card: Option<String>,
    pub engines: BTreeMap<String, EngineCount>,
}

impl Client {
    /// Clients repeat across descriptors that share an open, so they go by card and number.
    pub fn key(&self) -> (Option<&str>, u64) {
        (self.card.as_deref(), self.id)
    }
}

fn number(value: &str) -> Option<u64> {
    value.split_whitespace().next()?.parse().ok()
}

/// A time is only one if it says it's in nanoseconds, as the usage stats have it.
fn nanoseconds(value: &str) -> Option<u64> {
    let mut words = value.split_whitespace();
    let count = words.next()?.parse().ok()?;
    (words.next() == Some(NANOSECONDS)).then_some(count)
}

fn engine<'a>(engines: &'a mut BTreeMap<String, EngineCount>, name: &str) -> &'a mut EngineCount {
    engines.entry(name.to_owned()).or_insert(EngineCount { capacity: 1, ..EngineCount::default() })
}

/// Reads one descriptor's fdinfo. `None` for one that isn't a card's client, as most aren't.
pub fn parse(text: &str) -> Option<Client> {
    let mut driver = None;
    let mut id = None;
    let mut card = None;
    let mut engines = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let (key, value) = (key.trim(), value.trim());
        if key == DRIVER {
            driver = Some(value.to_owned());
        } else if key == CLIENT {
            id = number(value);
        } else if key == CARD {
            card = Some(value.to_owned());
        } else if let Some(name) = key.strip_prefix(ENGINE_CAPACITY) {
            if let Some(count) = number(value).and_then(|count| u32::try_from(count).ok()) {
                engine(&mut engines, name).capacity = count.max(1);
            }
        } else if let Some(name) = key.strip_prefix(ENGINE_TIME) {
            if let Some(busy) = nanoseconds(value) {
                engine(&mut engines, name).busy = busy;
            }
        } else if let Some(name) = key.strip_prefix(ENGINE_TOTAL_CYCLES) {
            if let Some(cycles) = number(value) {
                engine(&mut engines, name).clock = Some(cycles);
            }
        } else if let Some(name) = key.strip_prefix(ENGINE_CYCLES)
            && let Some(cycles) = number(value)
        {
            engine(&mut engines, name).busy = cycles;
        }
    }
    Some(Client { driver: driver?, id: id?, card, engines })
}

/// The card clients a process holds, read from `processes`, which is `/proc` outside tests. A process
/// that has gone, or whose descriptors can't be read, holds none that can be seen.
pub fn clients_of(processes: &Path, pid: u32) -> Vec<Client> {
    let folder = processes.join(pid.to_string()).join(DESCRIPTORS);
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    entries.flatten().filter_map(|entry| parse(&fs::read_to_string(entry.path()).ok()?)).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One of a guest's clients on amdgpu, as kernel 7.3 wrote it for a running daemon, shortened.
    pub const AMDGPU: &str = "pos:\t0\nflags:\t0100002\nmnt_id:\t41\nino:\t1353\n\
        drm-driver:\tamdgpu\ndrm-client-id:\t3162\ndrm-pdev:\t0000:14:00.0\npasid:\t1736\n\
        drm-total-vram:\t110504 KiB\ndrm-shared-vram:\t110108 KiB\ndrm-resident-vram:\t110504 KiB\n\
        drm-memory-vram:\t110504 KiB\ndrm-memory-gtt: \t48172 KiB\namd-evicted-vram:\t0 KiB\n\
        amd-requested-vram:\t110504 KiB\ndrm-engine-gfx:\t4671345800 ns\ndrm-engine-dma:\t7263120 ns\n";

    /// A client of xe's in the form the usage stats give, with two compute engines.
    pub const XE: &str = "drm-driver:\txe\ndrm-client-id:\t17\ndrm-pdev:\t0000:03:00.0\n\
        drm-total-vram0:\t40960 KiB\ndrm-cycles-rcs:\t28257900\ndrm-total-cycles-rcs:\t7655183225\n\
        drm-cycles-ccs:\t0\ndrm-total-cycles-ccs:\t7655183225\ndrm-engine-capacity-ccs:\t2\n";

    #[test]
    fn an_amdgpu_client_gives_its_engines_in_nanoseconds() {
        let client = parse(AMDGPU).unwrap();
        assert_eq!((client.driver.as_str(), client.id), ("amdgpu", 3162));
        assert_eq!(client.card.as_deref(), Some("0000:14:00.0"));
        assert_eq!(client.engines["gfx"], EngineCount { busy: 4_671_345_800, clock: None, capacity: 1 });
        assert_eq!(client.engines["dma"].busy, 7_263_120);
        assert_eq!(client.engines.len(), 2, "memory lines are not engines");
    }

    #[test]
    fn an_xe_client_gives_cycles_against_its_clock() {
        let client = parse(XE).unwrap();
        assert_eq!(
            client.engines["rcs"],
            EngineCount { busy: 28_257_900, clock: Some(7_655_183_225), capacity: 1 }
        );
        assert_eq!(client.engines["ccs"].capacity, 2);
        assert!(!client.engines.contains_key("capacity-ccs"));
    }

    #[test]
    fn a_descriptor_that_is_no_cards_client_is_nothing() {
        assert_eq!(parse("pos:\t0\nflags:\t02\nmnt_id:\t25\nino:\t7\n"), None);
        assert_eq!(parse("drm-driver:\tamdgpu\n"), None, "no client number");
    }

    #[test]
    fn a_time_without_its_unit_is_not_taken() {
        let client = parse("drm-driver:\ti915\ndrm-client-id:\t3\ndrm-engine-render:\t12\n").unwrap();
        assert!(client.engines.is_empty());
    }

    #[test]
    fn a_process_holds_the_clients_of_its_descriptors() {
        use crate::testing::Scratch;
        let scratch = Scratch::new();
        let processes = scratch.path("proc");
        let folder = processes.join("42").join(DESCRIPTORS);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("4"), AMDGPU).unwrap();
        fs::write(folder.join("5"), "pos:\t0\nflags:\t02\n").unwrap();
        let clients = clients_of(&processes, 42);
        assert_eq!(clients.iter().map(|client| client.id).collect::<Vec<_>>(), [3162]);
        assert!(clients_of(&processes, 43).is_empty(), "a process that has gone");
    }
}
