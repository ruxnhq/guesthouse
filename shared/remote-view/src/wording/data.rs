//! The sentences of [`super`] as data, for an app that cannot call Rust: each with a key that never
//! changes, its English text, and the names of the values it takes.
//!
//! The functions in [`super`] stay the source. Every text here is made by calling the function with
//! each value given as its placeholder, so the two cannot differ; a sentence that takes a number is
//! made the same way from the text the number is written as.
//!
//! - **Keys** are `remote_view.` and the Rust name in lower case: `remote_view.accept` for
//!   [`super::ACCEPT`], `remote_view.wants_to_pair` for [`super::wants_to_pair`]. A function that
//!   says one of several sentences has one key for each, the Rust name and a word for the case:
//!   `remote_view.closed_text.timeout`. A key is never renamed or given another meaning once it is
//!   released; a sentence that changes its meaning gets a new key.
//! - **Placeholders** are a value's name in braces, `{name}`: lower-case letters and `_`, the same
//!   names as `params`. A sentence never holds a brace of its own. A value goes in as it is, with
//!   nothing escaped, in one pass, so a value with braces in it is never filled in again ([`fill`]). A time left is written by [`super::countdown`] (minutes, a colon, two digits of
//!   seconds) and seconds as a whole number.
//! - **Hosts**: a sentence shown or true on some hosts only names them in `hosts` ("linux",
//!   "windows", or "mac" for another host that is not in this repository) with a `note` on why; one
//!   with no `hosts` is for every host. A sentence is the same words on every host it names; words
//!   that differ by host are separate keys.
//! - **The file**: [`json`] is `{"version": 1, "keys": {KEY: {"en": TEXT, "params": [NAME, ...],
//!   "hosts": [...], "note": ...}}}`, keys in order, two-space indents and a line end at the end. It
//!   is kept as `wording.json` in this crate's folder, and a test fails when it is not what [`json`]
//!   gives. `GUESTHOUSE_WRITE_GOLDEN=1 cargo test -p guesthouse-remote-view-core --test wording_json`
//!   writes it again. [`VERSION`] goes up only when the file's shape changes, not for new keys.
//!
//! Formatting that is not a sentence (a fingerprint's groups, a device's tag, the seconds until a
//! time) has no key: [`super::short_fingerprint`], [`super::device_tag`], [`super::seconds_until`]
//! and [`super::countdown`] say how it is done.

use super::*;
use serde::Serialize;
use std::collections::BTreeMap;

/// The shape of [`json`].
pub const VERSION: u32 = 1;
/// The file [`json`] is kept in, in this crate's folder.
pub const FILE: &str = "wording.json";
/// What every key starts with.
pub const KEY_PREFIX: &str = "remote_view.";

pub const LINUX: &str = "linux";
pub const WINDOWS: &str = "windows";
/// Another host, which shows some of the sentences too, by the name it goes by in `hosts`.
pub const MAC: &str = "mac";

/// One sentence, with its key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    pub key: String,
    /// The English text, with a `{name}` for each value.
    pub en: String,
    /// The names of the values it takes, in the order they first appear in it.
    pub params: Vec<String>,
    /// The hosts that show it, when not every host.
    pub hosts: Vec<&'static str>,
    /// Why it is for those hosts only.
    pub note: Option<String>,
}

/// The names in braces in `text`, in the order they first appear.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        let name = after[..end].to_string();
        if !names.contains(&name) {
            names.push(name);
        }
        rest = &after[end + 1..];
    }
    names
}

/// `text` with each `{name}` given its value, in one pass from the start, so a value that holds
/// braces is put in as it is and never filled in itself. A placeholder with no value is left as it is.
pub fn fill(text: &str, values: &[(&str, &str)]) -> String {
    let mut filled = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        filled.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            rest = &rest[start..];
            break;
        };
        let name = &after[..end];
        match values.iter().find(|(given, _)| *given == name) {
            Some((_, value)) => filled.push_str(value),
            None => filled.push_str(&rest[start..start + end + 2]),
        }
        rest = &after[end + 1..];
    }
    filled.push_str(rest);
    filled
}

fn one(key: &str, en: impl Into<String>) -> Sentence {
    let en = en.into();
    Sentence {
        key: format!("{KEY_PREFIX}{key}"),
        params: placeholders(&en),
        en,
        hosts: Vec::new(),
        note: None,
    }
}

