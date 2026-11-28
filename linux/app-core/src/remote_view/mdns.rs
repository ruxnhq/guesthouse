//! The helper's own mDNS responder: it answers for one machine, on this PC's real networks only,
//! as remoteman's viewers look for a screen, and says goodbye when the network side closes.
//!
//! remoteman's own host may answer on this PC too (it holds port 7791 and announces itself). The
//! responder shares port 5353 with it and with the system's, as mDNS responders do.

use super::{Announcement, Announcer, SERVICE_TYPE, lan_interface};
use mdns_sd::{IfKind, IfPredicate, ServiceDaemon, ServiceInfo};
use std::time::Duration;

/// How long a withdrawal waits for its goodbye to go out.
const GOODBYE: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Mdns {
    daemon: Option<ServiceDaemon>,
    /// The full name of what is announced now.
    announced: Option<String>,
}

impl Mdns {
    fn daemon(&mut self) -> Result<&ServiceDaemon, String> {
        if self.daemon.is_none() {
            let daemon = ServiceDaemon::new().map_err(|error| format!("mDNS could not start: {error}"))?;
            // Loopback, containers, bridges and tunnels never hear it.
            daemon
                .disable_interface(IfKind::Predicate(IfPredicate::new(|interface| {
                    !lan_interface(&interface.name)
                })))
                .map_err(|error| error.to_string())?;
            // The relay listens on IPv4, so an IPv6 address would send a viewer where nothing answers.
            daemon
                .disable_interface(vec![IfKind::LoopbackV4, IfKind::LoopbackV6, IfKind::IPv6])
                .map_err(|e| e.to_string())?;
            self.daemon = Some(daemon);
        }
        Ok(self.daemon.as_ref().expect("made just above"))
    }
}

impl Announcer for Mdns {
    fn announce(&mut self, announcement: &Announcement) -> Result<(), String> {
        self.withdraw();
        let ty_domain = format!("{SERVICE_TYPE}.local.");
        let txt = announcement.txt();
        let properties: Vec<(&str, &str)> = txt.iter().map(|(key, value)| (*key, value.as_str())).collect();
        let host = announcement.host_name();
        let info = match announcement.address.filter(|address| !address.is_unspecified()) {
            Some(address) => ServiceInfo::new(
                &ty_domain,
                &announcement.instance,
                &host,
                address,
                announcement.port,
                &properties[..],
            ),
            None => ServiceInfo::new(
                &ty_domain,
                &announcement.instance,
                &host,
                (),
                announcement.port,
                &properties[..],
            )
            .map(ServiceInfo::enable_addr_auto),
        }
        .map_err(|error| format!("The announcement could not be made: {error}"))?;
        let fullname = info.get_fullname().to_owned();
        self.daemon()?
            .register(info)
            .map_err(|error| format!("The announcement could not be made: {error}"))?;
        self.announced = Some(fullname);
        Ok(())
    }

    fn withdraw(&mut self) {
        let (Some(fullname), Some(daemon)) = (self.announced.take(), &self.daemon) else { return };
        if let Ok(done) = daemon.unregister(&fullname) {
            let _ = done.recv_timeout(GOODBYE);
        }
    }
}

impl Drop for Mdns {
    fn drop(&mut self) {
        self.withdraw();
        if let Some(daemon) = self.daemon.take()
            && let Ok(done) = daemon.shutdown()
        {
            let _ = done.recv_timeout(GOODBYE);
        }
    }
}
