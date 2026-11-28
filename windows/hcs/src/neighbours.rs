//! Which network card answers at an address: the link-layer address this PC reaches it by, as the
//! system resolves it on the interface its route takes. How it's written builds everywhere.

/// A card's address in the form HCN takes and a machine's record keeps: six bytes in capitals with
/// dashes, 00-15-5D-12-34-56.
pub fn mac_text(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join("-")
}

/// The card that answers at `address`, found as this PC would reach it: the interface the route to it
/// takes, and the neighbour resolved there, asked for afresh if the system doesn't have it.
#[cfg(windows)]
pub fn card_at(address: std::net::Ipv4Addr) -> Result<String, String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetBestInterfaceEx, MIB_IPNET_ROW2, ResolveIpNetEntry2,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, IN_ADDR, IN_ADDR_0, SOCKADDR, SOCKADDR_IN};

    let socket = SOCKADDR_IN {
        sin_family: AF_INET,
        sin_port: 0,
        sin_addr: IN_ADDR { S_un: IN_ADDR_0 { S_addr: u32::from_ne_bytes(address.octets()) } },
        sin_zero: [0; 8],
    };
    let mut interface = 0u32;
    // SAFETY: the address is a whole SOCKADDR_IN that outlives the call, and the index comes back in
    // a place for it.
    let status = unsafe { GetBestInterfaceEx((&raw const socket).cast::<SOCKADDR>(), &mut interface) };
    if status != 0 {
        return Err(format!("this PC has no route to {address} (error {status})"));
    }
    let mut row = MIB_IPNET_ROW2::default();
    row.Address.Ipv4 = socket;
    row.InterfaceIndex = interface;
    // SAFETY: the row says the address and the interface, and Windows fills in the rest of it.
    let status = unsafe { ResolveIpNetEntry2(&mut row, std::ptr::null()) };
    if status != 0 {
        return Err(format!("nothing answered at {address} (error {status})"));
    }
    let length = (row.PhysicalAddressLength as usize).min(row.PhysicalAddress.len());
    if length == 0 {
        return Err(format!("{address} is this PC's own"));
    }
    Ok(mac_text(&row.PhysicalAddress[..length]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cards_address_is_written_as_a_machines_record_keeps_it() {
        assert_eq!(mac_text(&[0x00, 0x15, 0x5D, 0x0A, 0xB6, 0x07]), "00-15-5D-0A-B6-07");
    }

    #[cfg(windows)]
    #[test]
    fn a_card_is_written_as_hcn_is_given_it() {
        let guid = crate::network::GUID::from_u128(0x0123_4567_89ab_cdef_0011_2233_4455_6677);
        assert_eq!(mac_text(&[0x00, 0x15, 0x5D, 0x55, 0x66, 0x77]), crate::network::mac_from(&guid));
    }

    /// An address on no network of this PC's has no card.
    #[cfg(windows)]
    #[test]
    fn nothing_answers_where_nothing_is() {
        assert!(card_at(std::net::Ipv4Addr::new(192, 0, 2, 77)).is_err());
    }
}
