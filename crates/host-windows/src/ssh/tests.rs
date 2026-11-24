use super::*;
use crate::events::{Event, Kept};
use std::net::TcpListener;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

impl GuestSocket for TcpStream {
    fn try_clone(&self) -> io::Result<Box<dyn GuestSocket>> {
        Ok(Box::new(TcpStream::try_clone(self)?))
    }

    fn close(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

/// A guest that is a TCP server on the loopback, standing in for sshd behind its Hyper-V socket.
struct Guest {
    address: SocketAddr,
    dialled: AtomicUsize,
}

impl Dial for Guest {
    fn dial(&self) -> io::Result<Box<dyn GuestSocket>> {
        self.dialled.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(TcpStream::connect(self.address)?))
    }
}

/// A guest that always refuses, as one with nothing on its port does.
struct Nobody;

impl Dial for Nobody {
    fn dial(&self) -> io::Result<Box<dyn GuestSocket>> {
        Err(io::Error::new(io::ErrorKind::ConnectionReset, "nothing is listening"))
    }
}

/// A server that answers each line with `echo: LINE`, until its client closes, and counts the
/// connections that ended.
fn echo_guest() -> (Arc<Guest>, Arc<AtomicUsize>) {
    let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let guest = Arc::new(Guest { address: server.local_addr().unwrap(), dialled: AtomicUsize::new(0) });
    let ended = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&ended);
    thread::spawn(move || {
        for connection in server.incoming().flatten() {
            let counted = Arc::clone(&counted);
            thread::spawn(move || {
                let mut reader = io::BufReader::new(connection.try_clone().unwrap());
                let mut writer = connection;
                let mut line = String::new();
                while io::BufRead::read_line(&mut reader, &mut line).is_ok_and(|read| read > 0) {
                    if write!(writer, "echo: {line}").is_err() {
                        break;
                    }
                    line.clear();
                }
                counted.fetch_add(1, Ordering::SeqCst);
            });
        }
    });
    (guest, ended)
}

fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
}

fn sink() -> (Arc<Kept>, Arc<dyn Sink>) {
    let kept = Arc::new(Kept::default());
    (Arc::clone(&kept), kept)
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let limit = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < limit, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn ask(port: u16, line: &str) -> String {
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    writeln!(client, "{line}").unwrap();
    let mut answer = String::new();
    io::BufRead::read_line(&mut io::BufReader::new(client), &mut answer).unwrap();
    answer
}

#[test]
fn a_connection_to_the_port_reaches_the_guest_and_back() {
    let (guest, _) = echo_guest();
    let (_, sink) = sink();
    let port = free_port();
    let listener = Listener::start(port, guest, sink).unwrap();
    assert_eq!(listener.port(), port);
    assert_eq!(ask(port, "hello"), "echo: hello\n");
    // Each connection gets a guest connection of its own.
    assert_eq!(ask(port, "again"), "echo: again\n");
}

#[test]
fn a_client_that_closes_closes_the_guests_end_too() {
    let (guest, ended) = echo_guest();
    let (_, sink) = sink();
    let port = free_port();
    let _listener = Listener::start(port, guest, sink).unwrap();
    assert_eq!(ask(port, "once"), "echo: once\n");
    until("the guest's end to close", || ended.load(Ordering::SeqCst) == 1);
}

#[test]
fn a_guest_that_closes_closes_the_clients_end_too() {
    // The guest says one thing and hangs up.
    let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let guest = Arc::new(Guest { address: server.local_addr().unwrap(), dialled: AtomicUsize::new(0) });
    thread::spawn(move || {
        if let Ok((mut connection, _)) = server.accept() {
            let _ = connection.write_all(b"bye\n");
        }
    });
    let (_, sink) = sink();
    let port = free_port();
    let _listener = Listener::start(port, guest, sink).unwrap();
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut all = String::new();
    client.read_to_string(&mut all).unwrap();
    assert_eq!(all, "bye\n", "the client reads to its end, which only a closed connection gives");
}

#[test]
fn stopping_closes_every_connection_and_frees_the_port() {
    let (guest, _) = echo_guest();
    let (_, sink) = sink();
    let port = free_port();
    let mut listener = Listener::start(port, guest, sink).unwrap();
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    writeln!(client, "x").unwrap();
    let mut first = [0u8; 8];
    client.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"echo: x\n");
    assert!(!port_free(port), "it listens while it runs");
    listener.stop();
    let mut rest = Vec::new();
    // A closed connection reads to its end, or ends in a reset the stop may have caused.
    let _ = client.read_to_end(&mut rest);
    assert!(port_free(port), "the port is free once it has stopped");
    assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err());
}

#[test]
fn dropping_it_stops_it_as_well() {
    let (guest, _) = echo_guest();
    let (_, sink) = sink();
    let port = free_port();
    drop(Listener::start(port, guest, sink).unwrap());
    assert!(port_free(port));
}

#[test]
fn a_port_that_is_taken_is_an_error_and_not_a_shared_one() {
    let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    assert!(!port_free(port));
    let (_, sink) = sink();
    assert!(Listener::start(port, Arc::new(Nobody), sink).is_err());
}

#[test]
fn a_guest_that_cant_be_reached_closes_the_client_and_is_told_once() {
    let (kept, sink) = sink();
    let port = free_port();
    let _listener = Listener::start(port, Arc::new(Nobody), sink).unwrap();
    for _ in 0..3 {
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut all = Vec::new();
        let _ = client.read_to_end(&mut all);
        assert!(all.is_empty());
    }
    let notes: Vec<Event> =
        kept.0.lock().unwrap().iter().filter(|event| matches!(event, Event::Note(_))).cloned().collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
}

#[test]
fn ports_run_from_2201_up() {
    assert_eq!(ports().next(), Some(2201));
    assert!(ports().all(|port| (2201..=2999).contains(&port)));
}
