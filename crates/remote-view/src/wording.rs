//! The sentences every host shows about viewing from other devices, as functions of the state.
//!
//! The words are the same on each host, so a person who moves between them reads the same thing.
//! Where a host needs more than the common words, the function says so, and where a sentence is
//! true on one host only, its doc says which. Sentences that differ by host are kept apart, each
//! under its own name; none is made to fit another host.
//!
//! The same sentences are data too, for an app that cannot call Rust: [`data`] gives each a key that
//! never changes, and `wording.json` beside this crate holds them all, made from these functions.

use crate::share::{CloseReason, DeviceInfo, ShareInfo, ShareState};

/// The title of the switch, and of its card.
pub const ALLOW_TITLE: &str = "Allow viewing from other devices";
pub const PAIRING_TITLE: &str = "Pairing";
pub const CONNECT_TITLE: &str = "Connect from remoteman";
pub const DEVICES_TITLE: &str = "Devices that can view this machine";

/// The reminder about the network, said once beside the switch.
pub const NETWORK_WARNING: &str =
    "Devices on the same network can reach this machine's screen once they are paired.";
/// Said under the cards.
pub const PAIRING_NOTE: &str = "Only devices you accepted here can connect. Anyone on the same network can ask to pair while pairing is open.";

/// How a device already paired connects.
pub const PAIRED_HINT: &str = "A device that is already paired connects to the address above. To add another device, press Pair a New Device.";
/// The line for the check that has to be made before Accept.
pub const COMPARE_CODE_LINE: &str =
    "Check that this code matches the one shown on the device before you press Accept.";
/// Shown while the list of devices is empty.
pub const NO_DEVICES: &str = "No device has been paired yet. Press Pair a New Device to add one.";
/// Under the question that asks before a device is removed.
pub const REMOVE_MESSAGE: &str =
    "It will be disconnected and will have to be paired again to view this machine.";

/// The buttons, as every host labels them.
pub const ACCEPT: &str = "Accept";
pub const DECLINE: &str = "Decline";
pub const STOP: &str = "Stop";
pub const PAIR_NEW_DEVICE: &str = "Pair a New Device";
pub const COPY_ADDRESS: &str = "Copy Address";
pub const DISCONNECT: &str = "Disconnect";
pub const REMOVE: &str = "Remove";

/// What a failure says when the host gave no reason.
const NO_REASON: &str = "Something went wrong.";

/// One line on where viewing from other devices stands. `allowed` is the machine's switch and
/// `machine_running` whether the machine is up, since the screen is only shared then.
pub fn headline(allowed: bool, machine_running: bool, share: &ShareInfo) -> String {
    if !allowed {
        return "Off. Nothing is listening for other devices.".to_string();
    }
    if !machine_running {
        return "On. The screen is shared once the machine is running.".to_string();
    }
    match share.state {
        ShareState::Off | ShareState::Starting => "Starting to share the screen\u{2026}".to_string(),
        ShareState::Listening => match &share.viewer {
            Some(viewer) => format!("{viewer} is viewing this machine."),
            None => "Ready. A paired device can connect.".to_string(),
        },
        ShareState::Failed => {
            let why = share.reason.as_deref().unwrap_or(NO_REASON);
            format!("The screen could not be shared: {why}")
        }
    }
}

/// The steps for connecting a new device: pairing is opened here, the device connects to the
/// address and waits, and the person here compares the code and accepts.
pub fn pairing_steps() -> [&'static str; 3] {
    [
        "In remoteman on the other device, add a computer and enter the address above.",
        "If remoteman shows a fingerprint, check that it matches the one above.",
        "The device waits, and its name and a code appear here. Check that the code matches the one on the device, then press Accept. Pairing closes when the device has been answered, or when the time is up.",
    ]
}

/// What to say about pairing while it is closed, by how it last ended.
pub fn closed_text(reason: Option<CloseReason>, paired: Option<&str>) -> String {
    match reason {
        Some(CloseReason::Paired) => match paired {
            Some(name) => format!("Closed. {name} was paired."),
            None => "Closed. A device was paired.".to_string(),
        },
        Some(CloseReason::Declined) => {
            "Closed. The device was turned away. Press Pair a New Device to try again.".to_string()
        }
        Some(CloseReason::Timeout) => "Closed. Nobody paired in time.".to_string(),
        Some(CloseReason::Asked | CloseReason::Other) | None => "Closed".to_string(),
    }
}

/// The card's title line for a device that waits.
pub fn wants_to_pair(name: &str) -> String {
    format!("{name} wants to pair")
}

/// The question before a device is removed.
pub fn remove_question(name: &str) -> String {
    format!("Remove {name}?")
}

/// What the machine's own window says while a device has its screen to itself. True where the
/// picture comes back by itself when the device leaves: the Linux host. Not true on the Windows host,
/// whose own window does not come back by itself, so its app does not show this and says
/// [`takes_screen`] instead.
pub fn only_notice(name: &str) -> String {
    format!(
        "{name} is showing this machine on its own screen only, so there is no picture here. It comes back when {name} leaves."
    )
}

/// "Viewing now" or "Not connected".
pub fn device_status(device: &DeviceInfo) -> &'static str {
    if device.connected { "Viewing now" } else { "Not connected" }
}

