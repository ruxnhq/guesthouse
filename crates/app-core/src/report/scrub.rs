//! Takes this PC's user name, host name and home folder out of text before it goes into a report,
//! along with the shapes that name a device rather than a model: UUIDs and MAC addresses. Machines'
//! IDs go too, as each is made from the name its user gave it.
//!
//! Logs are free text, so this catches the shapes it knows and nothing more. It's a best effort, and
//! nothing here can promise a report holds nothing personal.

use regex::Regex;
use rustix::process::getuid;
use std::fs;
use std::path::Path;

const HOME_MARK: &str = "~";
const USER_MARK: &str = "<user>";
const HOST_MARK: &str = "<host>";
const UUID_MARK: &str = "<uuid>";
const MAC_MARK: &str = "<mac>";
/// The machine a report is about. Any other is this with a number, from 2.
const MACHINE_MARK: &str = "machine";
/// IDs that are words the app's own lines use, or that its folders and programs are called. As a bare
/// word they'd take those words out of every log, so they're only taken out as a whole part of a path
/// or a whole quoted value, where an ID is what such a word almost always is.
const COMMON_IDS: [&str; 18] = [
    "windows", "ubuntu", "linux", "plain", "machine", "machines", "test", "guest", "host", "start", "stop",
    "install", "display", "gpu", "card", "daemon", "qemu", "app",
];
/// A shorter name as a bare word would take letters out of ordinary words all through a log. It's
/// still caught inside its home folder's path.
const SHORTEST_BARE_NAME: usize = 3;
/// Host names a distribution gives every PC it installs. They say nothing about whose PC it is, and
/// taking them out would take the distribution's own name out of the facts.
const STOCK_HOST_NAMES: [&str; 10] = [
    "localhost",
    "fedora",
    "archlinux",
    "arch",
    "ubuntu",
    "debian",
    "cachyos",
    "manjaro",
    "nixos",
    "opensuse",
];
const HOST_NAME_FILE: &str = "/proc/sys/kernel/hostname";
const ACCOUNTS_FILE: &str = "/etc/passwd";
const UUID_SHAPE: &str = r"\b[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\b";
const MAC_SHAPE: &str = r"\b[0-9A-Fa-f]{2}(?::[0-9A-Fa-f]{2}){5}\b";

/// What to take out, found once.
pub struct Scrubber {
    /// Each pattern with what goes in its place, in the order they run. A home folder goes before
    /// the bare user name, so its path becomes `~` and not `/home/<user>`.
    rules: Vec<(Regex, String)>,
    /// How many of the rules are for the home folder, which go before the machines' IDs.
    home_rules: usize,
    machines: Vec<MachineId>,
}

/// A machine's ID and what goes in its place.
struct MachineId {
    id: String,
    mark: String,
    /// Whether it's taken out wherever it stands as a word of its own, and not only in a path or quotes.
    as_word: bool,
}

impl Scrubber {
    /// For this PC and this user.
    pub fn for_this_pc(home: &Path) -> Self {
        let host = fs::read_to_string(HOST_NAME_FILE).unwrap_or_default();
        Self::new(&own_user_name(), host.trim(), home)
    }

    pub fn new(user: &str, host: &str, home: &Path) -> Self {
        let mut rules = Vec::new();
        let home = home.to_string_lossy();
        // A home of `/` or none would match every path.
        if home.len() > 1 {
            let home = home.trim_end_matches('/');
            // The path, followed by the end of it, so `/home/al` doesn't take a bite out of `/home/alex`.
            let pattern = format!(r"{}(/|[^A-Za-z0-9._-]|$)", regex::escape(home));
            push_rule(&mut rules, &pattern, &format!("{HOME_MARK}$1"));
        }
        let home_rules = rules.len();
        if user.len() >= SHORTEST_BARE_NAME && user != "root" {
            push_rule(&mut rules, &format!(r"\b{}\b", regex::escape(user)), USER_MARK);
        }
        let stock = STOCK_HOST_NAMES.iter().any(|name| name.eq_ignore_ascii_case(host));
        if host.len() >= SHORTEST_BARE_NAME && !stock {
            push_rule(&mut rules, &format!(r"(?i)\b{}\b", regex::escape(host)), HOST_MARK);
        }
        push_rule(&mut rules, UUID_SHAPE, UUID_MARK);
        push_rule(&mut rules, MAC_SHAPE, MAC_MARK);
        Self { rules, home_rules, machines: Vec::new() }
    }

