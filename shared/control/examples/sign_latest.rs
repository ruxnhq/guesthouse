//! Signs a Windows host release's `guesthouse-windows-latest.json` with the release key, as
//! `scripts/windows-release.ps1` does:
//!
//!     cargo run -p guesthouse-control --example sign_latest -- sign PATH\TO\guesthouse-windows-latest.json
//!
//! It reads the key's private half, in hex, from `GUESTHOUSE_RELEASE_KEY`, adds the signature to the
//! file, and then reads the file back as the app does, with the release keys built into it
//! (`guesthouse_update_check::signed::RELEASE_KEYS`). If the app wouldn't take it, as when the key
//! isn't one of those, the file is left as it was and this fails. The `release_key` example of
//! `guesthouse-update-check` makes a key.

use guesthouse_control::updates::{self, Latest, RELEASE_KEYS};
use std::process::ExitCode;

/// Where `sign` reads the key's private half.
const KEY_VARIABLE: &str = "GUESTHOUSE_RELEASE_KEY";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["sign", path] => sign(path),
        _ => Err("Give `sign LATEST-FILE`.".to_owned()),
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
             RELEASE_KEYS in shared/update-check/src/signed.rs, or the file's fields aren't a release's.",
            updates::signed::public_key(&seed)?
        ));
    }
    std::fs::write(path, signed + "\n").map_err(|e| format!("Cannot write {path}: {e}"))?;
    println!("Signed {path}.");
    Ok(())
}
