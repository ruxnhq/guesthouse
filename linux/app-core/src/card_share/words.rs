//! What the share panel says. A figure with no reading says so, and nothing is filled in from another.

use super::look::{CardShare, GpuTime, MachineShare};
use super::whole_card::WholeCard;
use crate::gpu_maker::GpuMaker;
use guesthouse_sensors::guest_use::GuestUse;
use std::collections::BTreeMap;

const UNKNOWN: &str = "not available";
const BYTES_PER_GIB: f64 = (1u64 << 30) as f64;

/// The drivers' names for their engines, in the order they're shown, with what people call them.
/// NVIDIA's shader cores, then amdgpu's and i915's names, then xe's. NVIDIA's encoder and decoder
/// go by amdgpu's names.
const ENGINES: [(&str, &str); 19] = [
    ("sm", "graphics and compute"),
    ("gfx", "graphics"),
    ("render", "graphics"),
    ("rcs", "graphics"),
    ("compute", "compute"),
    ("ccs", "compute"),
    ("dma", "copy"),
    ("copy", "copy"),
    ("bcs", "copy"),
    ("dec", "decode"),
    ("dec_1", "decode"),
    ("enc", "encode"),
    ("enc_1", "encode"),
    ("jpeg", "JPEG"),
    ("video", "video"),
    ("vcs", "video"),
    ("video-enhance", "video enhance"),
    ("vecs", "video enhance"),
    ("vpe", "video processing"),
];

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / BYTES_PER_GIB)
}

fn upper_first(text: &str) -> String {
    let mut letters = text.chars();
    letters.next().map(|first| first.to_uppercase().chain(letters).collect()).unwrap_or_default()
}

fn reading<T>(name: &str, value: Option<T>, text: impl Fn(T) -> String) -> String {
    format!("{name} {}", value.map_or_else(|| UNKNOWN.to_owned(), text))
}

/// The whole card, which is the host and every guest together.
pub fn whole_line(whole: &WholeCard) -> String {
    let memory = match (whole.memory_used_bytes, whole.memory_total_bytes) {
        (Some(used), Some(total)) => {
            format!("video memory {:.1} of {}", used as f64 / BYTES_PER_GIB, gib(total))
        }
        (Some(used), None) => format!("video memory {} used", gib(used)),
        _ => format!("video memory {UNKNOWN}"),
    };
    let mut parts = vec![reading("Load", whole.load_percent, |load| format!("{load}%"))];
    if let Some(video) = whole.video_percent {
        parts.push(format!("video engine {video}%"));
    }
    parts.push(memory);
    parts.join(", ")
}

/// Each engine's share in the order of [`ENGINES`], and those it doesn't know after, by their own
/// name. Two engines with one name, as a card with two encoders has, are numbered.
fn engine_words(engines: &BTreeMap<String, u32>) -> String {
    let rank = |name: &str| ENGINES.iter().position(|(known, _)| *known == name).unwrap_or(ENGINES.len());
    let mut names: Vec<&String> = engines.keys().collect();
    names.sort_by_key(|name| (rank(name), (*name).clone()));
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    let parts = names.into_iter().map(|name| {
        let label = ENGINES
            .iter()
            .find(|(known, _)| *known == name.as_str())
            .map_or(name.as_str(), |(_, label)| label);
        let count = seen.entry(label).or_default();
        *count += 1;
        let label = if *count > 1 { format!("{label} {count}") } else { label.to_owned() };
        format!("{label} {}%", engines[name])
    });
    upper_first(&parts.collect::<Vec<_>>().join(", "))
}

/// The guest's video memory by its daemon's books, against its cap, then what the driver charges the
/// machine's processes with, where it counts that. The two differ, and each says which it is.
fn video_words(video: Option<&GuestUse>, card_bytes: Option<u64>) -> String {
    let by_driver = |bytes: u64| format!("{} by the driver's count", gib(bytes));
    let Some(video) = video else {
        return format!("Video memory {}", card_bytes.map_or_else(|| UNKNOWN.to_owned(), by_driver));
    };
    let mut words = format!("Video memory {}", gib(video.video_bytes));
    match video.cap_bytes {
        Some(cap) => words.push_str(&format!(" of its {} cap", gib(cap))),
        None => words.push_str(", no cap"),
    }
    if video.spilled_bytes > 0 {
        words.push_str(&format!(", and {} in system memory", gib(video.spilled_bytes)));
    }
    if let Some(bytes) = card_bytes {
        words.push_str(&format!(", {}", by_driver(bytes)));
    }
    words
}

/// One machine's part of the card: the time each engine spent on it, then its video memory.
pub fn machine_line(share: &MachineShare) -> String {
    let time = match &share.time {
        GpuTime::Engines(engines) if !engines.is_empty() => engine_words(engines),
        GpuTime::NoneSeen => "No GPU time in the last second".to_owned(),
        _ => format!("GPU time {UNKNOWN}"),
    };
    format!("{time}. {}", video_words(share.video.as_ref(), share.card_bytes))
}

