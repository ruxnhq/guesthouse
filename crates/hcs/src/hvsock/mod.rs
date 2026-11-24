//! Hyper-V sockets, which reach a guest with no network: the host connects to a machine by its
//! runtime ID and a service ID, and a Linux guest listens on the matching vsock port. The IDs and
//! their text are plain functions, so they're tested anywhere; the socket itself is Windows only.

#[cfg(windows)]
mod socket;

#[cfg(windows)]
pub use socket::HvStream;

/// What a Linux guest's vsock port becomes as a Hyper-V service ID, as the Linux kernel's `hv_sock`
/// and Microsoft's WSL and hcsshim have it: the port in the GUID's first four bytes, and the rest of
/// this. Port 22 is `00000016-facb-11e6-bd58-64006a7986d3`.
pub const VSOCK_TEMPLATE: u128 = 0x0000_0000_facb_11e6_bd58_6400_6a79_86d3;

/// The Hyper-V service ID of vsock `port`, as a GUID's 128 bits written in order.
pub const fn vsock_service_id(port: u32) -> u128 {
    VSOCK_TEMPLATE | ((port as u128) << 96)
}

/// A GUID's text, in braces, as Windows writes one.
pub fn guid_text(id: u128) -> String {
    let hex = format!("{id:032x}");
    format!("{{{}-{}-{}-{}-{}}}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// The 128 bits of a GUID written as 32 hex digits in 8-4-4-4-12 groups, with or without braces, as
/// HCS writes a machine's runtime ID.
pub fn parse_guid(text: &str) -> Option<u128> {
    let text = text.trim();
    let text = text.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')).unwrap_or(text);
    let groups: Vec<&str> = text.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
    if lengths != [8, 4, 4, 4, 12] || !groups.iter().all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return None;
    }
    u128::from_str_radix(&groups.concat(), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_is_the_first_four_bytes_of_the_service_id() {
        assert_eq!(guid_text(vsock_service_id(22)), "{00000016-facb-11e6-bd58-64006a7986d3}");
        assert_eq!(guid_text(vsock_service_id(564)), "{00000234-facb-11e6-bd58-64006a7986d3}");
        assert_eq!(guid_text(vsock_service_id(0xFFFF_FFFF)), "{ffffffff-facb-11e6-bd58-64006a7986d3}");
    }

    #[test]
    fn a_runtime_id_reads_with_or_without_braces_and_in_any_case() {
        let id = parse_guid("6a7f3c3e-0000-0000-0000-000000000001").unwrap();
        assert_eq!(id, 0x6a7f3c3e_0000_0000_0000_000000000001);
        assert_eq!(parse_guid("{6A7F3C3E-0000-0000-0000-000000000001}"), Some(id));
        assert_eq!(guid_text(id), "{6a7f3c3e-0000-0000-0000-000000000001}");
    }

    #[test]
    fn text_that_is_not_a_guid_is_refused() {
        for text in [
            "",
            "6a7f3c3e",
            "6a7f3c3e-0000-0000-0000-00000000000g",
            "6a7f3c3e0000000000000000000000001",
            "{6a7f3c3e-0000-0000-0000-000000000001",
        ] {
            assert_eq!(parse_guid(text), None, "{text:?}");
        }
    }
}
