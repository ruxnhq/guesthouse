//! The release key: makes one, and signs a guest driver package's manifest with it, as the packages'
//! workflows do.
//!
//!     cargo run -p guesthouse-update-check --example release_key -- new-key KEY-FILE
//!     cargo run -p guesthouse-update-check --example release_key -- sign-driver MANIFEST
//!
//! `new-key` writes a new key's private half, in hex, to a file that mustn't exist yet, and prints its
//! public half, which goes into `RELEASE_KEYS` (`shared/update-check/src/signed.rs`). The private half
//! goes into the `GUESTHOUSE_RELEASE_KEY` secret and the password manager, never into the repo.
//!
//! `sign-driver` reads the private half from `GUESTHOUSE_RELEASE_KEY` and signs the manifest at
//! MANIFEST, whose file name is what it is signed for (`guesthouse-windows-driver-latest.json` or
//! `guesthouse-linux-driver-latest.json`). It then checks the signed manifest against the release keys
//! built in, as the Mac app does; if the app wouldn't take it, the file is left as it was and this
//! fails. The Windows host's latest file has its own signer, `guesthouse-control`'s `sign_latest`.

use guesthouse_update_check::{driver_manifest, signed};
use std::path::Path;
use std::process::ExitCode;

/// Where `sign-driver` reads the key's private half.
const KEY_VARIABLE: &str = "GUESTHOUSE_RELEASE_KEY";
/// The manifests it signs, by name.
const MANIFESTS: [&str; 2] = ["guesthouse-windows-driver-latest.json", "guesthouse-linux-driver-latest.json"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["new-key", path] => new_key(path),
        ["sign-driver", path] => sign_driver(path),
        _ => Err("Give `new-key KEY-FILE` or `sign-driver MANIFEST`.".to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn new_key(path: &str) -> Result<(), String> {
    let (seed, public) = signed::new_key()?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot make {path}: {e}"))?;
    std::io::Write::write_all(&mut file, format!("{seed}\n").as_bytes())
        .map_err(|e| format!("Cannot write {path}: {e}"))?;
    println!("{public}");
    Ok(())
}

fn sign_driver(path: &str) -> Result<(), String> {
    let purpose = Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if !MANIFESTS.contains(&purpose) {
        return Err(format!("{path} isn't a driver manifest: its name is one of {}.", MANIFESTS.join(", ")));
    }
    let seed = std::env::var(KEY_VARIABLE).map_err(|_| format!("{KEY_VARIABLE} holds no key."))?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("Cannot read {path}: {e}"))?;
    let signed = driver_manifest::sign(&text, purpose, &seed)?;
    if !driver_manifest::verify(&signed, purpose, signed::RELEASE_KEYS) {
        return Err(format!(
            "The Mac app wouldn't take {path} signed with this key: its public half, {}, isn't one of \
             RELEASE_KEYS in shared/update-check/src/signed.rs.",
            signed::public_key(&seed)?
        ));
    }
    std::fs::write(path, signed).map_err(|e| format!("Cannot write {path}: {e}"))?;
    println!("Signed {path}.");
    Ok(())
}
