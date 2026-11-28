//! A card's facts written to a file and read back, so the daemon can stand in for a card this PC
//! doesn't have. `intel-probe --record FILE` writes one on the PC with the card; the daemon reads it
//! only when [`RECORDED_CARD_VARIABLE`] names it, which a person sets on purpose for a lab run.
//!
//! This stage asks the card nothing but the facts in [`CardFacts`], so a recording of them is all a
//! start needs. The file is plain text, a fact a line, so it can be read, compared and mended by
//! hand: a word for the fact, then its values. Numbers are decimal, or hexadecimal after `0x`. A
//! `#` starts a comment. A line it doesn't know is refused, so a damaged file says so instead of
//! standing in for a different card.

use crate::card::CardFacts;
use crate::hwconfig::Entry;
use crate::sysfs::{Frequencies, PciIdentity};
use crate::xe_query::{Config, Engine, FirmwareVersion, Gt, IpVersion, MemoryRegion, TopologyMask};
use std::fmt::Write as _;
use std::path::PathBuf;

/// Names a recording for the daemon, and for the app's card list, to use instead of a card. Unset,
/// or empty, every program looks for the card itself, as it always has.
pub const RECORDED_CARD_VARIABLE: &str = "GUESTHOUSE_INTEL_RECORDED_CARD";
/// The first line, with the format's version.
const FIRST_LINE: &str = "guesthouse-intel-card 1";
const NONE: &str = "-";

/// The recording this process was asked to use, if it was. This is the one place that reads the
/// variable, so nothing picks a recording up by accident.
pub fn recorded_card_file() -> Option<PathBuf> {
    named(std::env::var_os(RECORDED_CARD_VARIABLE))
}

/// The file a value of the variable names. Empty names none, so `VARIABLE=` turns it off.
fn named(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// What is wrong with a recording, with the line it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadRecording(pub String);

impl std::fmt::Display for BadRecording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn hex(value: impl std::fmt::LowerHex) -> String {
    format!("{value:#x}")
}

fn optional(value: Option<u32>) -> String {
    value.map_or_else(|| NONE.to_owned(), |value| value.to_string())
}

/// The facts as a recording.
pub fn write(card: &CardFacts) -> String {
    let mut out = format!("{FIRST_LINE}\n");
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };
    for (word, value) in
        [("address", &card.address), ("kernel", &card.kernel), ("xe-source", &card.xe_source)]
    {
        if let Some(value) = value {
            line(format!("{word} {value}"));
        }
    }
    line(format!("driver {}", card.driver));
    let pci = card.pci;
    line(format!(
        "pci {} {} {} {} {} {}",
        hex(pci.vendor),
        hex(pci.device),
        hex(pci.revision),
        hex(pci.subsystem_vendor),
        hex(pci.subsystem_device),
        hex(pci.class)
    ));
    let config = card.config;
    line(format!(
        "config {} {} {} {} {} {}",
        hex(config.device_id),
        config.revision,
        hex(config.flags),
        hex(config.min_alignment),
        config.va_bits,
        config.max_queue_priority
    ));
    for region in &card.regions {
        line(format!(
            "memory {} {} {} {} {} {}",
            region.class,
            region.instance,
            hex(region.min_page_size),
            hex(region.total),
            hex(region.used),
            hex(region.cpu_visible)
        ));
    }
    for engine in &card.engines {
        line(format!("engine {} {} {}", engine.class, engine.instance, engine.gt));
    }
    for gt in &card.gts {
        line(format!(
            "gt {} {} {} {} {} {} {} {} {}",
            gt.id,
            gt.kind,
            gt.tile,
            gt.ip.major,
            gt.ip.minor,
            gt.ip.revision,
            gt.reference_clock,
            hex(gt.near_memory),
            hex(gt.far_memory)
        ));
    }
    for mask in &card.topology {
        let bytes: String = mask.mask.iter().fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        });
        line(format!("topology {} {} {bytes}", mask.gt, mask.kind));
    }
    for entry in &card.hwconfig.entries {
        let values: Vec<String> = entry.values.iter().map(u32::to_string).collect();
        line(format!("hwconfig {} {}", entry.key, values.join(" ")).trim_end().to_owned());
    }
    for (name, version) in [("guc", card.guc), ("huc", card.huc)] {
        if let Some(version) = version {
            let FirmwareVersion { branch, major, minor, patch } = version;
            line(format!("firmware {name} {branch} {major} {minor} {patch}"));
        }
    }
    let f = card.frequencies;
    line(format!(
        "frequency {} {} {} {} {}",
        optional(f.highest),
        optional(f.lowest),
        optional(f.efficient),
        optional(f.min_set),
        optional(f.max_set)
    ));
    for (what, error) in &card.unanswered {
        line(format!("# not answered when recorded: {what}: {error}"));
    }
    out
}

