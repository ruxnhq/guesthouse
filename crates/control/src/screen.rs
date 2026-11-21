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

/// Writes the viewer's identity into `state\identity` and gives the arguments that start remoteman's
/// viewer on the guest's screen, `connect ADDRESS:PORT --expect … --identity … --placement main
/// --state STATE`.
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
            "--state".into(),
            state.clone().into(),
        ];
        assert_eq!(args, expected);
        assert_eq!(fs::read(certificate).unwrap(), [1, 2]);
        assert_eq!(fs::read(key).unwrap(), [3, 4]);
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
