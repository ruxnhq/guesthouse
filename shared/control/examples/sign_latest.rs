//! Signs a Windows host release's `guesthouse-windows-latest.json` with the release key, as
//! `scripts/windows-release.ps1` does, and makes a release key:
//!
//!     cargo run -p guesthouse-control --example sign_latest -- sign PATH\TO\guesthouse-windows-latest.json
//!     cargo run -p guesthouse-control --example sign_latest -- new-key PATH\TO\KEY-FILE
//!
//! `sign` reads the key's private half, in hex, from `GUESTHOUSE_WINDOWS_UPDATE_KEY`, adds the
//! signature to the file, and then reads the file back as the app does, with the release keys built
//! into it (`guesthouse_control::updates::RELEASE_KEYS`). If the app wouldn't take it, as when the key
//! isn't one of those, the file is left as it was and this fails.
//!
//! `new-key` writes a new key's private half to a file that mustn't exist yet, and prints its public
//! half, which goes into `RELEASE_KEYS`. The private half goes into a secret, never into the repo.

use guesthouse_control::updates::{self, Latest, RELEASE_KEYS};
use std::process::ExitCode;

/// Where `sign` reads the key's private half.
const KEY_VARIABLE: &str = "GUESTHOUSE_WINDOWS_UPDATE_KEY";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["sign", path] => sign(path),
        ["new-key", path] => new_key(path),
        _ => Err("Give `sign LATEST-FILE` or `new-key KEY-FILE`.".to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

fn sign(path: &str) -> Result<(), String> {
    let seed = std::env::var(KEY_VARIABLE).map_err(|_| format!("{KEY_VARIABLE} holds no key."))?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("Cannot read {path}: {e}"))?;
    let mut latest: Latest =
        serde_json::from_str(&text).map_err(|e| format!("{path} isn't a latest file: {e}"))?;
    latest.signature = latest.sign(&seed)?;
    let signed = serde_json::to_string_pretty(&serde_json::json!({
        "version": latest.version,
        "installer": latest.installer,
        "sha256": latest.sha256,
        "notes": latest.notes,
        "signature": latest.signature,
    }))
    .map_err(|e| e.to_string())?;
    if updates::parse(&signed, RELEASE_KEYS).is_none() {
        return Err(format!(
            "The app wouldn't take {path} signed with this key: its public half, {}, isn't one of \
             RELEASE_KEYS in shared/control/src/updates.rs, or the file's fields aren't a release's.",
            updates::signed::public_key(&seed)?
        ));
    }
    std::fs::write(path, signed + "\n").map_err(|e| format!("Cannot write {path}: {e}"))?;
    println!("Signed {path}.");
    Ok(())
}

fn new_key(path: &str) -> Result<(), String> {
    let (seed, public) = updates::signed::new_key()?;
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
