//! Takes this PC's user name, computer name and profile folder out of text before it goes into a
//! report, along with the shapes that name a person or a device rather than a model: user SIDs,
//! MAC addresses, GUIDs and email addresses. Machines' names go too, as each is a name its user gave.
//!
//! Logs are free text, so this catches the shapes it knows and nothing more. It's a best effort, and
//! nothing here can promise a report holds nothing personal.
//!
//! Every match ignores case, and `\` and `/` are the same separator.
//!
//! This is the Windows host's scrubber. The Linux host has its own in app-core, which takes out a
//! different list in different marks, and the two are kept apart until it's decided which one every
//! host should use.
//!
//! A host that takes out more than this list gives its own pass with [`Scrubber::with_extra`]
//! instead of keeping a copy of the scrubber. The order is fixed here, not by the host: this list
//! first, so the host's pass only ever sees text that is already scrubbed; then the host's passes, in
//! the order given; then this list again, so whatever a host's pass puts in, it can't put back
//! anything this list takes out. A scrubber with no pass of the host's scrubs as it always has.

const HOME_MARK: &str = "~";
const USER_MARK: &str = "<user>";
const HOST_MARK: &str = "<host>";
const SID_MARK: &str = "<sid>";
const MAC_MARK: &str = "<mac>";
const UUID_MARK: &str = "<uuid>";
const EMAIL_MARK: &str = "<email>";
/// The machine a report is about. Any other is this with a number, from 2.
const MACHINE_MARK: &str = "machine";
/// What a machine's names start with in the names of pipes, services and the like.
const NAME_PREFIX: &str = "guesthouse-";
/// A shorter user name as a bare word would take letters out of ordinary words all through a log. It's
/// still caught inside its profile folder's path.
const SHORTEST_BARE_NAME: usize = 3;
/// A shorter machine name is only taken out where it's plainly a name: after the prefix, as a part of
/// a path or as a quoted value.
const SHORTEST_BARE_MACHINE: usize = 4;
/// Words the app's own lines use, or that its folders and programs are called. As a bare word they'd
/// take those words out of every log, so they're only taken out after the prefix, as a whole part of a
/// path or as a whole quoted value, where a name is what such a word almost always is.
const COMMON_WORDS: [&str; 22] = [
    "windows",
    "ubuntu",
    "linux",
    "plain",
    "machine",
    "machines",
    "test",
    "guest",
    "host",
    "start",
    "stop",
    "install",
    "display",
    "gpu",
    "card",
    "daemon",
    "qemu",
    "app",
    "service",
    "shared",
    "snapshot",
    "snapshots",
];
/// GUIDs that name things in Windows or Guesthouse and the same on every PC: the share's name for a
/// Hyper-V shared folder, and the like. They say nothing about a person, and they help to read a log.
const FIXED_UUIDS: [&str; 3] = [
    "dcc079ae-60ba-4d07-847c-3493609c0870",
    "1734c6e8-3154-4dda-ba5f-a874cc483422",
    "064092b3-625e-43bf-9eb5-dc845897dd59",
];

/// A match: how many bytes of the text it takes, and what stands in for them.
type Found = Option<(usize, String)>;

/// A host's own scrubbing pass: text in, the same text with more taken out. A plain function, so it
/// holds nothing of its own (not the names this scrubber takes out) and can go to any thread.
pub type Extra = fn(&str) -> String;

#[derive(Debug, Clone)]
pub struct Scrubber {
    user: String,
    profile: String,
    computer: String,
    /// Each machine's name and the word that stands for it, the longest name first.
    machines: Vec<(String, String)>,
    /// The host's own passes, in the order they run.
    extra: Vec<Extra>,
}

impl Scrubber {
    /// `machines[0]` is the machine the report is about.
    pub fn new(user: &str, profile: &str, computer: &str, machines: &[String]) -> Self {
        let mut named: Vec<(String, String)> = machines
            .iter()
            .enumerate()
            .filter(|(_, name)| !name.trim().is_empty())
            .map(|(number, name)| {
                let mark = if number == 0 {
                    MACHINE_MARK.to_owned()
                } else {
                    format!("{MACHINE_MARK}{}", number + 1)
                };
                (name.trim().to_owned(), mark)
            })
            .collect();
        // The longest first, so a name that starts another is not taken for it.
        named.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
        Self {
            user: user.trim().to_owned(),
            profile: profile.trim().trim_end_matches(['\\', '/']).to_owned(),
            computer: computer.trim().to_owned(),
            machines: named,
            extra: Vec::new(),
        }
    }

