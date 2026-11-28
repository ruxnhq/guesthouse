//! A machine's worker: the Hyper-V process that runs it and serves its pipes. Each machine's worker
//! runs as an account of its own, `NT VIRTUAL MACHINE\<runtime ID>`, whose SID is
//! `S-1-5-83-1-` and the runtime ID's 16 bytes, as Windows keeps a GUID in memory, read as four
//! little-endian numbers. The text is a plain function, so it's tested anywhere.

/// The SID's text of the account the worker of the machine with runtime ID `runtime_id` runs as,
/// the ID being a GUID's 128 bits written in order, as `hvsock::parse_guid` reads it.
pub fn account(runtime_id: u128) -> String {
    let bytes = runtime_id.to_be_bytes();
    // The GUID's first three fields are kept little-endian, and its last eight bytes as written.
    let mut kept = [0u8; 16];
    kept[..4].copy_from_slice(&[bytes[3], bytes[2], bytes[1], bytes[0]]);
    kept[4..6].copy_from_slice(&[bytes[5], bytes[4]]);
    kept[6..8].copy_from_slice(&[bytes[7], bytes[6]]);
    kept[8..].copy_from_slice(&bytes[8..]);
    let part = |at: usize| u32::from_le_bytes([kept[at], kept[at + 1], kept[at + 2], kept[at + 3]]);
    format!("S-1-5-83-1-{}-{}-{}-{}", part(0), part(4), part(8), part(12))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hvsock::parse_guid;

    /// A machine's runtime ID as HCS gave it, and the SID its worker was seen running as.
    #[test]
    fn a_workers_account_is_its_machines_runtime_id_as_four_numbers() {
        let id = parse_guid("4d9e6102-3034-5803-82de-c685d6232367").unwrap();
        assert_eq!(account(id), "S-1-5-83-1-1302225154-1476603956-2244402818-1730356182");
    }

    #[test]
    fn each_byte_of_the_id_lands_in_its_own_place() {
        let id = parse_guid("00010203-0405-0607-0809-0a0b0c0d0e0f").unwrap();
        // The first field reads as written, the next two as one number, and the last eight bytes as two
        // numbers, each from its lowest byte.
        let expected = [0x0001_0203u32, 0x0607_0405, 0x0b0a_0908, 0x0f0e_0d0c];
        assert_eq!(
            account(id),
            format!("S-1-5-83-1-{}-{}-{}-{}", expected[0], expected[1], expected[2], expected[3])
        );
    }
}
