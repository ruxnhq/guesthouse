//! This PC's side of the network: the address a device types in, and whether a port is free for the relay.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

/// Never sent to: an address from the range kept for documentation, which only picks the route.
const ANY_REMOTE: (Ipv4Addr, u16) = (Ipv4Addr::new(192, 0, 2, 1), 9);

/// This PC's address on its network, which a device on it types in: the one the system would send
/// from to the internet, if that is on a real network. Nothing is sent to find it out.
pub fn lan_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(ANY_REMOTE).ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}

/// Whether a relay could listen on `port` on every network of this PC now, by binding it for a moment.
pub fn udp_port_free(port: u16) -> bool {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(windows, ignore = "binds every network, which Windows' firewall may ask about")]
    fn a_port_something_listens_on_is_not_free() {
        let taken = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        assert!(!udp_port_free(taken.local_addr().unwrap().port()));
    }
}
