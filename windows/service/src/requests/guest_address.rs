//! The guest's address as this PC passes anything on to it: a port forward's portproxy entry, and a
//! remote view's relay. The guest says its address itself, so it's used only once this machine's
//! own network card, the one its record keeps, is the card that answers there. Any other address,
//! this PC's own or another device's, is refused.

use super::{Service, Work};
use guesthouse_host_windows::network::usable_guest_address;
use guesthouse_host_windows::record::Record;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

/// How long an address that was refused is refused before it's looked at again, as a guest that has
/// only just started may not answer yet.
const AGAIN: Duration = Duration::from_secs(5);

/// What was found of the address a machine's guest said last.
pub(super) struct Checked {
    said: String,
    found: Result<Ipv4Addr, String>,
    when: Instant,
}

impl<W: Work> Service<W> {
    /// `said`, the address `name`'s guest reported this run, as one this PC may pass traffic on to,
    /// or why it isn't. What's found is kept while the guest says the same, and looked at again
    /// after a while if it was refused.
    pub(super) fn guest_address(&self, name: &str, said: &str) -> Result<Ipv4Addr, String> {
        let kept = |checked: &Checked| {
            checked.said == said && (checked.found.is_ok() || checked.when.elapsed() < AGAIN)
        };
        if let Some(checked) = self.checked().get(name).filter(|checked| kept(checked)) {
            return checked.found.clone();
        }
        let found = self.look_at(name, said);
        let checked = Checked { said: said.to_owned(), found: found.clone(), when: Instant::now() };
        self.checked().insert(name.to_owned(), checked);
        found
    }

    fn look_at(&self, name: &str, said: &str) -> Result<Ipv4Addr, String> {
        let address = usable_guest_address(said)
            .ok_or_else(|| format!("the guest's address {said:?} isn't one a guest could have"))?;
        let card = Record::load(&self.machines.store.disk(name))
            .ok()
            .and_then(|record| record.mac)
            .ok_or_else(|| "the machine's network card isn't known".to_owned())?;
        match self.work.network().card_at(address) {
            Ok(found) if found.eq_ignore_ascii_case(&card) => Ok(address),
            Ok(found) => {
                Err(format!("the card at the guest's address {address} is {found}, not the machine's {card}"))
            }
            Err(reason) => Err(format!("the guest's address {address} couldn't be reached: {reason}")),
        }
    }

    fn checked(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Checked>> {
        self.checked.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
