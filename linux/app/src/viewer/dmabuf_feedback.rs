//! What the compositor says it would take for one surface, through linux-dmabuf's feedback. A tranche
//! marked for scanout lists the formats it could put straight on the display, which is the only way a
//! frame skips the compositor's own drawing.

use rustix::io::pread;
use std::os::fd::OwnedFd;

/// Each entry of the format table: the fourcc, four bytes of padding, then the modifier.
const ENTRY_BYTES: usize = 16;

/// One group of formats the compositor would take, in its order of preference.
#[derive(Debug, Clone, Default)]
pub struct Tranche {
    pub scanout: bool,
    pub formats: Vec<(u32, u64)>,
}

/// The feedback as it comes in, a piece at a time, until `done`.
#[derive(Default)]
pub struct Reading {
    table: Vec<(u32, u64)>,
    tranches: Vec<Tranche>,
    current: Tranche,
}

impl Reading {
    pub fn format_table(&mut self, file: &OwnedFd, size: u32) {
        let mut bytes = vec![0; size as usize];
        let mut read = 0;
        while read < bytes.len() {
            match pread(file, &mut bytes[read..], read as u64) {
                Ok(0) | Err(_) => break,
                Ok(count) => read += count,
            }
        }
        let (entries, _) = bytes[..read].as_chunks::<ENTRY_BYTES>();
        self.table = entries
            .iter()
            .map(|entry| {
                let fourcc = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
                let modifier = u64::from_le_bytes([
                    entry[8], entry[9], entry[10], entry[11], entry[12], entry[13], entry[14], entry[15],
                ]);
                (fourcc, modifier)
            })
            .collect();
    }

    /// The tranche's formats, as two-byte indices into the table.
    pub fn tranche_formats(&mut self, indices: &[u8]) {
        let (pairs, _) = indices.as_chunks::<2>();
        let found =
            pairs.iter().filter_map(|index| self.table.get(u16::from_le_bytes(*index) as usize).copied());
        self.current.formats.extend(found);
    }

    pub fn tranche_scanout(&mut self, scanout: bool) {
        self.current.scanout = scanout;
    }

    pub fn tranche_done(&mut self) {
        self.tranches.push(std::mem::take(&mut self.current));
    }

    /// The whole feedback, and a fresh start for the next.
    pub fn done(&mut self) -> Vec<Tranche> {
        self.current = Tranche::default();
        std::mem::take(&mut self.tranches)
    }
}

pub fn fourcc_name(fourcc: u32) -> String {
    fourcc.to_le_bytes().iter().map(|byte| char::from(*byte)).collect()
}

/// A line for the log: what could go straight to the screen, and whether `ours` is among it.
pub fn describe(tranches: &[Tranche], ours: Option<(u32, u64)>) -> String {
    let scanout: Vec<&(u32, u64)> =
        tranches.iter().filter(|tranche| tranche.scanout).flat_map(|tranche| &tranche.formats).collect();
    let ours_text = match ours {
        Some((fourcc, modifier)) => {
            let taken = scanout.contains(&&(fourcc, modifier));
            format!("ours {} {modifier:#x} {}", fourcc_name(fourcc), if taken { "is" } else { "is not" })
        }
        None => "ours not known yet".to_owned(),
    };
    let mut names: Vec<String> =
        scanout.iter().map(|(fourcc, modifier)| format!("{} {modifier:#x}", fourcc_name(*fourcc))).collect();
    names.sort();
    names.dedup();
    format!(
        "{} tranches, {} for scanout with {} formats ({ours_text} among them): {}",
        tranches.len(),
        tranches.iter().filter(|tranche| tranche.scanout).count(),
        scanout.len(),
        names.join(", ")
    )
}
