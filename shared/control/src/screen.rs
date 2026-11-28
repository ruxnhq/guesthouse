//! The app's side of a guest's screen. remoteman's viewer runs as the user, and its `connect`
//! command takes all it needs on its command line: the guest's address, the fingerprint its host
//! must have, the identity the guest trusts, and where its picture goes. So the app only writes the
//! identity the service gave it into files, in a state folder of the user's, and starts the viewer
//! with that command. What remoteman keeps in its own folder stays remoteman's business, and nobody
//! is asked to pair.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::Path;

/// Bytes as hex, as the pipe carries a certificate and a key.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Hex back to bytes, or nothing if it isn't hex.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
}

/// Where the viewer's picture goes in the guest: its display becomes the guest's main one, where the
/// taskbar and new windows go. A guest has a display of its own too, the one its console shows,
/// which stays on as a second display.
const PLACEMENT: &str = "main";

/// Switches the guest's sound and clipboard on in the viewer's own settings in `state`, as remoteman's
/// viewer keeps them (`viewer-settings`, for computers it meets from now on, and `viewer-computers`,
/// one `[computer]` section for each it knows, `key=value` lines): a session with the computer at
/// `address` then starts with its sound played here and its clipboard shared. Everything else the
/// viewer saved stays as it is, lines it doesn't know included. Files need no switch of the viewer's.
fn media_on(state: &Path, address: &str) -> io::Result<()> {
    fs::create_dir_all(state)?;
    let read = |name: &str| fs::read_to_string(state.join(name)).unwrap_or_default();
    let settings = set_lines(&read("viewer-settings"), &[("sound", "yes"), ("clipboard", "yes")]);
    fs::write(state.join("viewer-settings"), settings)?;
    let computers = read("viewer-computers");
    if computers.lines().any(|line| line.strip_prefix("address=") == Some(address)) {
        let mut out = String::new();
        let mut section: Vec<&str> = Vec::new();
        let flush = |section: &mut Vec<&str>, out: &mut String| {
            if section.iter().any(|line| line.strip_prefix("address=") == Some(address)) {
                let text = section.join("\n") + "\n";
                out.push_str(&set_lines(&text, &[("sound", "yes"), ("clipboard", "yes")]));
            } else if !section.is_empty() {
                out.push_str(&(section.join("\n") + "\n"));
            }
            section.clear();
        };
        for line in computers.lines() {
            if line == "[computer]" {
                flush(&mut section, &mut out);
            }
            section.push(line);
        }
        flush(&mut section, &mut out);
        fs::write(state.join("viewer-computers"), out)?;
    }
    Ok(())
}