/// One line's values, read in turn.
struct Values<'a> {
    words: std::str::SplitWhitespace<'a>,
    line: usize,
}

impl<'a> Values<'a> {
    fn bad(&self, why: &str) -> BadRecording {
        BadRecording(format!("line {}: {why}", self.line))
    }

    fn word(&mut self) -> Result<&'a str, BadRecording> {
        self.words.next().ok_or_else(|| self.bad("a value is missing"))
    }

    fn number<T: TryFrom<u64>>(&mut self) -> Result<T, BadRecording> {
        let word = self.word()?;
        let value = match word.strip_prefix("0x") {
            Some(digits) => u64::from_str_radix(digits, 16),
            None => word.parse(),
        };
        let value = value.map_err(|_| self.bad(&format!("{word} is not a number")))?;
        T::try_from(value).map_err(|_| self.bad(&format!("{word} is too big here")))
    }

    fn optional(&mut self) -> Result<Option<u32>, BadRecording> {
        if self.words.clone().next() == Some(NONE) {
            self.words.next();
            return Ok(None);
        }
        self.number().map(Some)
    }

    fn done(&mut self) -> Result<(), BadRecording> {
        match self.words.next() {
            Some(extra) => Err(self.bad(&format!("{extra} is one value too many"))),
            None => Ok(()),
        }
    }
}

fn mask_bytes(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
}

