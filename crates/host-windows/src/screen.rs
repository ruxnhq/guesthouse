//! A Windows guest's screen, through remoteman: its host runs in the guest and encodes the guest's
//! desktop on the guest's card, and its viewer runs on this PC. Guesthouse holds both ends, so it
//! pairs them itself and nobody is asked: it makes the viewer's identity, tells the guest to trust
//! only that, and learns the guest's own fingerprint from what the guest says on its serial port.
//!
//! A viewer's state folder is remoteman's own layout, so the viewer is started with it as is:
//! `client\cert.der` and `client\key.der` are its identity, and `client\hosts\ADDRESS_PORT` holds
//! the fingerprint of the host it expects there.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// The port remoteman's host listens on.
pub const PORT: u16 = 7791;
/// The installer the guest is given, which the host looks for beside the program that boots it.
pub const SETUP: &str = r"screen\remoteman-host-setup.exe";
/// The guest's folder for its screen, from the root of its Windows volume.
pub const GUEST_FOLDER: &str = r"Guesthouse\screen";
pub const SCRIPT: &str = include_str!("../guest/screen.ps1");
/// What the guest says when its host has made its identity.
const FINGERPRINT_LINE: &str = "guesthouse: screen: fingerprint ";

/// The fingerprint of a certificate, as remoteman writes one: SHA-256 of its DER, in lower-case hex.
pub fn fingerprint(certificate: &[u8]) -> String {
    Sha256::digest(certificate).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The viewer's identity in its state folder, made the first time, as remoteman makes its own: a
/// self-signed certificate and its key, PKCS#8. Gives its fingerprint.
pub fn viewer_identity(state: &Path) -> Result<String, String> {
    let client = state.join("client");
    let (cert_path, key_path) = (client.join("cert.der"), client.join("key.der"));
    if let (Ok(certificate), true) = (fs::read(&cert_path), key_path.is_file()) {
        return Ok(fingerprint(&certificate));
    }
    fs::create_dir_all(&client).map_err(|error| format!("{}: {error}", client.display()))?;
    let made = rcgen::generate_simple_self_signed(vec!["remoteman".to_owned()])
        .map_err(|error| format!("Making the viewer's identity: {error}"))?;
    fs::write(&key_path, made.key_pair.serialize_der())
        .map_err(|error| format!("{}: {error}", key_path.display()))?;
    let certificate = made.cert.der().to_vec();
    fs::write(&cert_path, &certificate).map_err(|error| format!("{}: {error}", cert_path.display()))?;
    Ok(fingerprint(&certificate))
}

/// The guest's trust list: the viewer, and nobody else.
pub fn trust_list(viewer: &str) -> String {
    format!("{viewer} Guesthouse\n")
}

/// Tells the viewer which host to expect at `address`, by the name remoteman gives the file.
pub fn expect_host(state: &Path, address: &str, host: &str) -> Result<PathBuf, String> {
    let hosts = state.join("client").join("hosts");
    fs::create_dir_all(&hosts).map_err(|error| format!("{}: {error}", hosts.display()))?;
    let path = hosts.join(format!("{address}:{PORT}").replace([':', '/'], "_"));
    fs::write(&path, host).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

/// The host's fingerprint, if `line` is the guest saying it.
pub fn fingerprint_in(line: &str) -> Option<String> {
    let rest = line.strip_prefix(FINGERPRINT_LINE)?.trim();
    (rest.len() == 64 && rest.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| rest.to_ascii_lowercase())
}

/// The encoder remoteman's host should use with a card, by the card's device ID. NVIDIA's own
/// encoder is found in a guest; the others' Media Foundation encoders aren't registered in one, but
/// Direct3D 12's video encoding works for AMD and Intel alike.
pub fn encoder_for(instance_id: &str) -> &'static str {
    if instance_id.to_ascii_uppercase().contains("VEN_10DE") { "auto" } else { "d3d12" }
}

/// The viewer's state folder for a machine whose disk is `disk`, beside it.
pub fn state_beside(disk: &Path) -> PathBuf {
    disk.with_extension("screen")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_is_the_sha256_of_the_certificate_in_lower_case_hex() {
        assert_eq!(fingerprint(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn the_viewers_identity_is_made_once_and_kept() {
        let state = std::env::temp_dir().join(format!("gh-screen-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        let first = viewer_identity(&state).unwrap();
        assert_eq!(viewer_identity(&state).unwrap(), first);
        assert_eq!(first, fingerprint(&fs::read(state.join(r"client/cert.der")).unwrap()));
        assert!(state.join("client").join("key.der").is_file());
        let expected = expect_host(&state, "172.29.1.2", &first).unwrap();
        assert_eq!(expected.file_name().unwrap(), "172.29.1.2_7791");
        let _ = fs::remove_dir_all(state);
    }

    #[test]
    fn the_guests_fingerprint_is_read_from_its_line_and_nothing_else() {
        let fingerprint = "AB".repeat(32);
        assert_eq!(fingerprint_in(&format!("{FINGERPRINT_LINE}{fingerprint}")), Some("ab".repeat(32)));
        assert_eq!(fingerprint_in(&format!("{FINGERPRINT_LINE}nothex")), None);
        assert_eq!(fingerprint_in("guesthouse: address: 172.29.1.2"), None);
    }

    #[test]
    fn the_encoder_suits_the_cards_maker() {
        assert_eq!(encoder_for(r"PCI\VEN_10DE&DEV_2B85"), "auto");
        assert_eq!(encoder_for(r"PCI\VEN_1002&DEV_13C0"), "d3d12");
        assert_eq!(encoder_for(r"PCI\VEN_8086&DEV_7D55"), "d3d12");
    }

    #[test]
    fn the_guest_script_uses_the_names_the_host_writes() {
        for name in
            ["remoteman-host-setup.exe", "trusted", "encoder.txt", "logon.txt", r"C:\Guesthouse\screen"]
        {
            assert!(SCRIPT.contains(name), "{name}");
        }
        assert!(SETUP.ends_with("remoteman-host-setup.exe"));
        assert!(SCRIPT.contains("screen: fingerprint"));
    }
}