    /// Takes out these machines' IDs as well: `about`, the machine the report is about, becomes
    /// `<machine>`, and each other one `<machine-2>`, `<machine-3>` and on, the same all through
    /// one report.
    pub fn with_machines(mut self, about: Option<&str>, all: &[String]) -> Self {
        let mut others: Vec<&String> = all.iter().filter(|id| Some(id.as_str()) != about).collect();
        others.sort();
        others.dedup();
        let mut marks: Vec<(&str, String)> =
            about.map(|id| (id, format!("<{MACHINE_MARK}>"))).into_iter().collect();
        for (number, id) in others.into_iter().enumerate() {
            marks.push((id, format!("<{MACHINE_MARK}-{}>", number + 2)));
        }
        // The longest first, so `w11` can't take part of `w11-old` should their bounds ever allow it.
        marks.sort_by_key(|(id, _)| std::cmp::Reverse(id.len()));
        self.machines = marks
            .into_iter()
            .filter(|(id, _)| !id.is_empty())
            .map(|(id, mark)| MachineId {
                id: id.to_owned(),
                mark,
                as_word: id.len() >= SHORTEST_BARE_NAME && !COMMON_IDS.contains(&id),
            })
            .collect();
        self
    }

    pub fn scrub(&self, text: &str) -> String {
        let (home, rest) = self.rules.split_at(self.home_rules);
        let mut text = apply(home, text);
        // Machines go after the home folder and before the user name, as an ID can hold the user's
        // name and would otherwise be left half taken out.
        text = self.scrub_machines(&text);
        apply(rest, &text)
    }

    /// Only the machines' IDs, for the tester's own words, which are otherwise sent as they wrote them.
    pub fn scrub_machines(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for machine in &self.machines {
            text = replace_id(&text, machine);
        }
        text
    }
}

/// Characters an ID can be made of, and so ones that make a longer name when they stand beside it.
fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

