//! Sockets with the buffers a burst of video needs.

use socket2::SockRef;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

/// A burst of video is up to 139 datagrams of about 1200 bytes back to back, which the kernel's usual
/// 212 KB default holds only just. The kernel caps what is asked for at its own limit, so this is a wish.
const BUFFER_BYTES: usize = 4 << 20;

/// A socket for the side of the relay that faces the network.
pub(crate) fn bound(address: SocketAddr) -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind(address)?;
    enlarge(&socket);
    Ok(socket)
}

/// A socket for one device. Towards a loopback port, as QEMU's forward is, it is on loopback too, so
/// that nothing from the network can reach it. Towards a guest with an address of its own it has to be
/// on a network the guest can answer on, so it takes whichever the system routes by. Anything on that
/// network can then send to it, so the relay passes on only what comes from the guest's port.
///
/// It is not connected to the upstream port, because QEMU can answer from a different port than the
/// one it listens on, as it does when the guest's port is forwarded twice. A connected socket would
/// drop those answers.
pub(crate) fn towards(upstream: SocketAddr) -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind(SocketAddr::new(local_for(upstream.ip()), 0))?;
    enlarge(&socket);
    Ok(socket)
}

/// The address a device's socket is bound to, for an upstream at `upstream`.
fn local_for(upstream: IpAddr) -> IpAddr {
    match (upstream, upstream.is_loopback()) {
        (IpAddr::V4(_), true) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        (IpAddr::V6(_), true) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        (IpAddr::V4(_), false) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        (IpAddr::V6(_), false) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    }
}

fn enlarge(socket: &UdpSocket) {
    let socket = SockRef::from(socket);
    let _ = socket.set_recv_buffer_size(BUFFER_BYTES);
    let _ = socket.set_send_buffer_size(BUFFER_BYTES);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loopback_upstream_gets_a_socket_on_loopback() {
        let v4 = towards("127.0.0.1:9".parse().unwrap()).unwrap();
        assert_eq!(v4.local_addr().unwrap().ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(local_for("::1".parse().unwrap()), IpAddr::V6(Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn an_upstream_elsewhere_gets_a_socket_on_any_network() {
        assert_eq!(local_for("172.20.0.5".parse().unwrap()), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(local_for("fd00::5".parse().unwrap()), IpAddr::V6(Ipv6Addr::UNSPECIFIED));
    }

    #[test]
    #[cfg_attr(windows, ignore = "binds every network, which Windows' firewall may ask about")]
    fn a_socket_towards_a_guest_with_its_own_address_is_on_every_network() {
        let socket = towards("192.0.2.1:7791".parse().unwrap()).unwrap();
        assert!(socket.local_addr().unwrap().ip().is_unspecified());
    }
}