/// What the figures mean on a card, or what its maker doesn't let the panel read, said once under
/// the card's name.
pub fn card_note(card: &CardShare) -> Option<&'static str> {
    match card.card.maker {
        GpuMaker::Nvidia if card.per_process => Some(
            "Each machine's GPU time is the driver's last one-second sample of its processes. Its video \
             memory is first what the guest has allocated, which its cap holds, then what the driver \
             charges its processes with, the daemon's own work included, which is what the card gives it.",
        ),
        GpuMaker::Nvidia if card.whole.load_percent.is_none() => Some(
            "Each machine's GPU time isn't read on this card now. It comes from the driver, through the \
             daemon of a Windows machine on the card of this version or later.",
        ),
        GpuMaker::Nvidia => Some(
            "Each machine's GPU time isn't read on this card now, only the whole card's load. It comes from \
             the driver, through the daemon of a Windows machine on the card of this version or later.",
        ),
        GpuMaker::Intel => Some("The whole card's load and video memory aren't read on Intel yet."),
        GpuMaker::Amd => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(engines: Option<&[(&str, u32)]>, video: Option<GuestUse>) -> MachineShare {
        let time = engines.map_or(GpuTime::NotRead, |pairs| {
            GpuTime::Engines(pairs.iter().map(|(name, share)| ((*name).to_owned(), *share)).collect())
        });
        MachineShare { name: "Work".to_owned(), time, video, card_bytes: None }
    }

    fn capped(gib: u64) -> GuestUse {
        GuestUse { daemon: 1, video_bytes: (gib - 1) << 30, spilled_bytes: 0, cap_bytes: Some(gib << 30) }
    }

    #[test]
    fn the_whole_card_says_each_figure_or_that_it_has_none() {
        let amd = WholeCard {
            load_percent: Some(99),
            video_percent: Some(0),
            memory_used_bytes: Some(292_540_416),
            memory_total_bytes: Some(2 << 30),
        };
        assert_eq!(whole_line(&amd), "Load 99%, video engine 0%, video memory 0.3 of 2.0 GiB");
        assert_eq!(whole_line(&WholeCard::default()), "Load not available, video memory not available");
        let nvidia =
            WholeCard { load_percent: Some(0), memory_used_bytes: Some(1 << 30), ..WholeCard::default() };
        assert_eq!(whole_line(&nvidia), "Load 0%, video memory 1.0 GiB used");
    }

    #[test]
    fn a_machine_on_amd_says_its_engines_in_their_order_and_its_memory() {
        let line =
            machine_line(&share(Some(&[("dma", 1), ("gfx", 49), ("enc_1", 0), ("enc", 3)]), Some(capped(4))));
        assert_eq!(
            line,
            "Graphics 49%, copy 1%, encode 3%, encode 2 0%. Video memory 3.0 GiB of its 4.0 GiB cap"
        );
    }

    #[test]
    fn an_engine_no_list_knows_goes_by_its_own_name() {
        let line = machine_line(&share(Some(&[("rcs", 12), ("gsc", 0)]), None));
        assert_eq!(line, "Graphics 12%, gsc 0%. Video memory not available");
    }

    #[test]
    fn a_machine_with_no_time_reading_says_so_and_keeps_its_memory() {
        let uncapped = GuestUse { daemon: 1, video_bytes: 3 << 29, spilled_bytes: 1 << 29, cap_bytes: None };
        assert_eq!(
            machine_line(&share(None, Some(uncapped))),
            "GPU time not available. Video memory 1.5 GiB, no cap, and 0.5 GiB in system memory"
        );
        assert_eq!(
            machine_line(&share(Some(&[]), None)),
            "GPU time not available. Video memory not available"
        );
    }

    /// The guest's books as they were measured on an RTX 5090, under a light load and idle.
    fn measured_books(bytes: u64) -> GuestUse {
        GuestUse { daemon: 1, video_bytes: bytes, spilled_bytes: 0, cap_bytes: Some(4 << 30) }
    }

    #[test]
    fn a_machine_on_nvidia_says_its_sample_and_both_memory_figures() {
        let mut loaded =
            share(Some(&[("sm", 12), ("enc", 0), ("dec", 0)]), Some(measured_books(778_764_288)));
        loaded.card_bytes = Some(1848 << 20);
        assert_eq!(
            machine_line(&loaded),
            "Graphics and compute 12%, decode 0%, encode 0%. Video memory 0.7 GiB of its 4.0 GiB cap, 1.8 GiB \
             by the driver's count"
        );
        let idle = MachineShare {
            time: GpuTime::NoneSeen,
            video: Some(measured_books(281_870_336)),
            card_bytes: Some(940 << 20),
            ..loaded
        };
        assert_eq!(
            machine_line(&idle),
            "No GPU time in the last second. Video memory 0.3 GiB of its 4.0 GiB cap, 0.9 GiB by the driver's \
             count"
        );
    }

    #[test]
    fn a_linux_guest_on_nvidia_has_only_the_drivers_count() {
        let linux = MachineShare { card_bytes: Some(1 << 30), ..share(Some(&[("sm", 30)]), None) };
        assert_eq!(
            machine_line(&linux),
            "Graphics and compute 30%. Video memory 1.0 GiB by the driver's count"
        );
    }

    fn card(maker: GpuMaker, per_process: bool) -> CardShare {
        CardShare {
            card: super::super::look::OnCard { maker, pci: None },
            name: "card".to_owned(),
            whole: WholeCard::default(),
            per_process,
            machines: Vec::new(),
        }
    }

    #[test]
    fn a_card_says_what_its_figures_are_or_what_it_keeps_back() {
        assert!(card_note(&card(GpuMaker::Nvidia, true)).unwrap().contains("one-second sample"));
        let blind = card_note(&card(GpuMaker::Nvidia, false)).unwrap();
        assert!(blind.contains("isn't read on this card") && !blind.contains("whole card"), "{blind}");
        let mut loaded = card(GpuMaker::Nvidia, false);
        loaded.whole.load_percent = Some(7);
        assert!(card_note(&loaded).unwrap().contains("only the whole card's load"));
        assert!(card_note(&card(GpuMaker::Intel, false)).is_some());
        assert_eq!(card_note(&card(GpuMaker::Amd, false)), None);
    }
}
