//! The host's side of a machine's network, as Windows does it: `netsh.exe` from System32 for port
//! forwards, the service list for IP Helper and Hyper-V, `dism.exe` for Hyper-V's feature, and the
//! interface table for adapters.
//! What it says and how it's read is [`crate::network`]'s.

use super::hyper_v::System32Dism;
use crate::hyper_v;
use crate::network::{self as portable, Netsh, Network, Protocol, Proxy, Run};
use guesthouse_control::{Bridging, HyperV};
use guesthouse_hcs::adapters;
use guesthouse_hcs::host;
use guesthouse_hcs::services::{self, Standing};
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Run with no window, as the service has none to show it in.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long `netsh` has to answer before it's stopped. It answers in well under a second, unless
/// the firewall or IP Helper is stuck.
const NETSH_LIMIT: Duration = Duration::from_secs(20);
/// How often a running `netsh` is looked at.
const POLL: Duration = Duration::from_millis(20);
/// The most of its answer that's kept. Listing every rule of the firewall is a megabyte or two.
const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;
/// IP Helper, which listens for the portproxy entries.
const IP_HELPER: &str = "iphlpsvc";

/// `%SystemRoot%\System32\netsh.exe`, run by its full path, with its arguments as they are and no
/// shell between.
struct System32Netsh;

impl System32Netsh {
    fn path() -> PathBuf {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        root.join("System32").join("netsh.exe")
    }
}

impl Run for System32Netsh {
    fn run(&self, args: &[String]) -> Result<String, String> {
        let doing = args.iter().take(3).map(String::as_str).collect::<Vec<_>>().join(" ");
        let mut child = Command::new(Self::path())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("netsh {doing} couldn't start: {error}"))?;
        // Both streams are read as they come, as a full pipe would stop netsh, and it would then only
        // ever end by timing out.
        let drain = |stream: Option<Box<dyn Read + Send>>| {
            thread::spawn(move || {
                let mut text = Vec::new();
                if let Some(stream) = stream {
                    let _ = stream.take(OUTPUT_LIMIT).read_to_end(&mut text);
                }
                String::from_utf8_lossy(&text).into_owned()
            })
        };
        let out = drain(child.stdout.take().map(|stream| Box::new(stream) as Box<dyn Read + Send>));
        let err = drain(child.stderr.take().map(|stream| Box::new(stream) as Box<dyn Read + Send>));
        let until = Instant::now() + NETSH_LIMIT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < until => thread::sleep(POLL),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "netsh {doing} didn't answer in {} s, so it was stopped.",
                        NETSH_LIMIT.as_secs()
                    ));
                }
                Err(error) => return Err(format!("netsh {doing} couldn't be waited for: {error}")),
            }
        };
        let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
        if status.success() {
            return Ok(out);
        }
        // netsh says why on its output, not its error.
        let why = if out.trim().is_empty() { err } else { out };
        let why: String = why.split_whitespace().collect::<Vec<_>>().join(" ");
        Err(format!("netsh {doing} said: {}", why.chars().take(300).collect::<String>()))
    }
}

/// The real thing: `netsh`, the service list, and the interface table.
pub struct RealNetwork {
    netsh: Netsh<System32Netsh>,
    /// The last read of Hyper-V's feature, which is read beside a status, never in it.
    kept: Arc<Mutex<hyper_v::Kept>>,
}

impl RealNetwork {
    pub fn new() -> Self {
        RealNetwork { netsh: Netsh(System32Netsh), kept: Arc::default() }
    }
}

impl Default for RealNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl Network for RealNetwork {
    fn proxies(&self) -> Result<Vec<Proxy>, String> {
        self.netsh.proxies()
    }

    fn add_proxy(&self, listen: u16, address: Ipv4Addr, connect: u16) -> Result<(), String> {
        self.netsh.add_proxy(listen, address, connect)
    }

    fn delete_proxy(&self, listen: u16) -> Result<(), String> {
        self.netsh.delete_proxy(listen)
    }

    fn rules(&self) -> Result<Vec<String>, String> {
        self.netsh.rules()
    }

    fn add_rule(&self, name: &str, port: u16, protocol: Protocol) -> Result<(), String> {
        self.netsh.add_rule(name, port, protocol)
    }

    fn delete_rule(&self, name: &str) -> Result<(), String> {
        self.netsh.delete_rule(name)
    }

    fn port_free(&self, port: u16) -> bool {
        // Bound on every address, as a portproxy entry would be. Windows lets two sockets share a port
        // only when both ask to, and std asks for nothing, so a port with a listener fails.
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).is_ok()
    }

    fn ip_helper_running(&self) -> bool {
        services::standing(IP_HELPER) == Standing::Running
    }

    fn bridging(&self) -> Bridging {
        let service = services::standing(portable::HYPER_V_SERVICE) != Standing::Missing;
        // The feature is read only when its service isn't there, and on a thread of its own, as
        // DISM takes seconds: the last read is used meanwhile.
        let hyper_v = if service {
            HyperV::On
        } else {
            let (feature, begin) =
                self.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take(Instant::now());
            if begin {
                let kept = Arc::clone(&self.kept);
                thread::spawn(move || {
                    let read = hyper_v::read(&System32Dism);
                    kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).put(Instant::now(), read);
                });
            }
            hyper_v::hyper_v_from(&hyper_v::Reading {
                service,
                feature,
                hypervisor: host::hypervisor_present(),
                firmware: Some(host::virtualization_in_firmware()),
            })
        };
        // The adapters are only read when they'll be offered.
        let interfaces = if service { adapters::interfaces() } else { Ok(Vec::new()) };
        portable::bridging_from(hyper_v, interfaces)
    }

    fn enable_hyper_v(&self, progress: &mut dyn FnMut(u8)) -> hyper_v::Enabled {
        self.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).turning = true;
        let enabled = hyper_v::enable(&System32Dism, progress);
        // What it is now, for the next status.
        let read = hyper_v::read(&System32Dism);
        let mut kept = self.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        kept.put(Instant::now(), read);
        kept.turning = false;
        enabled
    }
}