    /// The same scrubber with `extra`, a host's own pass, run after this list and before it runs once
    /// more. A second call adds a second pass after the first; none is ever dropped.
    #[must_use]
    pub fn with_extra(mut self, extra: Extra) -> Self {
        self.extra.push(extra);
        self
    }

    /// `text` with this list taken out, and then whatever the host's passes take out as well.
    pub fn scrub(&self, text: &str) -> String {
        let scrubbed = self.shared(text);
        if self.extra.is_empty() {
            return scrubbed;
        }
        let more = self.extra.iter().fold(scrubbed, |text, extra| extra(&text));
        self.shared(&more)
    }

    /// This list alone, as every host takes it out.
    fn shared(&self, text: &str) -> String {
        let text = replace_all(text, |text, at| self.profile_at(text, at));
        let text = replace_all(&text, email_at);
        let text = replace_all(&text, uuid_at);
        let text = replace_all(&text, sid_at);
        let text = replace_all(&text, mac_at);
        let text =
            replace_all(&text, |text, at| whole_word_at(text, at, &self.user, USER_MARK, SHORTEST_BARE_NAME));
        let text = replace_all(&text, |text, at| whole_word_at(text, at, &self.computer, HOST_MARK, 1));
        replace_all(&text, |text, at| self.machine_at(text, at))
    }

    /// The profile folder, as the start of a path. Where it's written in JSON its separators are doubled.
    fn profile_at(&self, text: &str, at: usize) -> Found {
        if self.profile.is_empty() || before(text, at).is_some_and(char::is_alphanumeric) {
            return None;
        }
        let bytes = text.as_bytes();
        let mut end = at;
        for wanted in self.profile.bytes() {
            if is_separator(wanted as char) {
                if !bytes.get(end).is_some_and(|byte| is_separator(*byte as char)) {
                    return None;
                }
                while bytes.get(end).is_some_and(|byte| is_separator(*byte as char)) {
                    end += 1;
                }
            } else if bytes.get(end).is_some_and(|byte| byte.eq_ignore_ascii_case(&wanted)) {
                end += 1;
            } else {
                return None;
            }
        }
        if after(text, end).is_some_and(is_word) {
            return None;
        }
        Some((end - at, HOME_MARK.to_owned()))
    }

    fn machine_at(&self, text: &str, at: usize) -> Found {
        if starts_with_ignoring_case(text, at, NAME_PREFIX) {
            let name_at = at + NAME_PREFIX.len();
            for (name, mark) in &self.machines {
                let end = name_at + name.len();
                if starts_with_ignoring_case(text, name_at, name)
                    && !after(text, end).is_some_and(char::is_alphanumeric)
                {
                    return Some((end - at, format!("{}{mark}", &text[at..name_at])));
                }
            }
        }
        for (name, mark) in &self.machines {
            if !starts_with_ignoring_case(text, at, name) {
                continue;
            }
            let end = at + name.len();
            let (previous, next) = (before(text, at), after(text, end));
            let part_of_path =
                previous.is_some_and(is_separator) && next.is_none_or(|c| is_separator(c) || c == '.');
            let quoted = previous.is_some_and(|c| c == '"' || c == '\'') && previous == next;
            let bare = is_whole(text, at, end)
                && name.chars().count() >= SHORTEST_BARE_MACHINE
                && !COMMON_WORDS.iter().any(|word| word.eq_ignore_ascii_case(name));
            if part_of_path || quoted || bare {
                return Some((name.len(), mark.clone()));
            }
        }
        None
    }
}

/// Every match of `find`, from the start, taken out and its stand-in put in its place.
fn replace_all(text: &str, find: impl Fn(&str, usize) -> Found) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(next) = text[at..].chars().next() {
        match find(text, at) {
            Some((length, stand_in)) if length > 0 => {
                out.push_str(&stand_in);
                at += length;
            }
            _ => {
                out.push(next);
                at += next.len_utf8();
            }
        }
    }
    out
}

/// `word` as a whole word of at least `shortest` characters.
fn whole_word_at(text: &str, at: usize, word: &str, mark: &str, shortest: usize) -> Found {
    let end = at + word.len();
    let found = word.chars().count() >= shortest.max(1)
        && starts_with_ignoring_case(text, at, word)
        && is_whole(text, at, end);
    found.then(|| (word.len(), mark.to_owned()))
}

