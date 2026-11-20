//! A program built on NixOS names its loader in the store, which a guest doesn't have, so the guest
//! can't run it. Its copy in a guest's image names the usual loader instead, which every x86-64 Linux
//! has at that path. The libraries need nothing: a store folder a library looks in first is simply
//! not there, and the guest's own places come next.

use std::ops::Range;

const USUAL_LOADER: &[u8] = b"/lib64/ld-linux-x86-64.so.2";
const STORE: &[u8] = b"/nix/store/";

// Where a 64-bit little-endian ELF file keeps what this reads, as the ELF specification places it.
const ELF_MAGIC: &[u8] = b"\x7fELF";
const CLASS_AT: usize = 4;
const CLASS_64: u8 = 2;
const BYTE_ORDER_AT: usize = 5;
const LITTLE_ENDIAN: u8 = 1;
const PROGRAM_HEADERS_AT: usize = 0x20;
const PROGRAM_HEADER_SIZE_AT: usize = 0x36;
const PROGRAM_HEADER_COUNT_AT: usize = 0x38;
// And in each program header: what kind it is, and where its contents are in the file and how long.
const KIND_AT: usize = 0;
const CONTENTS_AT: usize = 8;
const CONTENTS_SIZE_AT: usize = 32;
const LOADER_NAME: u32 = 3;

fn field<const SIZE: usize>(bytes: &[u8], at: usize) -> Option<[u8; SIZE]> {
    bytes.get(at..at.checked_add(SIZE)?)?.try_into().ok()
}

fn number(bytes: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u64::from_le_bytes(field(bytes, at)?)).ok()
}

fn short_number(bytes: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_le_bytes(field(bytes, at)?)))
}

/// Where a program's loader is named, as a part of the file, if it names one.
fn loader_name(program: &[u8]) -> Option<Range<usize>> {
    let readable = program.starts_with(ELF_MAGIC)
        && program.get(CLASS_AT) == Some(&CLASS_64)
        && program.get(BYTE_ORDER_AT) == Some(&LITTLE_ENDIAN);
    if !readable {
        return None;
    }
    let first = number(program, PROGRAM_HEADERS_AT)?;
    let size = short_number(program, PROGRAM_HEADER_SIZE_AT)?;
    let count = short_number(program, PROGRAM_HEADER_COUNT_AT)?;
    (0..count).find_map(|index| {
        let header = first.checked_add(index.checked_mul(size)?)?;
        let kind = u32::from_le_bytes(field(program, header.checked_add(KIND_AT)?)?);
        if kind != LOADER_NAME {
            return None;
        }
        let start = number(program, header.checked_add(CONTENTS_AT)?)?;
        let end = start.checked_add(number(program, header.checked_add(CONTENTS_SIZE_AT)?)?)?;
        (end <= program.len()).then_some(start..end)
    })
}

/// Points a program from the store at the usual loader, in place, and says whether it did. The usual
/// name is shorter than any in the store, so it fits where that was, with zeros after it. Anything
/// else, including a script, is left as it is.
pub fn use_usual_loader(program: &mut [u8]) -> bool {
    let Some(range) = loader_name(program) else { return false };
    let name = &mut program[range];
    if !name.starts_with(STORE) || name.len() <= USUAL_LOADER.len() {
        return false;
    }
    name.fill(0);
    name[..USUAL_LOADER.len()].copy_from_slice(USUAL_LOADER);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORE_LOADER: &str =
        "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-glibc-2.40/lib/ld-linux-x86-64.so.2";
    const HEADER_SIZE: usize = 64;
    const PROGRAM_HEADER_SIZE: usize = 56;
    const LOADABLE: u32 = 1;

    /// The least of a program: its header, a loadable part, and the part naming its loader.
    fn program(loader: &str) -> Vec<u8> {
        let name_at = HEADER_SIZE + 2 * PROGRAM_HEADER_SIZE;
        let mut bytes = vec![0u8; name_at];
        bytes[..4].copy_from_slice(ELF_MAGIC);
        bytes[CLASS_AT] = CLASS_64;
        bytes[BYTE_ORDER_AT] = LITTLE_ENDIAN;
        bytes[PROGRAM_HEADERS_AT..][..8].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
        bytes[PROGRAM_HEADER_SIZE_AT..][..2].copy_from_slice(&(PROGRAM_HEADER_SIZE as u16).to_le_bytes());
        bytes[PROGRAM_HEADER_COUNT_AT..][..2].copy_from_slice(&2u16.to_le_bytes());
        bytes[HEADER_SIZE..][..4].copy_from_slice(&LOADABLE.to_le_bytes());
        let named = HEADER_SIZE + PROGRAM_HEADER_SIZE;
        bytes[named..][..4].copy_from_slice(&LOADER_NAME.to_le_bytes());
        bytes[named + CONTENTS_AT..][..8].copy_from_slice(&(name_at as u64).to_le_bytes());
        let length = loader.len() as u64 + 1;
        bytes[named + CONTENTS_SIZE_AT..][..8].copy_from_slice(&length.to_le_bytes());
        bytes.extend(loader.as_bytes());
        bytes.push(0);
        bytes.extend(b"the rest of the program");
        bytes
    }

    #[test]
    fn a_program_from_the_store_gets_the_usual_loader() {
        let mut bytes = program(STORE_LOADER);
        let before = bytes.len();
        assert!(use_usual_loader(&mut bytes));
        assert_eq!(bytes.len(), before);
        let name = &bytes[loader_name(&bytes).unwrap()];
        assert!(name.starts_with(USUAL_LOADER) && name[USUAL_LOADER.len()..].iter().all(|byte| *byte == 0));
        assert!(bytes.ends_with(b"the rest of the program"));
    }

    #[test]
    fn a_program_with_the_usual_loader_is_left_alone() {
        let mut bytes = program("/lib64/ld-linux-x86-64.so.2");
        let before = bytes.clone();
        assert!(!use_usual_loader(&mut bytes));
        assert_eq!(bytes, before);
    }

    #[test]
    fn a_script_or_a_cut_off_program_is_left_alone() {
        let mut script = b"#!/nix/store/0000-bash/bin/sh\nexec nvidia-smi\n".to_vec();
        assert!(!use_usual_loader(&mut script));
        let mut cut = program(STORE_LOADER);
        cut.truncate(HEADER_SIZE + PROGRAM_HEADER_SIZE + 10);
        assert!(!use_usual_loader(&mut cut));
    }
}
