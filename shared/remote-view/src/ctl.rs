//! The words remoteman's host in the guest takes on its `ctl` command, and what it answers.
//!
//! Each function gives the arguments that follow `ctl --port PORT`, one word to a string, ready
//! for a program to be run with and never joined into a command line. A fingerprint is the one
//! thing a person's device supplies, so it is checked to be 64 hex digits before it goes in a
//! word: it is also what runs in PowerShell in the guest.
//!
//! The names follow the actions the other hosts have: `pair_close` is `pair off`, `device_drop`
//! is `drop`, and `device_allow` with `input` is `view-only`, its opposite.

use crate::share::PERMISSIONS;

/// One `ctl` command: the words after the port.
pub type Words = Vec<String>;

/// The length of a fingerprint: SHA-256 of a certificate, in hex.
pub const FINGERPRINT_LENGTH: usize = 64;

/// What is said of a fingerprint that is not one.
pub const NOT_A_FINGERPRINT: &str = "That is not a device's fingerprint.";

/// A fingerprint as the host writes it, in lower case, or why it is not one. The host compares
/// them in lower case, so a fingerprint typed in capitals still names its device.
pub fn check_fingerprint(fingerprint: &str) -> Result<String, String> {
    if fingerprint.len() == FINGERPRINT_LENGTH && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(fingerprint.to_ascii_lowercase());
    }
    Err(NOT_A_FINGERPRINT.to_string())
}

fn words(list: &[&str]) -> Words {
    list.iter().map(|word| (*word).to_string()).collect()
}

fn word(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// `status`: where the host stands, as one line of JSON.
pub fn status() -> Words {
    words(&["status"])
}

/// `pair on`: lets the next unknown device come and wait for an answer. The host puts no time on
/// it, so whoever opens it also closes it.
pub fn pair_open() -> Words {
    words(&["pair", "on"])
}

/// `pair on SECONDS`: the same, for a host that keeps pairing's time itself and closes it when the
/// time is up. remoteman's own host takes `pair on` with no time; a host that keeps the time takes
/// these words, or its own for them.
pub fn pair_open_for(seconds: u64) -> Words {
    words(&["pair", "on", &seconds.to_string()])
}

/// `pair off`: closes pairing. A device that is waiting is declined by it.
pub fn pair_close() -> Words {
    words(&["pair", "off"])
}

/// `pair accept`: trusts the device waiting.
pub fn pair_accept() -> Words {
    words(&["pair", "accept"])
}

/// `pair decline`: turns the device waiting away.
pub fn pair_decline() -> Words {
    words(&["pair", "decline"])
}

/// `revoke FINGERPRINT`: stops trusting a device, which is disconnected and refused from then on.
pub fn device_revoke(fingerprint: &str) -> Result<Words, String> {
    let fingerprint = check_fingerprint(fingerprint)?;
    Ok(vec!["revoke".to_string(), fingerprint])
}

/// `drop`: disconnects the viewer now. It stays trusted. A host with a viewer of Guesthouse's own
/// in it must not send this for a person's request, as it would cut that viewer.
pub fn device_drop() -> Words {
    words(&["drop"])
}

/// `sharing on` or `sharing off`: off turns every device away, declines one that waits, drops the
/// one connected, and is remembered across restarts. A host that serves Guesthouse's own viewer
/// must not send it.
pub fn share(on: bool) -> Words {
    words(&["sharing", word(on)])
}

/// `set updates off`: the host never looks for a newer remoteman by itself, and never fetches one.
/// Guesthouse pins the release it puts in a guest and updates it itself, so a guest doesn't reach a
/// download site on its own. The host keeps it in `updates.json` in its state folder, across restarts
/// and installer runs. Hosts before 0.1.9 don't know it.
pub fn updates_off() -> Words {
    words(&["set", "updates", "off"])
}

/// Whether the host took [`updates_off`]: only a plain `ok` does, as that is when it was written
/// down. Otherwise the host's words for why not: an older host's list of what it knows, a host
/// started with `--updates`, or one that couldn't keep it.
pub fn updates_off_took(answer: &str) -> Result<(), String> {
    match parse_ack(answer) {
        Ack::Done(line) if line == "ok" => Ok(()),
        Ack::Done(line) | Ack::NotKept(line) | Ack::Refused(line) => {
            Err(if line.is_empty() { "the host said nothing".to_string() } else { line })
        }
        Ack::Stopping => Err(STOPPING.to_string()),
    }
}

/// `allow clipboard on|off` or `allow files on|off`: what the host serves every device, before
/// what each device may do.
pub fn host_allow(what: &str, on: bool) -> Result<Words, String> {
    match what {
        "clipboard" | "files" => Ok(words(&["allow", what, word(on)])),
        _ => Err(format!("{what} is not something the whole host can be allowed.")),
    }
}

/// `device FINGERPRINT allow WHAT on|off`, or for `input` `device FINGERPRINT view-only on|off`,
/// which is the opposite: control off is view-only on.
pub fn device_allow(fingerprint: &str, what: &str, on: bool) -> Result<Words, String> {
    let fingerprint = check_fingerprint(fingerprint)?;
    if !PERMISSIONS.contains(&what) {
        return Err(format!("{what} is not something a device can be allowed."));
    }
    let mut line = vec!["device".to_string(), fingerprint];
    if what == "input" {
        line.extend(words(&["view-only", word(!on)]));
    } else {
        line.extend(words(&["allow", what, word(on)]));
    }
    Ok(line)
}

/// What the host said to a command other than `status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ack {
    /// It did what was asked. A command that shows something, as `device` does, is this too, and
    /// the line is kept.
    Done(String),
    /// It did it, but could not keep it for the next start. The text is its own.
    NotKept(String),
    /// It would not, or could not: "no device is waiting", "no such device", and the like.
    Refused(String),
    /// The host is shutting down, and has no answer.
    Stopping,
}

/// What the host says when it has gone and left the pipe.
pub const STOPPING: &str = "the host is stopping";

/// Reads the host's answer to a command. Only `ok`, a line that begins `ok:` and a line that
/// describes a device (`NAME (FP): ...`, which `device` answers with) count as done: everything
/// else is the host's words for why not, and is kept as they are.
pub fn parse_ack(answer: &str) -> Ack {
    let line = answer.trim();
    if line.eq_ignore_ascii_case(STOPPING) {
        return Ack::Stopping;
    }
    if line == "ok" || line.starts_with("ok:") {
        return Ack::Done(line.to_string());
    }
    if line.starts_with("done, but it won't be remembered") {
        return Ack::NotKept(line.to_string());
    }
    if describes_a_device(line) {
        return Ack::Done(line.to_string());
    }
    Ack::Refused(line.to_string())
}

/// `NAME (FINGERPRINT-START): can control this PC; clipboard allowed, ...`
fn describes_a_device(line: &str) -> bool {
    line.contains("): ") && (line.contains("can control this PC") || line.contains("view only"))
}

#[cfg(test)]
mod tests;
