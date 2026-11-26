//! The relay on loopback: a stand-in for QEMU's forward on one side and devices on the other.

use guesthouse_relay::{Config, Relay};
use std::net::{SocketAddr, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(3);

fn socket() -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_read_timeout(Some(WAIT)).unwrap();
    socket
}

/// A relay in front of a stand-in forward, and the forward's socket.
fn relay_with(change: impl FnOnce(&mut Config)) -> (Relay, UdpSocket) {
    let forward = socket();
    let mut config = Config::new("127.0.0.1:0".parse().unwrap(), forward.local_addr().unwrap());
    change(&mut config);
    (Relay::start(config).unwrap(), forward)
}

fn receive(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0u8; 2048];
    let (length, from) = socket.recv_from(&mut buffer).unwrap();
    (buffer[..length].to_vec(), from)
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_datagram_goes_through_and_its_answer_comes_back() {
    let (relay, forward) = relay_with(|_| {});
    let device = socket();
    device.send_to(b"input", relay.local_addr()).unwrap();
    let (got, from) = receive(&forward);
    assert_eq!(got, b"input");
    forward.send_to(b"video", from).unwrap();
    let (back, answered_from) = receive(&device);
    assert_eq!(back, b"video");
    assert_eq!(answered_from, relay.local_addr(), "answers come from the address the device used");
}

#[test]
fn answers_from_a_socket_other_than_the_forwarded_port_still_reach_the_device() {
    // QEMU's forward answers from a socket of its own.
    let (relay, forward) = relay_with(|_| {});
    let device = socket();
    device.send_to(b"input", relay.local_addr()).unwrap();
    let (_, source) = receive(&forward);
    let other = socket();
    other.send_to(b"video", source).unwrap();
    assert_eq!(receive(&device).0, b"video");
}

#[test]
fn each_device_has_a_socket_of_its_own_and_gets_only_its_answers() {
    let (relay, forward) = relay_with(|_| {});
    let (first, second) = (socket(), socket());
    first.send_to(b"one", relay.local_addr()).unwrap();
    second.send_to(b"two", relay.local_addr()).unwrap();
    let (a, from_a) = receive(&forward);
    let (b, from_b) = receive(&forward);
    assert_ne!(a, b);
    assert_ne!(from_a, from_b);
    let (from_first, from_second) = if a == b"one" { (from_a, from_b) } else { (from_b, from_a) };
    forward.send_to(b"for second", from_second).unwrap();
    forward.send_to(b"for first", from_first).unwrap();
    assert_eq!(receive(&first).0, b"for first");
    assert_eq!(receive(&second).0, b"for second");
    assert_eq!(relay.clients().len(), 2);
}

