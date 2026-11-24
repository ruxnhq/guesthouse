//! A machine's share of each engine, from two looks at its clients a moment apart. The kernel's counts
//! only grow, so a share is what grew between the looks over the time that went by. A client needs
//! both looks to count: one that came or went between them says nothing about that time.

use super::fdinfo::Client;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const WHOLE: u128 = 100;
/// amdgpu writes an engine's line only once the client has used it. Its graphics engine is the one
/// every guest uses, so a client that hasn't is at nothing there, not unknown.
const ALWAYS_COUNTED: [(&str, &str); 1] = [("amdgpu", "gfx")];

/// A machine's clients at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Look {
    /// When it was taken, on a clock that only goes forward.
    pub at: Duration,
    pub clients: Vec<Client>,
}

#[derive(Default)]
struct Tally {
    busy: u128,
    /// All of the engine's time, in the units `busy` is in.
    whole: u128,
}

fn by_key(clients: &[Client]) -> BTreeMap<(Option<&str>, u64), &Client> {
    clients.iter().map(|client| (client.key(), client)).collect()
}

/// What one client's engine did between the looks: busy, and the whole of the time.
fn grown(name: &str, before: &Client, now: &Client, elapsed: u128) -> Option<(u128, u128)> {
    let later = now.engines.get(name);
    let capacity = u128::from(later.map_or(1, |count| count.capacity.max(1)));
    let earlier = before.engines.get(name);
    let busy = |count: Option<&super::fdinfo::EngineCount>| count.map_or(0, |count| count.busy);
    let more = busy(later).checked_sub(busy(earlier))?;
    let whole = match later.and_then(|count| count.clock) {
        Some(clock) => u128::from(clock.checked_sub(earlier?.clock?)?),
        None => elapsed,
    };
    Some((u128::from(more), whole * capacity))
}

/// Each engine's share of the time between the looks, in percent, by the driver's name for it. `None`
/// when no client was there for both, which is no reading, not an idle card.
pub fn shares(before: &Look, now: &Look) -> Option<BTreeMap<String, u32>> {
    let elapsed = now.at.checked_sub(before.at)?.as_nanos();
    if elapsed == 0 {
        return None;
    }
    let earlier = by_key(&before.clients);
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut matched = false;
    for (key, client) in by_key(&now.clients) {
        let Some(before) = earlier.get(&key) else { continue };
        matched = true;
        let always = ALWAYS_COUNTED.iter().filter(|(driver, _)| *driver == client.driver);
        let mut names: BTreeSet<&str> = always.map(|(_, name)| *name).collect();
        names.extend(client.engines.keys().chain(before.engines.keys()).map(String::as_str));
        for name in names {
            let Some((busy, whole)) = grown(name, before, client, elapsed) else { continue };
            let tally = tallies.entry(name.to_owned()).or_default();
            tally.busy += busy;
            tally.whole = tally.whole.max(whole);
        }
    }
    let percent = |tally: &Tally| (tally.busy * WHOLE + tally.whole / 2) / tally.whole.max(1);
    // Work on several queues of one engine can overlap, and the sum can pass all of it.
    let shares = tallies.iter().map(|(name, tally)| (name.clone(), percent(tally).min(WHOLE) as u32));
    matched.then(|| shares.collect())
}

#[cfg(test)]
mod tests {
    use super::super::fdinfo::{EngineCount, parse, tests::AMDGPU, tests::XE};
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    fn look(at: Duration, clients: &[Client]) -> Look {
        Look { at, clients: clients.to_vec() }
    }

    fn with(client: &Client, name: &str, busy: u64, clock: Option<u64>) -> Client {
        let mut later = client.clone();
        let count =
            later.engines.entry(name.to_owned()).or_insert(EngineCount { capacity: 1, ..Default::default() });
        count.busy = busy;
        count.clock = clock;
        later
    }

    #[test]
    fn a_quarter_second_busy_in_a_second_is_a_quarter() {
        let first = parse(AMDGPU).unwrap();
        let gfx = first.engines["gfx"].busy;
        let later = with(&first, "gfx", gfx + 250_000_000, None);
        let shares = shares(&look(SECOND, &[first]), &look(2 * SECOND, &[later])).unwrap();
        assert_eq!((shares["gfx"], shares["dma"]), (25, 0));
    }

    #[test]
    fn clients_add_up_and_a_repeated_one_counts_once() {
        let one = parse(AMDGPU).unwrap();
        let two = Client { id: 9, ..one.clone() };
        let gfx = one.engines["gfx"].busy;
        let (one_later, two_later) =
            (with(&one, "gfx", gfx + 300_000_000, None), with(&two, "gfx", gfx + 100_000_000, None));
        let before = look(SECOND, &[one.clone(), two.clone(), one]);
        let now = look(2 * SECOND, &[one_later.clone(), two_later, one_later]);
        assert_eq!(shares(&before, &now).unwrap()["gfx"], 40);
    }

    #[test]
    fn amdgpu_graphics_unused_is_nothing_not_unknown() {
        let quiet = Client { engines: BTreeMap::new(), ..parse(AMDGPU).unwrap() };
        let shares =
            shares(&look(SECOND, std::slice::from_ref(&quiet)), &look(2 * SECOND, &[quiet])).unwrap();
        assert_eq!(shares, BTreeMap::from([("gfx".to_owned(), 0)]));
    }

    #[test]
    fn a_first_look_or_a_new_client_is_no_reading() {
        let client = parse(AMDGPU).unwrap();
        assert_eq!(shares(&Look::default(), &look(SECOND, std::slice::from_ref(&client))), None);
        let other = Client { id: 1, ..client.clone() };
        assert_eq!(shares(&look(SECOND, std::slice::from_ref(&client)), &look(2 * SECOND, &[other])), None);
        assert_eq!(
            shares(&look(SECOND, std::slice::from_ref(&client)), &look(SECOND, &[client])),
            None,
            "no time went by"
        );
    }

    #[test]
    fn a_client_whose_counts_went_back_is_left_out() {
        let client = parse(AMDGPU).unwrap();
        let back = with(&client, "gfx", 1, None);
        let shares = shares(&look(SECOND, &[client]), &look(2 * SECOND, &[back])).unwrap();
        assert!(!shares.contains_key("gfx"));
    }

    #[test]
    fn xe_cycles_go_against_its_clock_and_all_its_engines() {
        let first = parse(XE).unwrap();
        let rcs = first.engines["rcs"];
        let ccs = first.engines["ccs"];
        let clock = rcs.clock.unwrap();
        let later = with(&first, "rcs", rcs.busy + 600, Some(clock + 1000));
        let later = with(&later, "ccs", ccs.busy + 1000, Some(clock + 1000));
        let shares = shares(&look(SECOND, &[first]), &look(2 * SECOND, &[later])).unwrap();
        assert_eq!((shares["rcs"], shares["ccs"]), (60, 50), "two compute engines, one busy");
    }

    #[test]
    fn overlapping_queues_stop_at_the_whole() {
        let first = parse(AMDGPU).unwrap();
        let later = with(&first, "gfx", first.engines["gfx"].busy + 3_000_000_000, None);
        assert_eq!(shares(&look(SECOND, &[first]), &look(2 * SECOND, &[later])).unwrap()["gfx"], 100);
    }
}
