//! The host's network adapters, as a machine could be bridged onto one. What counts as a physical
//! adapter is a pure rule over the fields Windows gives, so it's tested anywhere; reading the fields
//! is Windows only.

/// An adapter's type for Ethernet (IF_TYPE_ETHERNET_CSMACD), which is also what many virtual
/// adapters call themselves.
pub const ETHERNET: u32 = 6;
/// An adapter's type for Wi-Fi (IF_TYPE_IEEE80211).
pub const WIFI: u32 = 71;

/// One row of Windows' interface table (GetIfTable2), with what a bridge needs of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    /// Its name in Windows' network settings, which HCN binds a network by.
    pub name: String,
    /// What its driver calls it.
    pub description: String,
    /// IF_TYPE_*.
    pub kind: u32,
    /// Whether it's up (IfOperStatusUp).
    pub up: bool,
    /// MIB_IF_ROW2's HardwareInterface: a real device behind it, which no virtual adapter has.
    pub hardware: bool,
    /// MIB_IF_ROW2's FilterInterface: a filter driver's own layer over another adapter, not an
    /// adapter in its own right.
    pub filter: bool,
}

/// The adapters a machine can be bridged onto: Ethernet and Wi-Fi hardware that's up, without
/// the filter layers drivers add over them. Virtual switches, VPNs, VirtualBox and VMware's
/// adapters call themselves Ethernet too, but have no hardware behind them.
pub fn physical(interfaces: &[Interface]) -> Vec<&Interface> {
    interfaces
        .iter()
        .filter(|adapter| {
            matches!(adapter.kind, ETHERNET | WIFI)
                && adapter.up
                && adapter.hardware
                && !adapter.filter
                && !adapter.name.is_empty()
        })
        .collect()
}

/// Every interface Windows has, or why they couldn't be read.
#[cfg(windows)]
pub fn interfaces() -> Result<Vec<Interface>, String> {
    use crate::text::from_wide;
    use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};

    /// MIB_IF_ROW2's InterfaceAndOperStatusFlags bits, from the lowest.
    const HARDWARE_INTERFACE: u8 = 1;
    const FILTER_INTERFACE: u8 = 1 << 1;
    /// IfOperStatusUp.
    const OPER_STATUS_UP: i32 = 1;

    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: `table` is a place for Windows' answer, which is freed below.
    let status = unsafe { GetIfTable2(&mut table) };
    if status != 0 || table.is_null() {
        return Err(format!("Windows couldn't list the network adapters (error {status})."));
    }
    // SAFETY: the table holds `NumEntries` rows from its first, and stays until it's freed.
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
    let found = rows
        .iter()
        .map(|row| Interface {
            name: from_wide(&row.Alias),
            description: from_wide(&row.Description),
            kind: row.Type,
            up: row.OperStatus == OPER_STATUS_UP,
            hardware: row.InterfaceAndOperStatusFlags._bitfield & HARDWARE_INTERFACE != 0,
            filter: row.InterfaceAndOperStatusFlags._bitfield & FILTER_INTERFACE != 0,
        })
        .collect();
    // SAFETY: the table came from GetIfTable2 and nothing refers to it any more.
    unsafe { FreeMibTable(table.cast()) };
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface(name: &str, kind: u32, up: bool, hardware: bool) -> Interface {
        Interface {
            name: name.into(),
            description: format!("{name} adapter"),
            kind,
            up,
            hardware,
            filter: false,
        }
    }

    #[test]
    fn only_ethernet_and_wifi_hardware_that_is_up_can_be_bridged() {
        let all = [
            interface("Ethernet", ETHERNET, true, true),
            interface("Wi-Fi", WIFI, true, true),
            interface("Ethernet 2", ETHERNET, false, true),
            // Virtual switches and VPNs say Ethernet but have no hardware.
            interface("vEthernet (Default Switch)", ETHERNET, true, false),
            interface("VirtualBox Host-Only Network", ETHERNET, true, false),
            // Loopback, PPP and tunnels.
            interface("Loopback Pseudo-Interface 1", 24, true, false),
            interface("Mobile broadband", 243, true, true),
            Interface { filter: true, ..interface("Ethernet-QoS Packet Scheduler", ETHERNET, true, true) },
            interface("", ETHERNET, true, true),
        ];
        let names: Vec<&str> = physical(&all).iter().map(|adapter| adapter.name.as_str()).collect();
        assert_eq!(names, ["Ethernet", "Wi-Fi"]);
    }
}
