use guesthouse_backend_intel::IntelBackend;
use guesthouse_backend_intel::card::CardFacts;
use guesthouse_backend_intel::ioctl::{self, INTEL_VENDOR, XE_DRIVER};
use guesthouse_backend_intel::protocol::record;
use guesthouse_backend_intel::recording;
use guesthouse_backend_intel::xe::device::XeCard;
use guesthouse_backend_intel::xe::selftest;
use guesthouse_backend_intel::xe_query::{FirmwareVersion, Gt, MemoryRegion};
use std::path::Path;
use std::process::ExitCode;

const MIB: u64 = 1 << 20;
const RECORD_WORDS_PER_LINE: usize = 6;

const RECORD_OPTION: &str = "--record";
const RUN_OPTION: &str = "--run";
const BREAK_OPTION: &str = "--break";

pub fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let (mut address, mut record_to) = (None, None);
    let (mut run, mut break_steps) = (false, false);
    while let Some(argument) = arguments.next() {
        if argument == RUN_OPTION {
            run = true;
        } else if argument == BREAK_OPTION {
            break_steps = true;
        } else if argument == RECORD_OPTION {
            let Some(file) = arguments.next() else {
                eprintln!("{RECORD_OPTION} needs the file to write the recording to.");
                return ExitCode::FAILURE;
            };
            record_to = Some(file);
        } else {
            address = Some(argument);
        }
    }
    if break_steps && !run {
        eprintln!("{BREAK_OPTION} only goes with {RUN_OPTION}.");
        return ExitCode::FAILURE;
    }
    // A lab run stands in for its card with a recording, and the probe tells what it says, as the
    // daemon uses it: see `recording`.
    if let Some(file) = recording::recorded_card_file() {
        if run {
            eprintln!("{}", selftest::recorded_card_refusal(&file));
            return ExitCode::FAILURE;
        }
        let card = match std::fs::read_to_string(&file)
            .map_err(|error| error.to_string())
            .and_then(|text| recording::read(&text).map_err(|bad| bad.to_string()))
        {
            Ok(card) => card,
            Err(why) => {
                eprintln!("The recorded card in {} can't be read: {why}", file.display());
                return ExitCode::FAILURE;
            }
        };
        println!(
            "recorded card   {} (no card is read: {})",
            file.display(),
            recording::RECORDED_CARD_VARIABLE
        );
        print_card(&file, &card);
        return ExitCode::SUCCESS;
    }
    let Some(node) = ioctl::intel_render_node(address.as_deref()) else {
        eprintln!("No Intel card on the {XE_DRIVER} driver was found.");
        for (name, driver) in ioctl::intel_nodes_in(Path::new("/sys/class/drm")) {
            let driver = driver.as_deref().unwrap_or("no driver");
            eprintln!("  {} is an Intel card on {driver}", name.to_string_lossy());
        }
        return ExitCode::FAILURE;
    };
    let card = match CardFacts::read(&node) {
        Ok(card) => card,
        Err(error) => {
            eprintln!("Opening {} failed: {error}", node.device_file.display());
            return ExitCode::FAILURE;
        }
    };
    print_card(&node.device_file, &card);
    if let Some(file) = record_to {
        if let Err(error) = std::fs::write(&file, recording::write(&card)) {
            eprintln!("Writing the recording to {file} failed: {error}");
            return ExitCode::FAILURE;
        }
        println!("recorded        {file}");
    }
    if run {
        return run_self_test(&node, &card, break_steps);
    }
    ExitCode::SUCCESS
}

