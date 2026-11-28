//! Announces made-up machines through Windows' own mDNS responder, as the service would a machine's
//! remote view, finds them again on this PC with DNS-SD's browse and resolve, then withdraws them
//! and looks once more:
//!
//!     cargo run -p guesthouse-host-windows --example announce -- [SECONDS]
//!
//! It needs no elevation and opens no port: the ports are made up, so a viewer that lists them
//! can't connect. A second announcement of the first name shows how a name already taken is
//! reported.

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use guesthouse_hcs::dns_sd::{browse, host_name, resolve};
    use guesthouse_host_windows::announce::{DnsSd, service};
    use guesthouse_remote_view_core::announce::{Announcement, Announcer, SERVICE_TYPE};
    use std::process::ExitCode;
    use std::time::{Duration, Instant};

    const LOOK: Duration = Duration::from_secs(3);
    const SETTLE: Duration = Duration::from_secs(2);
    const MACHINES: [(&str, u16, &str); 2] = [
        ("Guesthouse test one", 65101, "0011223344556677ffffffffffffffffffffffffffffffffffffffffffffffff"),
        ("Guesthouse test two", 65102, "8899aabbccddeeffffffffffffffffffffffffffffffffffffffffffffffffff"),
    ];

    let hold = std::env::args().nth(1).and_then(|seconds| seconds.parse().ok()).unwrap_or(5);
    println!("this PC's host name: {:?}", host_name());
    let mut announcers = Vec::new();
    let mut ok = true;
    for (name, port, fingerprint) in MACHINES {
        let announcement = Announcement::new(name, port, fingerprint, None).expect("a valid announcement");
        // As `Lan::with_announcer` takes it.
        let mut announcer: Box<dyn Announcer + Send> = Box::new(DnsSd::new());
        let began = Instant::now();
        let result = announcer.announce(&announcement);
        println!("announce {name:?} on {port}: {result:?} after {:?}", began.elapsed());
        ok &= result.is_ok();
        announcers.push((announcement, announcer));
    }
    let mut twin = DnsSd::new();
    let taken = twin.announce(&announcers[0].0);
    println!("announce {:?} a second time: {taken:?}", announcers[0].0.instance);
    ok &= taken.is_err();

    std::thread::sleep(Duration::from_secs(hold));
    let full_names: Vec<String> =
        announcers.iter().map(|(announcement, _)| service(announcement).full_name()).collect();
    let look = |when: &str| {
        let names = browse(SERVICE_TYPE, LOOK);
        println!("{when}: browse {SERVICE_TYPE}: {names:?}");
        for full in &full_names {
            println!("{when}: resolve {full:?}: {:?}", resolve(full, LOOK));
        }
        names.unwrap_or_default()
    };
    let seen = look("announced");
    ok &= full_names.iter().all(|full| seen.contains(full));
    ok &= !seen.iter().any(|name| name.contains("(1)"));

    for (announcement, announcer) in &mut announcers {
        announcer.withdraw();
        announcer.withdraw();
        println!("withdrew {:?}", announcement.instance);
    }
    // A browse just after can still name the last one taken back, which a resolve no longer finds.
    look("withdrawn");
    std::thread::sleep(SETTLE);
    let seen = look("withdrawn, a moment later");
    ok &= !full_names.iter().any(|full| seen.contains(full));
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This example announces through Windows' own mDNS responder, so it only runs on Windows.");
}