fn replace_id(text: &str, machine: &MachineId) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (at, _) in text.match_indices(&machine.id) {
        let end = at + machine.id.len();
        let before = text[..at].bytes().next_back();
        let after = text[end..].bytes().next();
        let alone = |byte: Option<u8>| byte.is_none_or(|byte| !is_name_byte(byte));
        let in_path = before == Some(b'/') && alone(after);
        let quoted = before == Some(b'"') && after == Some(b'"');
        if at >= copied && (in_path || quoted || (machine.as_word && alone(before) && alone(after))) {
            out.push_str(&text[copied..at]);
            out.push_str(&machine.mark);
            copied = end;
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn apply(rules: &[(Regex, String)], text: &str) -> String {
    let mut text = text.to_owned();
    for (pattern, mark) in rules {
        text = pattern.replace_all(&text, mark.as_str()).into_owned();
    }
    text
}

fn push_rule(rules: &mut Vec<(Regex, String)>, pattern: &str, mark: &str) {
    // Every pattern is escaped or fixed, so one that won't build is a bug here and is left out
    // rather than stopping the report.
    if let Ok(regex) = Regex::new(pattern) {
        rules.push((regex, mark.to_owned()));
    }
}

/// The account name, from the system's list by this process's user id, which `$USER` can disagree
/// with under `su` or `sudo`.
fn own_user_name() -> String {
    let uid = getuid().as_raw().to_string();
    let accounts = fs::read_to_string(ACCOUNTS_FILE).unwrap_or_default();
    let listed = accounts.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.get(2) == Some(&uid.as_str())).then(|| fields[0].to_owned())
    });
    listed.or_else(|| std::env::var("USER").ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A user, a PC and a home that belong to nobody.
    fn scrubber() -> Scrubber {
        Scrubber::new("quillon", "marrowbox", Path::new("/home/quillon"))
    }

    #[test]
    fn home_folders_become_a_tilde() {
        let text =
            "opened /home/quillon/.local/share/guesthouse/machines/w11/disk.qcow2\nHOME=/home/quillon\n";
        assert_eq!(
            scrubber().scrub(text),
            "opened ~/.local/share/guesthouse/machines/w11/disk.qcow2\nHOME=~\n"
        );
    }

    #[test]
    fn a_longer_home_with_the_same_start_is_not_cut_in_two() {
        let scrubber = Scrubber::new("al", "marrowbox", Path::new("/home/al"));
        assert_eq!(scrubber.scrub("/home/al/x and /home/alex/y"), "~/x and /home/alex/y");
    }

    #[test]
    fn the_user_and_host_names_are_taken_out_as_words() {
        let text = "quillon@marrowbox: started by quillon on MARROWBOX, as quillons and marrowboxes";
        assert_eq!(
            scrubber().scrub(text),
            "<user>@<host>: started by <user> on <host>, as quillons and marrowboxes"
        );
    }

    #[test]
    fn a_short_user_name_stays_in_words_but_not_in_its_home() {
        let scrubber = Scrubber::new("al", "marrowbox", Path::new("/home/al"));
        assert_eq!(scrubber.scrub("all calls from /home/al/run"), "all calls from ~/run");
    }

    #[test]
    fn a_stock_host_name_stays_as_the_distributions_name() {
        let scrubber = Scrubber::new("quillon", "fedora", Path::new("/home/quillon"));
        assert_eq!(scrubber.scrub("Fedora Linux 42 on fedora"), "Fedora Linux 42 on fedora");
    }

    #[test]
    fn uuids_and_mac_addresses_are_taken_out() {
        let text =
            "root=UUID=3f2a9c1e-77b0-4d5e-9a1c-0b2c3d4e5f60 nic 52:54:00:12:ab:CD at 0000:01:00.0 12:30:45";
        assert_eq!(scrubber().scrub(text), "root=UUID=<uuid> nic <mac> at 0000:01:00.0 12:30:45");
    }

    #[test]
    fn nothing_planted_survives_in_a_whole_log() {
        let log = "\
10:00:01 app: could not read /home/quillon/.config/guesthouse/app.json
10:00:02 quillon-desk task: start w11 on marrowbox
10:00:03 Command Line: /home/quillon/.local/lib/guesthouse/current/guesthouse
10:00:04 Hostname: marrowbox
10:00:05 user quillon (uid 1000)
";
        let scrubbed = scrubber().scrub(log);
        for planted in ["quillon", "marrowbox", "/home/"] {
            assert!(!scrubbed.to_lowercase().contains(planted), "{planted} is still in:\n{scrubbed}");
        }
        assert!(scrubbed.contains("~/.config/guesthouse/app.json"));
    }

    fn with_machines() -> Scrubber {
        let all = ["w11".to_owned(), "quillon-games".to_owned(), "tv".to_owned(), "windows".to_owned()];
        scrubber().with_machines(Some("quillon-games"), &all)
    }

    #[test]
    fn machine_ids_go_in_paths_words_and_quotes() {
        let text = "disk /home/quillon/.local/share/guesthouse/machines/quillon-games/disk.qcow2\n\
                    10:00 quillon-games stopped by itself. w11 is still running.\n\
                    {\"machine\": \"w11\"} and \"tv\" at /store/machines/tv/vars.fd\n";
        assert_eq!(
            with_machines().scrub(text),
            "disk ~/.local/share/guesthouse/machines/<machine>/disk.qcow2\n\
             10:00 <machine> stopped by itself. <machine-3> is still running.\n\
             {\"machine\": \"<machine-3>\"} and \"<machine-2>\" at /store/machines/<machine-2>/vars.fd\n"
        );
    }

    #[test]
    fn a_short_or_common_id_stays_as_a_word() {
        let text = "a tv show on windows, Windows with GPU-PV, /machines/windows/disk and w11-old";
        assert_eq!(
            with_machines().scrub(text),
            "a tv show on windows, Windows with GPU-PV, /machines/<machine-4>/disk and w11-old"
        );
    }

    #[test]
    fn the_same_machine_gets_the_same_mark_all_through() {
        let scrubber = with_machines();
        assert_eq!(scrubber.scrub("w11"), scrubber.scrub("w11"));
        assert_eq!(scrubber.scrub_machines("my quillon-games won't start"), "my <machine> won't start");
    }

    #[test]
    fn with_nothing_known_only_the_shapes_go() {
        let scrubber = Scrubber::new("", "", Path::new(""));
        assert_eq!(scrubber.scrub("/home/x 52:54:00:12:ab:cd"), "/home/x <mac>");
    }
}
