//! Whether a pinned OpenPGP RSA key signed a file: an ASCII-armored detached version 4 signature
//! over the file's bytes, as Ubuntu's `SHA256SUMS.gpg` is. Only the one key it is given is trusted, named by its
//! fingerprint, so no keyring or `gpg` is needed on any host.

use ring::{digest, signature};

/// What a check says when it fails.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The key isn't the one pinned, or isn't an RSA key.
    Key,
    /// The signature isn't a binary-document signature made with RSA, or is cut short.
    Signature,
    /// The key didn't sign these bytes.
    Mismatch,
}

fn be16(bytes: &[u8]) -> Option<usize> {
    Some(usize::from(u16::from_be_bytes(bytes.get(..2)?.try_into().ok()?)))
}

/// A multi-precision integer: a bit count, then the bytes it takes.
fn mpi(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let size = be16(bytes)?.div_ceil(8);
    let value = bytes.get(2..2 + size)?;
    Some((value, &bytes[2 + size..]))
}

/// An ASN.1 length.
fn der_length(length: usize) -> Vec<u8> {
    match u8::try_from(length) {
        Ok(short) if short < 0x80 => vec![short],
        _ => {
            let long = u16::try_from(length).unwrap_or(u16::MAX).to_be_bytes();
            vec![0x82, long[0], long[1]]
        }
    }
}

/// An ASN.1 INTEGER holding the unsigned `value`.
fn der_integer(value: &[u8]) -> Vec<u8> {
    let start = value.iter().position(|byte| *byte != 0).unwrap_or(value.len());
    let value = &value[start..];
    let pad = value.first().is_some_and(|byte| byte & 0x80 != 0);
    let mut out = vec![0x02];
    out.extend(der_length(value.len() + usize::from(pad)));
    out.extend(pad.then_some(0));
    out.extend(value);
    out
}

/// The key packet's lowercase hex fingerprint, and its RSA public key as DER.
fn rsa_key(packet: &[u8]) -> Option<(String, Vec<u8>)> {
    let [4, _, _, _, _, 1, rest @ ..] = packet else { return None };
    let (modulus, rest) = mpi(rest)?;
    let (exponent, rest) = mpi(rest)?;
    if !rest.is_empty() {
        return None;
    }
    let mut hashed = vec![0x99];
    hashed.extend(u16::try_from(packet.len()).ok()?.to_be_bytes());
    hashed.extend(packet);
    let sum = digest::digest(&digest::SHA1_FOR_LEGACY_USE_ONLY, &hashed);
    let fingerprint = sum.as_ref().iter().map(|byte| format!("{byte:02x}")).collect();
    let body = [der_integer(modulus), der_integer(exponent)].concat();
    let mut der = vec![0x30];
    der.extend(der_length(body.len()));
    der.extend(body);
    Some((fingerprint, der))
}

/// The bytes inside an ASCII-armored signature: its base64 lines, without the header lines and the
/// checksum line.
fn dearmor(text: &str) -> Option<Vec<u8>> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "-----BEGIN PGP SIGNATURE-----" {
        return None;
    }
    let mut out = Vec::new();
    let (mut bits, mut have) = (0u32, 0u32);
    let mut in_body = false;
    for line in lines {
        let line = line.trim();
        if line.starts_with("-----END") {
            return Some(out);
        }
        if !in_body {
            in_body = line.is_empty();
            continue;
        }
        if line.starts_with('=') {
            continue;
        }
        for byte in line.bytes().take_while(|byte| *byte != b'=') {
            let value = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => return None,
            };
            bits = bits << 6 | u32::from(value);
            have += 6;
            if have >= 8 {
                have -= 8;
                out.push((bits >> have) as u8);
                bits &= (1 << have) - 1;
            }
        }
    }
    None
}

/// The signature packets in `bytes`, as their bodies.
fn packets(mut bytes: &[u8]) -> Vec<&[u8]> {
    let mut found = Vec::new();
    while let Some((&tag, rest)) = bytes.split_first() {
        let (kind, length, rest) = if tag & 0x40 != 0 {
            let Some((&first, more)) = rest.split_first() else { break };
            match first {
                0..=191 => (tag & 0x3f, usize::from(first), more),
                192..=223 => match more.split_first() {
                    Some((&second, more)) => {
                        (tag & 0x3f, ((usize::from(first) - 192) << 8) + usize::from(second) + 192, more)
                    }
                    None => break,
                },
                _ => break,
            }
        } else {
            let width = match tag & 3 {
                0 => 1,
                1 => 2,
                _ => break,
            };
            let Some(size) = rest.get(..width) else { break };
            let length = size.iter().fold(0, |all, byte| all << 8 | usize::from(*byte));
            ((tag >> 2) & 0xf, length, &rest[width..])
        };
        let Some(body) = rest.get(..length) else { break };
        if kind == 2 {
            found.push(body);
        }
        bytes = &rest[length..];
    }
    found
}

