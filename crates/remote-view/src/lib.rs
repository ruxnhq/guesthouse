//! A Windows guest's screen, through remoteman: its host runs in the guest and encodes the guest's
//! desktop on the guest's card, and its viewer runs on this PC. Guesthouse holds both ends, so it
//! pairs them itself and nobody is asked: it makes the viewer's identity, tells the guest to trust
//! only that, and learns the guest's own fingerprint from what the guest says on its serial port.
//!
//! The viewer's identity is kept beside the machine's disk, in `NAME.screen\client`, and the
//! service hands it to the app, which gives it to remoteman's viewer with its `connect` command.
//! Everything else remoteman keeps is remoteman's own.
//!
//! Beyond that, this is everything about viewing a machine from other devices that needs no
//! system, no network and no clock of its own, so each host builds on one implementation: the
//! state a screen shows (`share`), the words remoteman's host takes and how it answers (`ctl`,
//! `status`), pairing as a state machine (`pairing`, `policy`), what a screen makes of them
//! (`view`), the trust list across the installer's runs (`trust`), the switch and its port
//! (`switch`) and the sentences (`wording`).
//!
//! Nothing here calls the system or a hypervisor, so every host can build on it.

pub mod announce;
pub mod ctl;
pub mod install;
pub mod pairing;
pub mod policy;
pub mod share;
pub mod status;
pub mod switch;
pub mod trust;
pub mod view;
pub mod wording;

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// The port remoteman's host listens on.
pub const PORT: u16 = 7791;
/// remoteman's host's installer, from remoteman's release, which the host looks for beside the
/// program that boots the machine. The guest runs it unattended, with the viewer to trust, the
/// encoder for its card, and no pairing.
pub const SETUP: &str = r"screen\remoteman-host-setup.exe";
/// What the guest finds it as, in its screen folder.
pub const GUEST_SETUP: &str = "remoteman-host-setup.exe";
/// The guest's folder for its screen, from the root of its Windows volume.
pub const GUEST_FOLDER: &str = r"Guesthouse\screen";
/// The script the guest runs as SYSTEM to install remoteman's host and say its fingerprint.
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
    fs::write(&key_path, made.signing_key.serialize_der())
        .map_err(|error| format!("{}: {error}", key_path.display()))?;
    let certificate = made.cert.der().to_vec();
    fs::write(&cert_path, &certificate).map_err(|error| format!("{}: {error}", cert_path.display()))?;
    Ok(fingerprint(&certificate))
}

/// The guest's trust list: the viewer, and nobody else.
pub fn trust_list(viewer: &str) -> String {
    format!("{viewer} Guesthouse\n")
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
        for name in [GUEST_SETUP, "trusted", "encoder.txt", "logon.txt", r"C:\Guesthouse\screen"] {
            assert!(SCRIPT.contains(name), "{name}");
        }
        assert!(SETUP.ends_with(GUEST_SETUP));
        assert!(SCRIPT.contains("screen: fingerprint"));
    }

    #[test]
    fn the_guest_script_switches_remotemans_own_update_check_off_with_the_shared_words() {
        let line = format!("ctl --port {PORT} {}", ctl::updates_off().join(" "));
        assert!(SCRIPT.contains(&line), "{line}");
        // Taken only on a plain `ok`, as ctl::updates_off_took reads it.
        assert!(SCRIPT.contains("if ($said -eq 'ok') { break }"));
        assert_eq!(ctl::updates_off_took("ok"), Ok(()));
    }
}