/// The self-test on the card just read. See `xe::selftest`.
fn run_self_test(node: &ioctl::RenderNode, card: &CardFacts, break_steps: bool) -> ExitCode {
    println!();
    println!("self-test       a few tiny buffers and batches on this card, freed at the end");
    let mut xe = match XeCard::open(&node.device_file) {
        Ok(xe) => xe,
        Err(error) => {
            eprintln!("Opening {} for the self-test failed: {error}", node.device_file.display());
            return ExitCode::FAILURE;
        }
    };
    let options = selftest::Options { break_steps };
    let (mut input, mut out) = (std::io::stdin().lock(), std::io::stdout().lock());
    let summary = selftest::run(&mut xe, card, Some(&node.sysfs), options, &mut input, &mut out);
    if summary.all_passed() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn or_unknown(text: &Option<String>) -> &str {
    text.as_deref().unwrap_or("unknown")
}

fn print_card(device_file: &Path, card: &CardFacts) {
    println!("intel-probe {}", env!("CARGO_PKG_VERSION"));
    println!("node            {} ({})", device_file.display(), or_unknown(&card.address));
    println!(
        "kernel          {}, driver {}, xe source {}",
        or_unknown(&card.kernel),
        card.driver,
        or_unknown(&card.xe_source)
    );
    let pci = card.pci;
    let vendor_note = if pci.vendor == INTEL_VENDOR { "" } else { " (not Intel)" };
    println!(
        "pci             {:04x}:{:04x} rev {:#04x}, subsystem {:04x}:{:04x}, class {:#08x}{vendor_note}",
        pci.vendor, pci.device, pci.revision, pci.subsystem_vendor, pci.subsystem_device, pci.class
    );
    let config = card.config;
    println!(
        "config          device {:#06x} rev {}, flags {:#x}, alignment {:#x}, va bits {}, top queue priority {}",
        config.device_id,
        config.revision,
        config.flags,
        config.min_alignment,
        config.va_bits,
        config.max_queue_priority
    );
    for region in &card.regions {
        let class = if region.class == MemoryRegion::SYSTEM { "system" } else { "vram" };
        println!(
            "memory          {class} {}: {} MiB, {} MiB used, page {:#x}, cpu visible {} MiB",
            region.instance,
            region.total / MIB,
            region.used / MIB,
            region.min_page_size,
            region.cpu_visible / MIB
        );
    }
    for gt in &card.gts {
        let kind = if gt.kind == Gt::MAIN {
            "main"
        } else if gt.kind == Gt::MEDIA {
            "media"
        } else {
            "other"
        };
        println!(
            "gt              {} {kind} on tile {}: ip {}, timestamp {} Hz, memory near {:#x} far {:#x}",
            gt.id, gt.tile, gt.ip, gt.reference_clock, gt.near_memory, gt.far_memory
        );
    }
    let engines: Vec<String> = card
        .engines
        .iter()
        .map(|engine| format!("{}{} (gt {})", engine.class_name(), engine.instance, engine.gt))
        .collect();
    println!("engines         {}", engines.join(", "));
    for mask in &card.topology {
        let bytes: String = mask.mask.iter().map(|byte| format!("{byte:02x}")).collect();
        println!("topology        gt {} {}: {bytes} ({} set)", mask.gt, mask.name(), mask.bits_set());
    }
    println!(
        "counted         {} dual subslices x {} EUs = {} EUs, slices {}",
        card.dual_subslices(),
        card.eus_per_dual_subslice(),
        card.eu_count(),
        card.slices().map_or("unknown".into(), |slices| slices.to_string())
    );
    for entry in &card.hwconfig.entries {
        let values: Vec<String> = entry.values.iter().map(u32::to_string).collect();
        let name = entry.name().map(|name| format!(" ({name})")).unwrap_or_default();
        println!("hwconfig        {}{name}: {}", entry.key, values.join(" "));
    }
    println!("firmware        GuC {}, HuC {}", firmware(card.guc), firmware(card.huc));
    let f = card.frequencies;
    let mhz = |value: Option<u32>| value.map_or("-".to_string(), |value| value.to_string());
    println!(
        "frequency MHz   RP0 {}, RPn {}, RPe {}, set {}..{}",
        mhz(f.highest),
        mhz(f.lowest),
        mhz(f.efficient),
        mhz(f.min_set),
        mhz(f.max_set)
    );
    for (what, error) in &card.unanswered {
        println!("not answered    {what}: {error}");
    }
    match IntelBackend::from_card(card.clone()) {
        Ok(backend) => print_record(&backend),
        Err(refusal) => println!("profile         none: {refusal}"),
    }
}

/// The record a guest on this card would be given, as its non-zero dwords.
fn print_record(backend: &IntelBackend) {
    println!("profile         {}", backend.generation().name);
    println!("record          {}", record::summary(backend.record()));
    let words: Vec<String> = backend
        .record()
        .chunks(4)
        .enumerate()
        .filter_map(|(index, word)| {
            let value = u32::from_le_bytes(word.try_into().ok()?);
            (value != 0).then(|| format!("{:#06x}:{value:#x}", index * 4))
        })
        .collect();
    for line in words.chunks(RECORD_WORDS_PER_LINE) {
        println!("record words    {}", line.join(" "));
    }
}

fn firmware(version: Option<FirmwareVersion>) -> String {
    version.map_or("unknown".into(), |version| version.to_string())
}