fn starts_with_ignoring_case(text: &str, at: usize, start: &str) -> bool {
    !start.is_empty()
        && text
            .as_bytes()
            .get(at..at + start.len())
            .is_some_and(|part| part.eq_ignore_ascii_case(start.as_bytes()))
}

fn before(text: &str, at: usize) -> Option<char> {
    text[..at].chars().next_back()
}

fn after(text: &str, at: usize) -> Option<char> {
    text[at..].chars().next()
}

fn is_separator(c: char) -> bool {
    c == '\\' || c == '/'
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `at..end` stands alone, with no letters or digits against it. A stand-in such as `<user>`
/// is never taken for a name again.
fn is_whole(text: &str, at: usize, end: usize) -> bool {
    let (previous, next) = (before(text, at), after(text, end));
    !previous.is_some_and(is_word)
        && !next.is_some_and(is_word)
        && !(previous == Some('<') && next == Some('>'))
}

/// An email address, from the start of its first part.
fn email_at(text: &str, at: usize) -> Found {
    let is_local = |c: char| c.is_ascii_alphanumeric() || "._%+-".contains(c);
    if before(text, at).is_some_and(is_local) {
        return None;
    }
    let local = text[at..].find(|c| !is_local(c)).unwrap_or(text.len() - at);
    let at_sign = at + local;
    if local == 0 || after(text, at_sign) != Some('@') {
        return None;
    }
    let domain_at = at_sign + 1;
    let run = text[domain_at..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'));
    let domain = text[domain_at..][..run.unwrap_or(text.len() - domain_at)].trim_end_matches(['.', '-']);
    let has_dot = domain.find('.').is_some_and(|dot| dot > 0);
    has_dot.then(|| (local + 1 + domain.len(), EMAIL_MARK.to_owned()))
}

/// A GUID, with or without braces round it, which stay.
fn uuid_at(text: &str, at: usize) -> Found {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    if before(text, at).is_some_and(char::is_alphanumeric) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut end = at;
    for (number, length) in GROUPS.into_iter().enumerate() {
        if number > 0 {
            if bytes.get(end) != Some(&b'-') {
                return None;
            }
            end += 1;
        }
        let digits = bytes.get(end..end + length)?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        end += length;
    }
    if after(text, end).is_some_and(char::is_alphanumeric) {
        return None;
    }
    let fixed = FIXED_UUIDS.iter().any(|uuid| uuid.eq_ignore_ascii_case(&text[at..end]));
    (!fixed).then(|| (end - at, UUID_MARK.to_owned()))
}

/// A SID of a user or a computer on a domain: `S-1-5-21` and three or four numbers. The short SIDs of
/// Windows' own groups and accounts are the same on every PC, so they stay.
fn sid_at(text: &str, at: usize) -> Found {
    const START: &str = "S-1-5-21";
    if before(text, at).is_some_and(|c| is_word(c) || c == '-') || !starts_with_ignoring_case(text, at, START)
    {
        return None;
    }
    let bytes = text.as_bytes();
    let (mut end, mut numbers) = (at + START.len(), 0);
    while numbers < 4 && bytes.get(end) == Some(&b'-') {
        let digits = bytes[end + 1..].iter().take_while(|byte| byte.is_ascii_digit()).count();
        if digits == 0 {
            break;
        }
        end += 1 + digits;
        numbers += 1;
    }
    let fits = numbers >= 3 && !after(text, end).is_some_and(is_word);
    fits.then(|| (end - at, SID_MARK.to_owned()))
}

/// A MAC address: six pairs of hex digits, with `-` or `:` between them.
fn mac_at(text: &str, at: usize) -> Found {
    if before(text, at).is_some_and(char::is_alphanumeric) {
        return None;
    }
    // The tail of a longer run of pairs is not a MAC address.
    let mut earlier = text[..at].chars().rev();
    if matches!(earlier.next(), Some('-' | ':')) && earlier.next().is_some_and(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = text.as_bytes();
    let separator = *bytes.get(at + 2)?;
    if separator != b'-' && separator != b':' {
        return None;
    }
    let mut end = at;
    for number in 0..6 {
        if number > 0 {
            if bytes.get(end) != Some(&separator) {
                return None;
            }
            end += 1;
        }
        if !bytes.get(end..end + 2)?.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        end += 2;
    }
    // A longer run of pairs is not a MAC address.
    let longer = bytes.get(end) == Some(&separator) && bytes.get(end + 1).is_some_and(u8::is_ascii_hexdigit);
    (!longer && !after(text, end).is_some_and(char::is_alphanumeric)).then(|| (end - at, MAC_MARK.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pat() -> Scrubber {
        Scrubber::new("Pat", r"C:\Users\Pat", "DESKTOP-1A2B3C", &["win11".to_owned(), "work-box".to_owned()])
    }

    fn assert_scrubs(scrubber: &Scrubber, from: &str, to: &str) {
        assert_eq!(scrubber.scrub(from), to, "from {from:?}");
    }

    #[test]
    fn the_profile_folder_at_the_start_of_a_path_becomes_a_tilde() {
        assert_scrubs(&pat(), r"C:\Users\Pat\Downloads\x.iso", r"~\Downloads\x.iso");
        assert_scrubs(&pat(), "c:/users/pat/Desktop", "~/Desktop");
        assert_scrubs(&pat(), r"opened C:\Users\Pat", "opened ~");
        assert_scrubs(&pat(), r"C:\\Users\\Pat\\x", r"~\\x");
        assert_scrubs(&pat(), r"C:\Users\Patricia\x", r"C:\Users\Patricia\x");
    }

    #[test]
    fn the_user_name_is_taken_as_a_whole_word() {
        assert_scrubs(&pat(), "user PAT signed in", "user <user> signed in");
        assert_scrubs(&pat(), "pathetic", "pathetic");
        assert_scrubs(&pat(), "Pat's PC", "<user>'s PC");
    }

    #[test]
    fn a_short_user_name_is_only_taken_inside_the_profile_path() {
        let al = Scrubber::new("al", r"C:\Users\al", "PC", &[]);
        assert_scrubs(&al, r"C:\Users\al\x", r"~\x");
        assert_scrubs(&al, "al said", "al said");
    }

    #[test]
    fn the_computer_name_is_taken_as_a_whole_word() {
        assert_scrubs(&pat(), "on DESKTOP-1A2B3C now", "on <host> now");
        assert_scrubs(&pat(), "on desktop-1a2b3c.local now", "on <host>.local now");
        assert_scrubs(&pat(), "DESKTOP-1A2B3C2", "DESKTOP-1A2B3C2");
    }

    #[test]
    fn a_users_sid_is_taken_and_a_well_known_one_stays() {
        assert_scrubs(
            &pat(),
            r#"AccessSids: ["S-1-5-21-1004336348-1177238915-682003330-1001", "S-1-5-32-544"]"#,
            r#"AccessSids: ["<sid>", "S-1-5-32-544"]"#,
        );
        for known in ["S-1-5-18", "S-1-5-4", "S-1-5-32-544", "S-1-1-0", "S-1-5-21-1-2"] {
            assert_scrubs(&pat(), known, known);
        }
        assert_scrubs(&pat(), "s-1-5-21-1-2-3-500 and S-1-5-21-4-5-6", "<sid> and <sid>");
    }

    #[test]
    fn mac_addresses_are_taken_with_either_separator() {
        assert_scrubs(&pat(), "mac 00-15-5D-12-34-56 and 00:15:5d:aa:bb:cc", "mac <mac> and <mac>");
        assert_scrubs(&pat(), "12:34:56", "12:34:56");
        assert_scrubs(&pat(), "00:15:5d:aa:bb:cc:dd", "00:15:5d:aa:bb:cc:dd");
    }

    #[test]
    fn guids_are_taken_but_the_fixed_ones_stay() {
        assert_scrubs(&pat(), "{4F2A1C3E-9B7D-4E6A-8C1F-2D3E4F5A6B7C}", "{<uuid>}");
        assert_scrubs(&pat(), "id 4f2a1c3e-9b7d-4e6a-8c1f-2d3e4f5a6b7c.", "id <uuid>.");
        for fixed in [
            r"\\?\VMSMB\VSMB-{dcc079ae-60ba-4d07-847c-3493609c0870}\shared",
            "{1734C6E8-3154-4DDA-BA5F-A874CC483422}",
            "064092b3-625e-43bf-9eb5-dc845897dd59",
        ] {
            assert_scrubs(&pat(), fixed, fixed);
        }
    }

    #[test]
    fn email_addresses_are_taken_before_the_user_name() {
        assert_scrubs(&pat(), "write to pat@example.com", "write to <email>");
        assert_scrubs(&pat(), "write to Pat.Smith+gh@mail.example.co.uk.", "write to <email>.");
        assert_scrubs(&pat(), "a @ b", "a @ b");
    }

    #[test]
    fn a_machine_is_taken_after_the_prefix() {
        assert_scrubs(&pat(), "guesthouse-win11 started", "guesthouse-machine started");
        assert_scrubs(&pat(), r"\\.\pipe\guesthouse-work-box-com1", r"\\.\pipe\guesthouse-machine2-com1");
        assert_scrubs(
            &pat(),
            r"\\.\pipe\guesthouse-win11-0123456789abcdef0123456789abcdef-com1",
            r"\\.\pipe\guesthouse-machine-0123456789abcdef0123456789abcdef-com1",
        );
        assert_scrubs(&pat(), "Guesthouse-WIN11", "Guesthouse-machine");
        assert_scrubs(&pat(), "guesthouse-win112", "guesthouse-win112");
    }

    #[test]
    fn a_machine_is_taken_as_a_part_of_a_path() {
        assert_scrubs(
            &pat(),
            r"C:\ProgramData\Guesthouse\machines\win11\win11.vhdx",
            r"C:\ProgramData\Guesthouse\machines\machine\machine.vhdx",
        );
        let short = Scrubber::new("Pat", r"C:\Users\Pat", "PC", &["vm1".to_owned()]);
        assert_scrubs(
            &short,
            r"C:\Guesthouse\machines\vm1\disk.vhdx",
            r"C:\Guesthouse\machines\machine\disk.vhdx",
        );
    }

    #[test]
    fn a_machine_is_taken_as_a_quoted_value() {
        assert_scrubs(&pat(), r#""name": "win11""#, r#""name": "machine""#);
        let short = Scrubber::new("Pat", r"C:\Users\Pat", "PC", &["vm1".to_owned()]);
        assert_scrubs(&short, r#""name": "vm1""#, r#""name": "machine""#);
        assert_scrubs(&short, "vm1 is a name", "vm1 is a name");
    }

    #[test]
    fn a_machine_is_taken_as_a_bare_word_when_it_is_long_and_not_a_common_word() {
        assert_scrubs(&pat(), "machine win11 is running; WIN11!", "machine machine is running; machine!");
        assert_scrubs(&pat(), "work-box and win11", "machine2 and machine");
        assert_scrubs(&pat(), "win111", "win111");
    }

    #[test]
    fn a_common_word_is_not_taken_as_a_bare_word() {
        let named = Scrubber::new("Pat", r"C:\Users\Pat", "PC", &["windows".to_owned()]);
        assert_scrubs(&named, "the windows guest", "the windows guest");
        assert_scrubs(&named, r#"C:\machines\windows\x "windows""#, r#"C:\machines\machine\x "machine""#);
    }

    #[test]
    fn later_machines_are_numbered_in_the_order_given() {
        let many = Scrubber::new(
            "Pat",
            r"C:\Users\Pat",
            "PC",
            &["alpha".to_owned(), "bravo".to_owned(), "charlie".to_owned(), "alpha2".to_owned()],
        );
        assert_scrubs(&many, "charlie bravo alpha alpha2", "machine3 machine2 machine machine4");
    }

    #[test]
    fn a_stand_in_is_never_taken_for_a_name() {
        let odd = Scrubber::new("Pat", r"C:\Users\Pat", "PC1", &["user".to_owned(), "email".to_owned()]);
        assert_scrubs(&odd, "pat@example.com Pat", "<email> <user>");
    }

    #[test]
    fn an_empty_name_takes_nothing() {
        let none = Scrubber::new("", "", "", &[String::new()]);
        assert_scrubs(&none, r"C:\Users\Pat\x", r"C:\Users\Pat\x");
    }
}

#[cfg(test)]
mod more_cases {
    use super::*;

    #[test]
    fn json_paths_pipes_and_names_at_a_line_start() {
        let s = Scrubber::new("Pat", r"C:\Users\Pat", "DESKTOP-1A2B3C", &["win11".into(), "work-box".into()]);
        for (input, out) in [
            (r#"{"Path": "C:\\Users\\Pat\\vm\\win11.vhdx"}"#, r#"{"Path": "~\\vm\\machine.vhdx"}"#),
            ("guesthouse-win11-console", "guesthouse-machine-console"),
            ("Patrick and pat", "Patrick and <user>"),
            ("work-box: started", "machine2: started"),
            ("win11", "machine"),
        ] {
            assert_eq!(s.scrub(input), out, "{input}");
        }
    }
}
