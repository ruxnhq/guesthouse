//! What the share panel says. A figure with no reading says so, and nothing is filled in from another.

use super::look::MachineShare;
use super::whole_card::WholeCard;
use crate::gpu_maker::GpuMaker;
use guesthouse_sensors::guest_use::GuestUse;
use std::collections::BTreeMap;

const UNKNOWN: &str = "not available";
const BYTES_PER_GIB: f64 = (1u64 << 30) as f64;

/// The drivers' names for their engines, in the order they're shown, with what people call them.
/// amdgpu's and i915's names, then xe's.
const ENGINES: [(&str, &str); 18] = [
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

fn video_words(video: Option<&GuestUse>) -> String {
    let Some(video) = video else { return format!("Video memory {UNKNOWN}") };
    let mut words = format!("Video memory {}", gib(video.video_bytes));
    match video.cap_bytes {
        Some(cap) => words.push_str(&format!(" of its {} cap", gib(cap))),
        None => words.push_str(", no cap"),
    }
    if video.spilled_bytes > 0 {
        words.push_str(&format!(", and {} in system memory", gib(video.spilled_bytes)));
    }
    words
}

/// One machine's part of the card: the time each engine spent on it, then its video memory.
pub fn machine_line(share: &MachineShare) -> String {
    let engines = match &share.engines {
        Some(engines) if !engines.is_empty() => engine_words(engines),
        _ => format!("GPU time {UNKNOWN}"),
    };
    format!("{engines}. {}", video_words(share.video.as_ref()))
}

/// What a card's maker doesn't let the panel read, said once under the card's name.
pub fn card_note(maker: GpuMaker) -> Option<&'static str> {
    match maker {
        GpuMaker::Nvidia => {
            Some("Each machine's GPU time isn't read on NVIDIA yet, only the whole card's load.")
        }
        GpuMaker::Intel => Some("The whole card's load and video memory aren't read on Intel yet."),
        GpuMaker::Amd => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(engines: Option<&[(&str, u32)]>, video: Option<GuestUse>) -> MachineShare {
        let engines =
            engines.map(|pairs| pairs.iter().map(|(name, share)| ((*name).to_owned(), *share)).collect());
        MachineShare { name: "Work".to_owned(), engines, video }
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

    #[test]
    fn only_a_maker_that_keeps_something_back_has_a_note() {
        assert!(card_note(GpuMaker::Nvidia).unwrap().contains("Each machine"));
        assert!(card_note(GpuMaker::Intel).is_some());
        assert_eq!(card_note(GpuMaker::Amd), None);
    }
}