/// `text`'s `key=value` lines with these keys set, each in place if it's there and added at the end
/// if not; every other line as it was.
fn set_lines(text: &str, set: &[(&str, &str)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    for (key, value) in set {
        let line = format!("{key}={value}");
        match lines.iter_mut().find(|l| l.split_once('=').is_some_and(|(k, _)| k == *key)) {
            Some(found) => *found = line,
            None => lines.push(line),
        }
    }
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// Writes the viewer's identity into `state\identity`, switches the guest's sound and clipboard on
/// in the viewer's settings there, and gives the arguments that start remoteman's viewer on the
/// guest's screen, `connect ADDRESS:PORT --expect … --identity … --placement main --sound --state
/// STATE`. `--sound` is the viewer's own switch for a session's sound; the guest's host decides
/// whether its clipboard and files are on.
pub fn connect_command(
    state: &Path,
    address: &str,
    port: u16,
    host_fingerprint: &str,
    certificate: &str,
    key: &str,
) -> io::Result<Vec<OsString>> {
    let bad =
        |what: &str| io::Error::new(io::ErrorKind::InvalidData, format!("The service's {what} isn't right."));
    let fingerprint_ok =
        host_fingerprint.len() == 64 && host_fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !fingerprint_ok {
        return Err(bad("host fingerprint"));
    }
    let certificate = unhex(certificate).ok_or_else(|| bad("certificate"))?;
    let key = unhex(key).ok_or_else(|| bad("key"))?;
    let identity = state.join("identity");
    fs::create_dir_all(&identity)?;
    let (certificate_file, key_file) = (identity.join("cert.der"), identity.join("key.der"));
    fs::write(&certificate_file, certificate)?;
    fs::write(&key_file, key)?;
    media_on(state, &format!("{address}:{port}"))?;
    Ok(vec![
        "connect".into(),
        format!("{address}:{port}").into(),
        "--expect".into(),
        host_fingerprint.into(),
        "--identity".into(),
        certificate_file.into(),
        key_file.into(),
        "--placement".into(),
        PLACEMENT.into(),
        "--sound".into(),
        "--state".into(),
        state.into(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_goes_there_and_back() {
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
        assert_eq!(unhex("00abff"), Some(vec![0, 0xab, 0xff]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }

    #[test]
    fn the_command_names_the_guest_and_carries_the_identity_in_files() {
        let state = std::env::temp_dir().join(format!("gh-viewer-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        let fingerprint = "ab".repeat(32);
        let args = connect_command(&state, "172.29.1.2", 7791, &fingerprint, "0102", "0304").unwrap();
        let (certificate, key) =
            (state.join("identity").join("cert.der"), state.join("identity").join("key.der"));
        let expected: Vec<OsString> = vec![
            "connect".into(),
            "172.29.1.2:7791".into(),
            "--expect".into(),
            fingerprint.clone().into(),
            "--identity".into(),
            certificate.clone().into(),
            key.clone().into(),
            "--placement".into(),
            "main".into(),
            "--sound".into(),
            "--state".into(),
            state.clone().into(),
        ];
        assert_eq!(args, expected);
        assert_eq!(fs::read(certificate).unwrap(), [1, 2]);
        assert_eq!(fs::read(key).unwrap(), [3, 4]);
        let _ = fs::remove_dir_all(state);
    }

    #[test]
    fn the_viewers_settings_get_sound_and_clipboard_and_keep_the_rest() {
        let state = std::env::temp_dir().join(format!("gh-viewer-media-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("viewer-settings"), "placement=main\nsound=no\nfuture-key=x\n").unwrap();
        let computers = "[computer]\nname=other\naddress=10.0.0.9:7791\nsound=no\nclipboard=no\n\
                         [computer]\nname=guest\naddress=172.29.1.2:7791\nsound=no\nclipboard=no\nwake=yes\nodd=1\n";
        fs::write(state.join("viewer-computers"), computers).unwrap();
        let fingerprint = "ab".repeat(32);
        connect_command(&state, "172.29.1.2", 7791, &fingerprint, "0102", "0304").unwrap();
        assert_eq!(
            fs::read_to_string(state.join("viewer-settings")).unwrap(),
            "placement=main\nsound=yes\nfuture-key=x\nclipboard=yes\n"
        );
        assert_eq!(
            fs::read_to_string(state.join("viewer-computers")).unwrap(),
            "[computer]\nname=other\naddress=10.0.0.9:7791\nsound=no\nclipboard=no\n\
             [computer]\nname=guest\naddress=172.29.1.2:7791\nsound=yes\nclipboard=yes\nwake=yes\nodd=1\n"
        );
        // Again: nothing changes.
        connect_command(&state, "172.29.1.2", 7791, &fingerprint, "0102", "0304").unwrap();
        assert_eq!(fs::read_to_string(state.join("viewer-computers")).unwrap().matches("yes").count(), 3);
        let _ = fs::remove_dir_all(state);
    }

    #[test]
    fn a_fingerprint_that_isnt_one_is_refused() {
        let state = std::env::temp_dir().join(format!("gh-viewer-bad-{}", std::process::id()));
        assert!(connect_command(&state, "172.29.1.2", 7791, "ff", "0102", "0304").is_err());
        assert!(connect_command(&state, "172.29.1.2", 7791, &"zz".repeat(32), "0102", "0304").is_err());
        assert!(!state.exists(), "nothing is written for a bad answer");
    }
}
