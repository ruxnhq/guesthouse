//! The app's side of a guest's screen: the remoteman viewer runs as the user, with a state folder of
//! the user's, so the app writes what the service's `Screen` answer says into that folder, in
//! remoteman's own layout, and starts the viewer with it. Then the viewer proves itself with the
//! identity the guest trusts, and expects the guest's own, and nobody is asked to pair.

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

/// Writes a viewer's state folder: its identity, `client\cert.der` and `client\key.der`, and the
/// host it expects at `address:port`, `client\hosts\ADDRESS_PORT`, the name remoteman gives it.
/// Gives the address to start the viewer with.
pub fn viewer_state(
    state: &Path,
    address: &str,
    port: u16,
    host_fingerprint: &str,
    certificate: &str,
    key: &str,
) -> io::Result<String> {
    let bad =
        |what: &str| io::Error::new(io::ErrorKind::InvalidData, format!("The service's {what} isn't hex."));
    let certificate = unhex(certificate).ok_or_else(|| bad("certificate"))?;
    let key = unhex(key).ok_or_else(|| bad("key"))?;
    let client = state.join("client");
    let hosts = client.join("hosts");
    fs::create_dir_all(&hosts)?;
    fs::write(client.join("cert.der"), certificate)?;
    fs::write(client.join("key.der"), key)?;
    let target = format!("{address}:{port}");
    fs::write(hosts.join(target.replace([':', '/'], "_")), host_fingerprint)?;
    Ok(target)
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
    fn a_viewer_state_is_remotemans_layout() {
        let state = std::env::temp_dir().join(format!("gh-viewer-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        let target = viewer_state(&state, "172.29.1.2", 7791, "ff", "0102", "0304").unwrap();
        assert_eq!(target, "172.29.1.2:7791");
        assert_eq!(fs::read(state.join("client").join("cert.der")).unwrap(), [1, 2]);
        assert_eq!(fs::read(state.join("client").join("key.der")).unwrap(), [3, 4]);
        assert_eq!(
            fs::read_to_string(state.join("client").join("hosts").join("172.29.1.2_7791")).unwrap(),
            "ff"
        );
        let _ = fs::remove_dir_all(state);
    }
}
