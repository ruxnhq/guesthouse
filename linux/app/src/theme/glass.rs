//! Whether the compositor blurs what is behind a window, asked of it over its own socket. Tide leaves
//! its frame and sidebar see-through only then: unblurred, the wallpaper through them is noise.
//!
//! Hyprland says through `getoption`, which covers its own blur and the glass plugins on top of it.
//! SwayFX answers sway's IPC with a version that has `sway_original_version` in it, and blurs once
//! its config says `blur enable`. Any other compositor is taken not to blur.

use serde_json::Value;
use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// How long a compositor gets to answer. It's asked while the app starts.
const ANSWER_WAIT: Duration = Duration::from_millis(300);
const HYPRLAND_SIGNATURE: &str = "HYPRLAND_INSTANCE_SIGNATURE";
const HYPRLAND_BLUR: &str = "j/getoption decoration:blur:enabled";
const SWAY_SOCKET: &str = "SWAYSOCK";
/// sway's IPC: a message starts with this, then the payload's length and the message's type.
const I3_MAGIC: &[u8; 6] = b"i3-ipc";
const GET_VERSION: u32 = 7;
const GET_CONFIG: u32 = 9;
/// What only SwayFX's version answer has.
const SWAYFX_MARK: &str = "sway_original_version";
const SWAYFX_BLUR_ON: &str = "blur enable";

/// Whether the compositor this app runs on blurs behind windows.
pub fn compositor_blurs() -> bool {
    hyprland_blurs().or_else(swayfx_blurs).unwrap_or(false)
}

fn connect(path: PathBuf) -> Option<UnixStream> {
    let stream = UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(ANSWER_WAIT)).ok()?;
    stream.set_write_timeout(Some(ANSWER_WAIT)).ok()?;
    Some(stream)
}

fn hyprland_blurs() -> Option<bool> {
    let signature = env::var_os(HYPRLAND_SIGNATURE)?;
    let runtime = env::var_os("XDG_RUNTIME_DIR")?;
    let socket = PathBuf::from(runtime).join("hypr").join(signature).join(".socket.sock");
    let mut stream = connect(socket)?;
    stream.write_all(HYPRLAND_BLUR.as_bytes()).ok()?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;
    Some(hyprland_says_blur(&answer))
}

fn hyprland_says_blur(answer: &str) -> bool {
    let value: Value = serde_json::from_str(answer).unwrap_or_default();
    value.get("int").and_then(Value::as_i64).is_some_and(|on| on != 0)
}

fn i3_message(stream: &mut UnixStream, kind: u32) -> Option<Value> {
    let mut message = I3_MAGIC.to_vec();
    message.extend_from_slice(&0u32.to_ne_bytes());
    message.extend_from_slice(&kind.to_ne_bytes());
    stream.write_all(&message).ok()?;
    let mut header = [0u8; 14];
    stream.read_exact(&mut header).ok()?;
    let length = u32::from_ne_bytes(header[6..10].try_into().ok()?) as usize;
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload).ok()?;
    serde_json::from_slice(&payload).ok()
}

fn swayfx_blurs() -> Option<bool> {
    let mut stream = connect(PathBuf::from(env::var_os(SWAY_SOCKET)?))?;
    let version = i3_message(&mut stream, GET_VERSION)?;
    if version.get(SWAYFX_MARK).is_none() {
        return Some(false);
    }
    let config = i3_message(&mut stream, GET_CONFIG)?;
    Some(config_turns_blur_on(config.get("config")?.as_str()?))
}

fn config_turns_blur_on(config: &str) -> bool {
    let words = |line: &str| {
        line.split('#').next().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ")
    };
    config.lines().any(|line| words(line) == SWAYFX_BLUR_ON)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyprland_blurs_when_its_option_is_on() {
        assert!(hyprland_says_blur(r#"{"option": "decoration:blur:enabled", "int": 1, "set": true}"#));
        assert!(!hyprland_says_blur(r#"{"option": "decoration:blur:enabled", "int": 0, "set": true}"#));
        assert!(!hyprland_says_blur("no such option"));
    }

    #[test]
    fn swayfx_blurs_when_its_config_says_so() {
        assert!(config_turns_blur_on("output * bg x fill\nblur  enable\n"));
        assert!(!config_turns_blur_on("# blur enable\nblur disable\n"));
        assert!(!config_turns_blur_on(""));
    }
}
