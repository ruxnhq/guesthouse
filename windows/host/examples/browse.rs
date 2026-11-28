//! Lists the remote views announced on the network as remoteman's viewers find them: DNS-SD's
//! browse for remoteman's service type, then a resolve of each, with its host, port and records.
//! For checking what the service announces, as any user:
//!
//!     cargo run -p guesthouse-host-windows --example browse -- [SECONDS]

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use guesthouse_hcs::dns_sd::{browse, resolve};
    use guesthouse_remote_view_core::announce::SERVICE_TYPE;
    use std::process::ExitCode;
    use std::time::Duration;

    let look =
        Duration::from_secs(std::env::args().nth(1).and_then(|seconds| seconds.parse().ok()).unwrap_or(3));
    let names = match browse(SERVICE_TYPE, look) {
        Ok(names) => names,
        Err(failure) => {
            eprintln!("browse {SERVICE_TYPE}: {failure}");
            return ExitCode::FAILURE;
        }
    };
    println!("browse {SERVICE_TYPE}: {} found", names.len());
    for name in names {
        println!("{name}: {:?}", resolve(&name, look));
    }
    ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Browsing through Windows' own responder needs Windows.");
}