/// Reads a recording back. Nothing the card was never asked is filled in: a fact the file lacks
/// stays as a card that didn't answer would leave it.
pub fn read(text: &str) -> Result<CardFacts, BadRecording> {
    let mut lines = text.lines().enumerate().map(|(at, line)| (at + 1, line.trim()));
    match lines.next() {
        Some((_, first)) if first == FIRST_LINE => {}
        _ => return Err(BadRecording(format!("it doesn't start with \"{FIRST_LINE}\""))),
    }
    let mut card = CardFacts::default();
    for (number, line) in lines.filter(|(_, line)| !line.is_empty() && !line.starts_with('#')) {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let text = || Some(rest.trim().to_owned()).filter(|value| !value.is_empty());
        let mut values = Values { words: rest.split_whitespace(), line: number };
        match word {
            "address" => card.address = text(),
            "kernel" => card.kernel = text(),
            "xe-source" => card.xe_source = text(),
            "driver" => card.driver = rest.trim().to_owned(),
            "pci" => {
                card.pci = PciIdentity {
                    vendor: values.number()?,
                    device: values.number()?,
                    revision: values.number()?,
                    subsystem_vendor: values.number()?,
                    subsystem_device: values.number()?,
                    class: values.number()?,
                };
                values.done()?;
            }
            "config" => {
                card.config = Config {
                    device_id: values.number()?,
                    revision: values.number()?,
                    flags: values.number()?,
                    min_alignment: values.number()?,
                    va_bits: values.number()?,
                    max_queue_priority: values.number()?,
                };
                values.done()?;
            }
            "memory" => {
                card.regions.push(MemoryRegion {
                    class: values.number()?,
                    instance: values.number()?,
                    min_page_size: values.number()?,
                    total: values.number()?,
                    used: values.number()?,
                    cpu_visible: values.number()?,
                });
                values.done()?;
            }
            "engine" => {
                card.engines.push(Engine {
                    class: values.number()?,
                    instance: values.number()?,
                    gt: values.number()?,
                });
                values.done()?;
            }
            "gt" => {
                let (id, kind, tile) = (values.number()?, values.number()?, values.number()?);
                let ip = IpVersion {
                    major: values.number()?,
                    minor: values.number()?,
                    revision: values.number()?,
                };
                card.gts.push(Gt {
                    kind,
                    tile,
                    id,
                    reference_clock: values.number()?,
                    near_memory: values.number()?,
                    far_memory: values.number()?,
                    ip,
                });
                values.done()?;
            }
            "topology" => {
                let (gt, kind) = (values.number()?, values.number()?);
                let bytes = values.word()?;
                let mask = mask_bytes(bytes).ok_or_else(|| values.bad(&format!("{bytes} is not a mask")))?;
                card.topology.push(TopologyMask { gt, kind, mask });
                values.done()?;
            }
            "hwconfig" => {
                let key = values.number()?;
                let mut entry = Entry { key, values: Vec::new() };
                while values.words.clone().next().is_some() {
                    entry.values.push(values.number()?);
                }
                card.hwconfig.entries.push(entry);
            }
            "firmware" => {
                let which = values.word()?.to_owned();
                let version = FirmwareVersion {
                    branch: values.number()?,
                    major: values.number()?,
                    minor: values.number()?,
                    patch: values.number()?,
                };
                values.done()?;
                match which.as_str() {
                    "guc" => card.guc = Some(version),
                    "huc" => card.huc = Some(version),
                    other => return Err(values.bad(&format!("{other} is not GuC or HuC"))),
                }
            }
            "frequency" => {
                card.frequencies = Frequencies {
                    highest: values.optional()?,
                    lowest: values.optional()?,
                    efficient: values.optional()?,
                    min_set: values.optional()?,
                    max_set: values.optional()?,
                };
                values.done()?;
            }
            other => return Err(values.bad(&format!("{other} is not a fact a recording has"))),
        }
    }
    Ok(card)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::tests::{battlemage, lunar_lake};

    #[test]
    fn only_a_value_given_names_a_recording() {
        assert_eq!(named(None), None);
        assert_eq!(named(Some("".into())), None);
        assert_eq!(named(Some("/lab/arc-b580.card".into())), Some("/lab/arc-b580.card".into()));
    }

    #[test]
    fn a_card_reads_back_as_it_was_written() {
        for card in [lunar_lake(), battlemage()] {
            assert_eq!(read(&write(&card)), Ok(card));
        }
    }

    #[test]
    fn a_recording_from_another_program_or_with_a_slip_in_it_is_refused() {
        let good = write(&lunar_lake());
        assert!(read("intel-probe 0.4.1\n").unwrap_err().0.contains("doesn't start with"));
        let typo = good.replace("\nconfig ", "\nconfg ");
        assert!(read(&typo).unwrap_err().0.contains("confg is not a fact"), "{typo}");
        let short = good.replace("engine 0 0 0", "engine 0 0");
        assert!(read(&short).unwrap_err().0.contains("a value is missing"));
        let long = good.replace("engine 0 0 0", "engine 0 0 0 7");
        assert!(read(&long).unwrap_err().0.contains("7 is one value too many"));
        let wide = good.replace("pci 0x8086", "pci 0x18086");
        assert!(read(&wide).unwrap_err().0.contains("too big"));
    }

    #[test]
    fn comments_blank_lines_and_unknown_frequencies_are_taken() {
        let mut card = lunar_lake();
        card.frequencies.efficient = None;
        let text = write(&card).replace("\npci ", "\n# the card\n\npci ");
        assert_eq!(read(&text), Ok(card));
    }
}
