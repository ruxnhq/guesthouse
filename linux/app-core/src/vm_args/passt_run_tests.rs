//! A held machine's passt words, run by the real passt in a user and network namespace of its own,
//! with an interface named as the VPN's and a `/etc/resolv.conf` of its own that lists an IPv6
//! server and a search domain. Skipped where passt, `ip` or such a namespace isn't to be had.

use super::*;
use crate::only_through::OnlyThrough;
use crate::testing::{Scratch, scratch_context};
use std::io::Write;
use std::net::{Ipv6Addr, UdpSocket};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const RESOLVER: &str = "192.0.2.99";
const HOSTS_SERVER: &str = "2001:db8::53";
const HOSTS_SEARCH: &str = "corp.example";
const GUEST: &str = "2001:db8::10";
/// The guest's IPv6 DNS server on a held machine.
const PASST_DNS6_TEXT: &str = "fd00:3::3";
const WORDS_VARIABLE: &str = "GUESTHOUSE_TEST_PASST_WORDS";
/// The interface, its addresses and routes, and the resolv.conf, then the program after the script.
const SETUP: &str = r#"set -e
ip link add wg0 type dummy; ip link set wg0 up; ip link set lo up
ip addr add 192.0.2.10/24 dev wg0; ip addr add 192.0.2.99/32 dev wg0; ip route add default via 192.0.2.1
ip -6 addr add 2001:db8::10/64 dev wg0 nodad; ip -6 route add default via 2001:db8::1
printf 'nameserver 2001:db8::53\nnameserver 192.0.2.99\nsearch corp.example\n' > "$RESOLV"
mount --bind "$RESOLV" /etc/resolv.conf
exec "$@""#;

fn held_words(scratch: &Scratch) -> Vec<String> {
    let context = scratch_context(scratch);
    let machine = Machine::new("win", "Win", "/m/disk.qcow2", "/m/vars.fd");
    let mut plan = VmPlan::new(&context, &machine, Boot::Gpu, crate::slots::first(), 0);
    plan.network.passt = true;
    plan.network.passt_age = PasstAge::Current;
    plan.network.resolver = Some(RESOLVER.into());
    plan.network.only_through = Some(OnlyThrough { interface: "wg0".into(), ipv6: true });
    plan.passt_words()
}

/// A command that runs `program` in a namespace set up by [`SETUP`], or `None` where that can't be.
fn in_namespace(scratch: &Scratch, program: &[&str]) -> Option<Command> {
    program_on_path("passt")?;
    program_on_path("ip")?;
    let works = Command::new("unshare").args(["-rmn", "true"]).stderr(Stdio::null()).status();
    if !works.is_ok_and(|status| status.success()) {
        return None;
    }
    let mut command = Command::new("unshare");
    command.args(["-rmn", "sh", "-c", SETUP, "sh"]).args(program);
    command.env("RESOLV", scratch.path("resolv.conf"));
    Some(command)
}

#[test]
fn a_held_guest_is_told_no_dns_server_or_search_domain_of_the_hosts() {
    let scratch = Scratch::new();
    let socket = scratch.path("passt.socket");
    let mut program = vec!["timeout", "2", "passt", "-f", "-s", socket.to_str().unwrap()];
    let words = held_words(&scratch);
    program.extend(words.iter().map(String::as_str));
    // After the machine's own words, which ask passt to be quiet.
    program.push("--debug");
    let Some(mut command) = in_namespace(&scratch, &program) else {
        println!("skipped: no passt, ip or user namespace here");
        return;
    };
    let output = command.output().unwrap();
    let said =
        String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("DNS:") && said.contains(PASST_DNS6_TEXT), "{said}");
    assert!(!said.contains(HOSTS_SERVER), "{said}");
    assert!(!said.contains(HOSTS_SEARCH), "{said}");
}

