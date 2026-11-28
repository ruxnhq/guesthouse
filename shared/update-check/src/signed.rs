//! Ed25519 signatures over the fields of a small file a release publishes, so a host only takes the
//! file when a release key, whose public half is built into the host, signed exactly those fields.
//! The file can then be served from anywhere: whoever serves it can't change what it says.
//!
//! What is signed is plain text, not the file's bytes, so the file's layout (its spaces, the order
//! of its keys) can change freely: the purpose, a line naming which file it is, then each field on
//! a line of its own. A field with a line break in it is never signed or taken, so the text has one
//! reading only. Keys and signatures are written in lowercase hex.

use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

/// The length of a key's private half (its seed) and of its public half, in bytes.
pub const KEY_BYTES: usize = 32;
/// The length of a signature, in bytes.
pub const SIGNATURE_BYTES: usize = 64;

/// The text that is signed for `purpose` and `fields`, or None if a field (or the purpose) has a line
/// break in it.
pub fn signed_text(purpose: &str, fields: &[&str]) -> Option<String> {
    let mut text = String::new();
    for line in std::iter::once(&purpose).chain(fields) {
        if line.contains(['\n', '\r']) {
            return None;
        }
        text.push_str(line);
        text.push('\n');
    }
    Some(text)
}

/// Whether `signature` (hex) is a signature of `purpose` and `fields` by one of `keys` (each the
/// public half, in hex). A key that isn't 32 bytes of hex is skipped; with no key, nothing is taken.
pub fn verify(keys: &[&str], purpose: &str, fields: &[&str], signature: &str) -> bool {
    let Some(text) = signed_text(purpose, fields) else { return false };
    let Some(signature) = from_hex::<SIGNATURE_BYTES>(signature) else { return false };
    keys.iter()
        .filter_map(|key| from_hex::<KEY_BYTES>(key))
        .any(|key| UnparsedPublicKey::new(&ED25519, key).verify(text.as_bytes(), &signature).is_ok())
}

/// The signature (hex) of `purpose` and `fields` by the key whose private half is `seed` (hex).
pub fn sign(seed: &str, purpose: &str, fields: &[&str]) -> Result<String, String> {
    let text = signed_text(purpose, fields).ok_or("A field to sign has a line break in it.")?;
    let pair = pair_of(seed)?;
    Ok(to_hex(pair.sign(text.as_bytes()).as_ref()))
}

/// The public half (hex) of the key whose private half is `seed` (hex).
pub fn public_key(seed: &str) -> Result<String, String> {
    Ok(to_hex(pair_of(seed)?.public_key().as_ref()))
}

/// A new key: its private half and its public half, each in hex. For a release key, made once and
/// kept apart from the repo, and for the tests, which make one each run.
pub fn new_key() -> Result<(String, String), String> {
    let mut seed = [0u8; KEY_BYTES];
    SystemRandom::new().fill(&mut seed).map_err(|_| "The system gave no random bytes.")?;
    let seed = to_hex(&seed);
    let public = public_key(&seed)?;
    Ok((seed, public))
}

fn pair_of(seed: &str) -> Result<Ed25519KeyPair, String> {
    let seed = from_hex::<KEY_BYTES>(seed.trim()).ok_or("The signing key isn't 32 bytes in hex.")?;
    Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| "The signing key isn't an Ed25519 key.".to_owned())
}

fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let bytes = text.as_bytes();
    if bytes.len() != N * 2 {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = [0u8; N];
    for (byte, [high, low]) in out.iter_mut().zip(bytes.as_chunks::<2>().0) {
        *byte = digit(*high)? << 4 | digit(*low)?;
    }
    Some(out)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PURPOSE: &str = "guesthouse test file";

    #[test]
    fn a_signature_is_taken_only_for_the_fields_and_key_it_was_made_with() {
        let (seed, public) = new_key().unwrap();
        let (_, other) = new_key().unwrap();
        let fields = ["0.14.0", "https://files.ruxn.com/p/guesthouse/x.exe"];
        let signature = sign(&seed, PURPOSE, &fields).unwrap();
        assert!(verify(&[&public], PURPOSE, &fields, &signature));
        assert!(verify(&[&other, &public], PURPOSE, &fields, &signature), "any one of the keys");
        assert!(!verify(&[&other], PURPOSE, &fields, &signature), "another key");
        assert!(!verify(&[], PURPOSE, &fields, &signature), "no key");
        assert!(!verify(&[&public], "another file", &fields, &signature), "another purpose");
        assert!(!verify(&[&public], PURPOSE, &["0.14.1", fields[1]], &signature), "a changed field");
        assert!(!verify(&[&public], PURPOSE, &fields[..1], &signature), "a field left out");
        let mut flipped = signature.clone().into_bytes();
        flipped[0] = if flipped[0] == b'0' { b'1' } else { b'0' };
        assert!(!verify(&[&public], PURPOSE, &fields, std::str::from_utf8(&flipped).unwrap()));
        assert!(!verify(&[&public], PURPOSE, &fields, ""));
        assert!(!verify(&[&public], PURPOSE, &fields, &"zz".repeat(SIGNATURE_BYTES)));
    }

    #[test]
    fn fields_split_differently_are_not_the_same_text() {
        assert_ne!(signed_text(PURPOSE, &["ab", "c"]), signed_text(PURPOSE, &["a", "bc"]));
        assert_eq!(signed_text(PURPOSE, &["a\nb"]), None);
        assert_eq!(signed_text(PURPOSE, &["a\rb"]), None);
        let (seed, public) = new_key().unwrap();
        assert!(sign(&seed, PURPOSE, &["a\nb"]).is_err());
        assert!(!verify(&[&public], PURPOSE, &["a\nb"], &"00".repeat(SIGNATURE_BYTES)));
    }

    #[test]
    fn a_key_is_hex_of_the_right_length() {
        let (seed, public) = new_key().unwrap();
        assert_eq!(public_key(&seed).unwrap(), public);
        assert_eq!(public_key(&format!("{seed}\n")).unwrap(), public, "a line end after it is fine");
        assert!(public_key(&seed[2..]).is_err());
        assert!(public_key(&"g".repeat(KEY_BYTES * 2)).is_err());
    }
}