#[test]
fn a_device_keeps_the_same_socket_for_as_long_as_it_is_heard() {
    let (relay, forward) = relay_with(|_| {});
    let device = socket();
    let mut sources = Vec::new();
    for _ in 0..5 {
        device.send_to(b"again", relay.local_addr()).unwrap();
        sources.push(receive(&forward).1);
    }
    assert!(sources.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(relay.counters().clients_started, 1);
}

#[test]
fn an_idle_device_is_dropped_and_can_come_back() {
    let (relay, forward) = relay_with(|config| config.idle_timeout = Duration::from_millis(300));
    let device = socket();
    device.send_to(b"hello", relay.local_addr()).unwrap();
    let (_, first_source) = receive(&forward);
    wait_until("the idle device to be dropped", || relay.clients().is_empty());
    assert_eq!(relay.counters().clients_expired, 1);
    device.send_to(b"hello again", relay.local_addr()).unwrap();
    let (got, second_source) = receive(&forward);
    assert_eq!(got, b"hello again");
    assert_ne!(first_source, second_source, "a returning device gets a fresh socket");
    forward.send_to(b"video", second_source).unwrap();
    assert_eq!(receive(&device).0, b"video");
}

#[test]
fn a_device_that_keeps_sending_is_not_dropped() {
    let (relay, forward) = relay_with(|config| config.idle_timeout = Duration::from_millis(400));
    let device = socket();
    for _ in 0..12 {
        device.send_to(b"input", relay.local_addr()).unwrap();
        receive(&forward);
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(relay.clients().len(), 1);
    assert_eq!(relay.counters().clients_expired, 0);
}

#[test]
fn the_most_devices_are_kept_and_the_one_over_is_counted_and_dropped() {
    let (relay, forward) = relay_with(|config| config.max_clients = 2);
    let devices = [socket(), socket(), socket()];
    for device in &devices {
        device.send_to(b"hi", relay.local_addr()).unwrap();
    }
    receive(&forward);
    receive(&forward);
    wait_until("the refusal to be counted", || relay.counters().refused_full == 1);
    assert_eq!(relay.clients().len(), 2);
}

#[test]
fn one_device_going_quiet_changes_nothing_for_another() {
    let (relay, forward) = relay_with(|config| config.idle_timeout = Duration::from_millis(300));
    let (quiet, busy) = (socket(), socket());
    quiet.send_to(b"once", relay.local_addr()).unwrap();
    receive(&forward);
    busy.send_to(b"input", relay.local_addr()).unwrap();
    let (_, busy_source) = receive(&forward);
    // The quiet one expires while the busy one carries on, in both directions.
    for _ in 0..8 {
        busy.send_to(b"input", relay.local_addr()).unwrap();
        assert_eq!(receive(&forward).1, busy_source);
        forward.send_to(b"video", busy_source).unwrap();
        assert_eq!(receive(&busy).0, b"video");
        thread::sleep(Duration::from_millis(60));
    }
    wait_until("the quiet device to be dropped", || relay.clients().len() == 1);
    assert_eq!(relay.clients()[0].address, busy.local_addr().unwrap());
}

#[test]
fn a_burst_for_one_device_arrives_whole_and_in_order() {
    let (relay, forward) = relay_with(|_| {});
    let device = socket();
    // The whole burst is on its way before the test reads any of it, and the default buffer would not hold it.
    rustix::net::sockopt::set_socket_recv_buffer_size(&device, 4 << 20).unwrap();
    device.send_to(b"start", relay.local_addr()).unwrap();
    let (_, source) = receive(&forward);
    const BURST: u32 = 139;
    for number in 0..BURST {
        let mut datagram = vec![0u8; 1200];
        datagram[..4].copy_from_slice(&number.to_be_bytes());
        forward.send_to(&datagram, source).unwrap();
    }
    for number in 0..BURST {
        let (got, _) = receive(&device);
        assert_eq!(got.len(), 1200);
        assert_eq!(got[..4], number.to_be_bytes());
    }
}

#[test]
fn counters_add_up_what_went_through() {
    let (relay, forward) = relay_with(|_| {});
    let device = socket();
    device.send_to(&[0u8; 64], relay.local_addr()).unwrap();
    let (_, source) = receive(&forward);
    forward.send_to(&[0u8; 1200], source).unwrap();
    receive(&device);
    // A datagram is counted once it has been sent, so the device can have it a moment before that.
    wait_until("the counters", || {
        let counters = relay.counters();
        counters.up_packets == 1 && counters.down_packets == 1
    });
    let counters = relay.counters();
    assert_eq!((counters.up_packets, counters.up_bytes), (1, 64));
    assert_eq!((counters.down_packets, counters.down_bytes), (1, 1200));
    let clients = relay.clients();
    assert_eq!((clients[0].up_packets, clients[0].down_packets), (1, 1));
}

#[test]
fn stopping_waits_for_every_thread_and_gives_the_port_back() {
    let (relay, forward) = relay_with(|_| {});
    let address = relay.local_addr();
    let device = socket();
    device.send_to(b"hi", address).unwrap();
    receive(&forward);
    let began = Instant::now();
    relay.stop();
    assert!(began.elapsed() < Duration::from_secs(2), "stopping took {:?}", began.elapsed());
    UdpSocket::bind(address).expect("the port is free again");
}

#[test]
fn a_port_in_use_is_an_error_and_not_a_panic() {
    let taken = socket();
    let config = Config::new(taken.local_addr().unwrap(), "127.0.0.1:9".parse().unwrap());
    assert!(Relay::start(config).is_err());
}