/// The first eight hex digits of a device's fingerprint in fours, "9f2c 41ab": enough to tell two
/// devices with the same name apart, which is all a name can be.
pub fn device_tag(fingerprint: &str) -> String {
    let head: Vec<char> = fingerprint.chars().take(8).collect();
    let groups: Vec<String> =
        head.chunks(4).map(|group| group.iter().collect::<String>().to_lowercase()).collect();
    groups.join(" ")
}

/// A device's line under its name: whether it is viewing, and its [`device_tag`].
/// "Viewing now · 9f2c 41ab".
pub fn device_line(device: &DeviceInfo) -> String {
    device_line_of(device_status(device), &device_tag(&device.fingerprint))
}

fn device_line_of(status: &str, tag: &str) -> String {
    format!("{status} \u{b7} {tag}")
}

/// What a device's switch is called, by the name of what it allows. `input` is shown as Control.
/// Every host shows Control. The others are for a host whose viewer really serves them: remoteman's
/// host in a guest does, where a host that serves video and input only has nothing to switch.
pub fn permission_label(what: &str) -> Option<&'static str> {
    Some(match what {
        "input" => "Control",
        "clipboard" => "Clipboard",
        "files" => "Files",
        "sound" => "Sound",
        "mic" => "Microphone",
        "camera" => "Camera",
        _ => return None,
    })
}

/// The host key's fingerprint in its first 16 characters, grouped in fours, which is what fits on
/// a line. The whole one is for the tooltip and for copying.
pub fn short_fingerprint(fingerprint: &str) -> String {
    let head: Vec<char> = fingerprint.chars().take(16).collect();
    let groups: Vec<String> = head.chunks(4).map(|group| group.iter().collect()).collect();
    let more = if fingerprint.chars().count() > 16 { "\u{2026}" } else { "" };
    format!("{}{more}", groups.join(" "))
}

/// Whole seconds until a time given in milliseconds since 1970, or `None` when it is not ahead.
pub fn seconds_until(until_ms: u64, now_ms: u64) -> Option<u64> {
    until_ms.checked_sub(now_ms).filter(|left| *left > 0).map(|left| left.div_ceil(1000))
}

/// Whole seconds until an open pairing ends, or `None` when it is not open or has no end.
pub fn pairing_seconds_left(share: &ShareInfo, now_ms: u64) -> Option<u64> {
    if !share.pairing {
        return None;
    }
    seconds_until(share.pairing_until_ms?, now_ms)
}

/// The host's own wording of the line a refused or failed `ctl` command should show: the text,
/// without the shell's line ends. A host that said nothing gets a sentence.
pub fn host_said(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() { "The host said nothing.".to_string() } else { text.to_string() }
}

/// The heading of the Windows app's group for viewing from other devices.
pub const REMOTE_VIEW_TITLE: &str = "Remote View";
/// The labels of the Windows app's rows for where to connect.
pub const ADDRESS_LABEL: &str = "Address";
pub const PORT_LABEL: &str = "Port";
pub const FINGERPRINT_LABEL: &str = "Fingerprint";
/// The heading of the Windows app's row while pairing is open.
pub const PAIRING_OPEN_TITLE: &str = "Pairing is open";
/// The button that leaves the Windows app's question before a device is removed.
pub const CANCEL: &str = "Cancel";
/// Said by the Windows app while pairing is open or a device waits: the host holds clipboard and
/// files off for every device, the machine's own window on this PC included.
pub const HELD_WHILE_PAIRING: &str = "Clipboard sharing with this PC pauses until pairing closes.";
/// Said by the Windows app under the switch while the machine is bridged.
pub const BRIDGED_UNTRIED: &str = "This machine is bridged. Viewing from other devices is made for the shared \
                                   network, and hasn't been tried bridged yet.";
/// Said by the Windows app once an open pairing's time is up, until the host says it closed.
pub const CLOSING: &str = "Closing\u{2026}";

/// "1:45" for a time left: minutes, a colon, and two digits of seconds, as a countdown reads best.
pub fn countdown(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// The Windows app's countdown of an open pairing, "Closes in 1:45", with the time as
/// [`countdown`] gives it.
pub fn closes_in(seconds: u64) -> String {
    closes_in_at(&countdown(seconds))
}

fn closes_in_at(time: &str) -> String {
    format!("Closes in {time}")
}

/// The time a waiting device has to be answered in, as the Windows app says it.
pub fn left_to_answer(seconds: u64) -> String {
    left_to_answer_of(&seconds.to_string())
}

fn left_to_answer_of(seconds: &str) -> String {
    format!("{seconds} s left to answer")
}

/// Said by the Windows app while a device views the machine. True on a host whose guest serves one
/// device at a time, its own window included, and whose window does not connect again by itself:
/// the Windows host. A host that gives the device the screen to itself and has the picture come
/// back says [`only_notice`] instead.
pub fn takes_screen(name: &str) -> String {
    format!(
        "One device shows this machine at a time, so {name} has its screen and the machine's window here has \
         lost it. Open screen takes it back, and {name}'s view ends. Neither connects again by itself."
    )
}

/// The Windows app's line in the Screen row while a device views the machine. True on the same host
/// as [`takes_screen`].
pub fn screen_line(name: &str) -> String {
    format!("{name} has this machine's screen now. Open screen takes it back.")
}

/// What a device's switch is called for a screen reader, which reads it apart from its row: the
/// switch's [`permission_label`] and the device's name.
pub fn switch_name(label: &str, device: &str) -> String {
    format!("{label} for {device}")
}

pub mod data;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_data;