/// Checks one signature packet's body over `data`.
fn check_one(der: &[u8], data: &[u8], body: &[u8]) -> Result<(), Refusal> {
    let [4, 0x00, 1, hash, rest @ ..] = body else { return Err(Refusal::Signature) };
    let algorithm: &'static signature::RsaParameters = match hash {
        8 => &signature::RSA_PKCS1_2048_8192_SHA256,
        9 => &signature::RSA_PKCS1_2048_8192_SHA384,
        10 => &signature::RSA_PKCS1_2048_8192_SHA512,
        _ => return Err(Refusal::Signature),
    };
    let hashed = be16(rest).ok_or(Refusal::Signature)?;
    let header = 6 + hashed;
    let after = body.get(header..).ok_or(Refusal::Signature)?;
    let unhashed = be16(after).ok_or(Refusal::Signature)?;
    let after = after.get(2 + unhashed + 2..).ok_or(Refusal::Signature)?;
    let (value, _) = mpi(after).ok_or(Refusal::Signature)?;
    let mut message = data.to_vec();
    message.extend(&body[..header]);
    message.extend([4, 0xff]);
    message.extend(u32::try_from(header).map_err(|_| Refusal::Signature)?.to_be_bytes());
    let key = signature::UnparsedPublicKey::new(algorithm, der);
    // The signature is as long as the modulus, with the leading zeros the packet leaves out.
    let width = der_modulus_len(der).ok_or(Refusal::Key)?;
    let mut padded = vec![0; width.saturating_sub(value.len())];
    padded.extend(value);
    key.verify(&message, &padded).map_err(|_| Refusal::Mismatch)
}

/// The modulus' length in bytes, from the DER `rsa_key` makes.
fn der_modulus_len(der: &[u8]) -> Option<usize> {
    let rest = der.get(1..)?;
    let rest = rest.get(if rest.first()? & 0x80 != 0 { 3 } else { 1 }..)?;
    let rest = rest.strip_prefix(&[0x02])?;
    let (length, rest) = if rest.first()? & 0x80 != 0 {
        (usize::from(u16::from_be_bytes([*rest.get(1)?, *rest.get(2)?])), rest.get(3..)?)
    } else {
        (usize::from(*rest.first()?), rest.get(1..)?)
    };
    Some(length - usize::from(*rest.first()? == 0))
}

/// Whether `signature` (ASCII-armored) is a signature of `data` by the key whose packet is
/// `key_packet`, and that key's fingerprint is `fingerprint` (40 lowercase hex digits).
pub fn verify(key_packet: &[u8], fingerprint: &str, data: &[u8], signature: &[u8]) -> Result<(), Refusal> {
    let (found, der) = rsa_key(key_packet).ok_or(Refusal::Key)?;
    if found != fingerprint {
        return Err(Refusal::Key);
    }
    let mut last = Refusal::Signature;
    let signature = std::str::from_utf8(signature).ok().and_then(dearmor).ok_or(Refusal::Signature)?;
    for body in packets(&signature) {
        match check_one(&der, data, body) {
            Ok(()) => return Ok(()),
            Err(refusal) => last = refusal,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = include_bytes!("../../catalog/src/ubuntu_cd_signing_key.bin");
    const FINGERPRINT: &str = "843938df228d22f7b3742bc0d94aa3f0efe21092";
    const SUMS: &[u8] =
        include_bytes!("../../../linux/app-core/tests/catalog/releases-ubuntu-26.04-SHA256SUMS");
    const SIGNATURE: &[u8] =
        include_bytes!("../../../linux/app-core/tests/catalog/releases-ubuntu-26.04-SHA256SUMS.gpg");

    #[test]
    fn ubuntu_s_own_list_and_signature_check_out() {
        assert_eq!(verify(KEY, FINGERPRINT, SUMS, SIGNATURE), Ok(()));
    }

    #[test]
    fn a_changed_list_a_changed_signature_or_another_key_is_refused() {
        let mut list = SUMS.to_vec();
        list[0] ^= 1;
        assert_eq!(verify(KEY, FINGERPRINT, &list, SIGNATURE), Err(Refusal::Mismatch));
        let mut signature = SIGNATURE.to_vec();
        signature[0x80] = if signature[0x80] == b'A' { b'B' } else { b'A' };
        assert_eq!(verify(KEY, FINGERPRINT, SUMS, &signature), Err(Refusal::Mismatch));
        assert_eq!(verify(KEY, &"0".repeat(40), SUMS, SIGNATURE), Err(Refusal::Key));
        assert_eq!(verify(&KEY[..100], FINGERPRINT, SUMS, SIGNATURE), Err(Refusal::Key));
        assert_eq!(verify(KEY, FINGERPRINT, SUMS, b""), Err(Refusal::Signature));
        assert_eq!(verify(KEY, FINGERPRINT, SUMS, &SIGNATURE[..100]), Err(Refusal::Signature));
    }
}
