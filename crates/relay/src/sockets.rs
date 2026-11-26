//! Sockets with the buffers a burst of video needs.

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

/// A socket for one device, on loopback so that nothing from the network can reach it. It is not
/// connected to the loopback port, because QEMU can answer from a different port than the one it
/// listens on, as it does when the guest's port is forwarded twice. A connected socket would drop
/// those answers.
pub(crate) fn towards(upstream: SocketAddr) -> io::Result<UdpSocket> {
    let loopback =
        if upstream.is_ipv4() { IpAddr::V4(Ipv4Addr::LOCALHOST) } else { IpAddr::V6(Ipv6Addr::LOCALHOST) };
    let from = SocketAddr::new(loopback, 0);
    let socket = UdpSocket::bind(from)?;
    enlarge(&socket);
    Ok(socket)
}

fn enlarge(socket: &UdpSocket) {
    let _ = rustix::net::sockopt::set_socket_recv_buffer_size(socket, BUFFER_BYTES);
    let _ = rustix::net::sockopt::set_socket_send_buffer_size(socket, BUFFER_BYTES);
}