#[test]
fn a_held_guests_ipv6_dns_question_goes_to_the_machines_resolver() {
    let scratch = Scratch::new();
    let words = held_words(&scratch).join("\u{1f}");
    let me = std::env::current_exe().unwrap();
    let test = "vm_args::passt_run_tests::asks_over_ipv6_inside_the_namespace";
    let program = [me.to_str().unwrap(), "--exact", test, "--include-ignored", "--test-threads=1"];
    let Some(mut command) = in_namespace(&scratch, &program) else {
        println!("skipped: no passt, ip or user namespace here");
        return;
    };
    command.env(WORDS_VARIABLE, words).env("GUESTHOUSE_TEST_FOLDER", scratch.root());
    let output = command.output().unwrap();
    let said =
        String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success() && said.contains("1 passed"), "{said}");
}

/// The guest's half of the test above, run only by it, in its namespace.
#[test]
#[ignore = "run by a_held_guests_ipv6_dns_question_goes_to_the_machines_resolver in a namespace"]
fn asks_over_ipv6_inside_the_namespace() {
    let Ok(words) = std::env::var(WORDS_VARIABLE) else { return };
    let folder = std::path::PathBuf::from(std::env::var("GUESTHOUSE_TEST_FOLDER").unwrap());
    let socket = folder.join("inside.socket");
    let resolver = UdpSocket::bind((RESOLVER, 53)).unwrap();
    resolver.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut passt = Command::new("passt")
        .args(["-f", "-q", "-s"])
        .arg(&socket)
        .args(words.split('\u{1f}'))
        .spawn()
        .unwrap();
    let start = Instant::now();
    let mut stream = loop {
        match UnixStream::connect(&socket) {
            Ok(stream) => break stream,
            Err(_) if start.elapsed() < Duration::from_secs(3) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(error) => panic!("passt's socket: {error}"),
        }
    };
    let question = b"\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01";
    let frame = udp6_frame(GUEST.parse().unwrap(), PASST_DNS6_TEXT.parse().unwrap(), 40000, 53, question);
    stream.write_all(&u32::try_from(frame.len()).unwrap().to_be_bytes()).unwrap();
    stream.write_all(&frame).unwrap();
    let mut buffer = [0u8; 512];
    let got = resolver.recv_from(&mut buffer);
    let _ = passt.kill();
    let _ = passt.wait();
    let (length, _) = got.expect("the question reaches the machine's resolver");
    assert_eq!(&buffer[..length], question);
}

/// An Ethernet frame with an IPv6 UDP packet in it, checksum and all.
fn udp6_frame(source: Ipv6Addr, target: Ipv6Addr, from: u16, to: u16, payload: &[u8]) -> Vec<u8> {
    let length = u16::try_from(8 + payload.len()).unwrap();
    let mut udp = [from.to_be_bytes(), to.to_be_bytes(), length.to_be_bytes(), [0, 0]].concat();
    udp.extend_from_slice(payload);
    let mut sum_over = [source.octets().as_slice(), &target.octets()].concat();
    sum_over.extend_from_slice(&u32::from(length).to_be_bytes());
    sum_over.extend_from_slice(&[0, 0, 0, 17]);
    sum_over.extend_from_slice(&udp);
    if sum_over.len() % 2 == 1 {
        sum_over.push(0);
    }
    let mut sum: u32 = sum_over.chunks(2).map(|pair| u32::from(u16::from_be_bytes([pair[0], pair[1]]))).sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    let checksum = !u16::try_from(sum).unwrap();
    udp[6..8].copy_from_slice(&checksum.to_be_bytes());
    let mut frame = vec![0x9a, 0x55, 0x9a, 0x55, 0x9a, 0x55, 0x52, 0x54, 0, 0x12, 0x34, 0x56, 0x86, 0xdd];
    frame.extend_from_slice(&(6u32 << 28).to_be_bytes());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&[17, 64]);
    frame.extend_from_slice(&source.octets());
    frame.extend_from_slice(&target.octets());
    frame.extend_from_slice(&udp);
    frame
}