fn only(sentence: Sentence, hosts: &[&'static str], note: &str) -> Sentence {
    Sentence { hosts: hosts.to_vec(), note: Some(note.to_string()), ..sentence }
}

fn share(state: ShareState) -> ShareInfo {
    ShareInfo { state, ..ShareInfo::default() }
}

fn device(connected: bool) -> DeviceInfo {
    DeviceInfo { fingerprint: String::new(), name: String::new(), connected, permissions: Default::default() }
}

/// Every sentence, in the order [`super`] has them.
pub fn sentences() -> Vec<Sentence> {
    let windows_app = "Shown by the Windows app.";
    let viewing = ShareInfo { viewer: Some("{viewer}".into()), ..share(ShareState::Listening) };
    let failed = ShareInfo { reason: Some("{why}".into()), ..share(ShareState::Failed) };
    let [step_1, step_2, step_3] = pairing_steps();
    let mut all = vec![
        one("allow_title", ALLOW_TITLE),
        one("pairing_title", PAIRING_TITLE),
        one("connect_title", CONNECT_TITLE),
        one("devices_title", DEVICES_TITLE),
        one("network_warning", NETWORK_WARNING),
        one("pairing_note", PAIRING_NOTE),
        one("paired_hint", PAIRED_HINT),
        one("compare_code_line", COMPARE_CODE_LINE),
        one("no_devices", NO_DEVICES),
        one("remove_message", REMOVE_MESSAGE),
        one("accept", ACCEPT),
        one("decline", DECLINE),
        one("stop", STOP),
        one("pair_new_device", PAIR_NEW_DEVICE),
        one("copy_address", COPY_ADDRESS),
        one("disconnect", DISCONNECT),
        one("remove", REMOVE),
        one("headline.off", headline(false, false, &share(ShareState::Off))),
        one("headline.not_running", headline(true, false, &share(ShareState::Off))),
        one("headline.starting", headline(true, true, &share(ShareState::Starting))),
        one("headline.ready", headline(true, true, &share(ShareState::Listening))),
        one("headline.viewing", headline(true, true, &viewing)),
        one("headline.failed", headline(true, true, &failed)),
        one("headline.no_reason", NO_REASON),
        one("pairing_steps.1", step_1),
        one("pairing_steps.2", step_2),
        one("pairing_steps.3", step_3),
        one("closed_text.paired_name", closed_text(Some(CloseReason::Paired), Some("{name}"))),
        one("closed_text.paired", closed_text(Some(CloseReason::Paired), None)),
        one("closed_text.declined", closed_text(Some(CloseReason::Declined), None)),
        one("closed_text.timeout", closed_text(Some(CloseReason::Timeout), None)),
        one("closed_text.closed", closed_text(None, None)),
        one("wants_to_pair", wants_to_pair("{name}")),
        one("remove_question", remove_question("{name}")),
        only(
            one("only_notice", only_notice("{name}")),
            &[LINUX],
            "Says the picture comes back when the device leaves, which is true on the Linux host. The \
             Windows host's own window does not come back by itself, so the Windows app shows \
             remote_view.takes_screen instead.",
        ),
        one("device_status.viewing_now", device_status(&device(true))),
        one("device_status.not_connected", device_status(&device(false))),
        one("device_line", device_line_of("{status}", "{tag}")),
    ];
    for what in crate::share::PERMISSIONS {
        let label = permission_label(what).expect("every permission has a label");
        all.push(one(&format!("permission_label.{what}"), label));
    }
    all.push(one("host_said.nothing", host_said("")));
    let windows = [
        one("remote_view_title", REMOTE_VIEW_TITLE),
        one("address_label", ADDRESS_LABEL),
        one("port_label", PORT_LABEL),
        one("fingerprint_label", FINGERPRINT_LABEL),
        one("pairing_open_title", PAIRING_OPEN_TITLE),
        one("cancel", CANCEL),
        one("held_while_pairing", HELD_WHILE_PAIRING),
        one("bridged_untried", BRIDGED_UNTRIED),
        one("closing", CLOSING),
        one("closes_in", closes_in_at("{time}")),
        one("left_to_answer", left_to_answer_of("{seconds}")),
        one("switch_name", switch_name("{label}", "{device}")),
    ];
    let shown_by_two = "Shown by the Windows app and by another host's app, in the same words.";
    all.extend(windows.into_iter().map(|sentence| {
        let both = [
            "remote_view.address_label",
            "remote_view.fingerprint_label",
            "remote_view.cancel",
            "remote_view.switch_name",
        ];
        if both.contains(&sentence.key.as_str()) {
            only(sentence, &[WINDOWS, MAC], shown_by_two)
        } else {
            only(sentence, &[WINDOWS], windows_app)
        }
    }));
    let one_device = "True on a host whose guest serves one device at a time, its own window included, and \
                      whose window does not connect again by itself: the Windows host. The Linux host says \
                      remote_view.only_notice instead.";
    all.push(only(one("takes_screen", takes_screen("{name}")), &[WINDOWS], one_device));
    all.push(only(one("screen_line", screen_line("{name}")), &[WINDOWS], one_device));
    let read_out = "What a screen reader says on another host, whose app shows the time left as a running \
                    countdown of its own.";
    all.extend([
        only(one("open_time_left", OPEN_TIME_LEFT), &[MAC], read_out),
        only(one("waiting_time_left", WAITING_TIME_LEFT), &[MAC], read_out),
        only(one("code_label", code_label("{code}")), &[MAC], read_out),
        only(
            one("code_time_left", code_time_left_of("{code}", "{seconds}")),
            &[LINUX],
            "The Linux app's line for a waiting device: its code and the seconds left to answer.",
        ),
        only(
            one("spoken_code", spoken_code_of("{digits}")),
            &[WINDOWS],
            "What a screen reader says for the code in the Windows app: the digits one by one, with a space \
             between each.",
        ),
    ]);
    all
}

#[derive(Serialize)]
struct File {
    version: u32,
    keys: BTreeMap<String, Entry>,
}

#[derive(Serialize)]
struct Entry {
    en: String,
    params: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    hosts: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

/// Every sentence as the JSON kept in [`FILE`].
pub fn json() -> String {
    let keys = sentences()
        .into_iter()
        .map(|sentence| {
            let entry = Entry {
                en: sentence.en,
                params: sentence.params,
                hosts: sentence.hosts,
                note: sentence.note,
            };
            (sentence.key, entry)
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&File { version: VERSION, keys }).expect("plain data");
    text.push('\n');
    text
}
