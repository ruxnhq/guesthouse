//! The signature of a guest driver package's manifest, `guesthouse-windows-driver-latest.json` or
//! `guesthouse-linux-driver-latest.json`, as the packages' workflows publish it:
//!
//! ```json
//! {"version": "0.3.36", "files": [
//!   {"name": "guesthouse-windows-driver-0.3.36-ruxnwddm.sys", "as": "ruxnwddm.sys", "sha256": "...", "size": 123}
//! ], "signature": "..."}
//! ```
//!
//! `signature` is a release key's signature ([`crate::signed`]) of the version, the number of files,
//! and each file's published name, name in the package, SHA-256 and size (empty when it gives none), in
//! order. The purpose is the manifest's own name, so one package's manifest is never taken as the
//! other's. A host that fetches a package takes it only with a valid signature, and checks every file
//! against the SHA-256 the signature covers; what else a manifest must hold is the host's to check.

use crate::signed;
use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    version: String,
    files: Vec<Entry>,
    #[serde(default)]
    signature: String,
}

#[derive(Deserialize)]
struct Entry {
    name: String,
    #[serde(rename = "as")]
    package_name: String,
    sha256: String,
    #[serde(default)]
    size: Option<u64>,
}

impl Manifest {
    fn read(text: &str) -> Result<Manifest, String> {
        serde_json::from_str(text).map_err(|e| format!("The driver's manifest is not readable: {e}"))
    }

    fn fields(&self) -> Vec<String> {
        let mut fields = vec![self.version.clone(), self.files.len().to_string()];
        for entry in &self.files {
            fields.push(entry.name.clone());
            fields.push(entry.package_name.clone());
            fields.push(entry.sha256.clone());
            fields.push(entry.size.map(|size| size.to_string()).unwrap_or_default());
        }
        fields
    }

    fn verify(&self, purpose: &str, keys: &[&str]) -> bool {
        let fields = self.fields();
        let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
        signed::verify(keys, purpose, &fields, &self.signature)
    }
}

/// Whether the manifest `text` carries a signature of its fields by one of `keys` (public halves, in
/// hex), for `purpose`, the manifest's name.
pub fn verify(text: &str, purpose: &str, keys: &[&str]) -> bool {
    Manifest::read(text).is_ok_and(|manifest| manifest.verify(purpose, keys))
}

/// The manifest `text` with its signature by the key whose private half is `seed` (hex), for
/// `purpose`, the manifest's name: every field it had, and `signature` set.
pub fn sign(text: &str, purpose: &str, seed: &str) -> Result<String, String> {
    let manifest = Manifest::read(text)?;
    let fields = manifest.fields();
    let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
    let signature = signed::sign(seed, purpose, &fields)?;
    let mut value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let object = value.as_object_mut().ok_or("The driver's manifest is not an object.")?;
    object.insert("signature".to_owned(), signature.into());
    let signed = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())? + "\n";
    // What was signed is what the file now says.
    debug_assert!(verify(&signed, purpose, &[&signed::public_key(seed)?]));
    Ok(signed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signed::new_key;

    const WINDOWS: &str = "guesthouse-windows-driver-latest.json";
    const LINUX: &str = "guesthouse-linux-driver-latest.json";
    const SUM: &str = "38d05ebe74bb28e8402705f407ce20f16f1c611c0716062f52d911dd089cbf1d";

    fn manifest() -> String {
        format!(
            r#"{{"version": "0.1.7", "files": [
  {{"name": "guesthouse-linux-driver-0.1.7-install.sh", "as": "install.sh", "sha256": "{SUM}"}},
  {{"name": "guesthouse-linux-driver-0.1.7-pvg-dkms_0.1.7_all.deb", "as": "pvg-dkms_0.1.7_all.deb", "sha256": "{SUM}", "size": 4096}}
]}}
"#
        )
    }

    fn changed(text: &str, change: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut value: serde_json::Value = serde_json::from_str(text).unwrap();
        change(&mut value);
        value.to_string()
    }

    #[test]
    fn a_signed_manifest_is_taken_with_its_key_and_name() {
        let (seed, public) = new_key().unwrap();
        let signed = sign(&manifest(), LINUX, &seed).unwrap();
        assert!(verify(&signed, LINUX, &[&public]));
        let (_, other) = new_key().unwrap();
        assert!(!verify(&signed, LINUX, &[&other]), "another key");
        assert!(!verify(&signed, LINUX, &[]), "no key");
        assert!(!verify(&signed, WINDOWS, &[&public]), "the other package's manifest");
        assert!(!verify(&manifest(), LINUX, &[&public]), "no signature");
        // The fields it had are all still there, as the guest and older apps read them.
        let value: serde_json::Value = serde_json::from_str(&signed).unwrap();
        assert_eq!(value["version"], "0.1.7");
        assert_eq!(value["files"][1]["size"], 4096);
        assert_eq!(value["files"][0]["as"], "install.sh");
    }

    #[test]
    fn a_manifest_changed_after_signing_is_not_taken() {
        let (seed, public) = new_key().unwrap();
        let signed = sign(&manifest(), LINUX, &seed).unwrap();
        let other_sum = "00".repeat(32);
        type Change = Box<dyn FnOnce(&mut serde_json::Value)>;
        let changes: Vec<(&str, Change)> = vec![
            ("version", Box::new(|v| v["version"] = "0.1.8".into())),
            ("a sha256", Box::new(move |v| v["files"][1]["sha256"] = other_sum.into())),
            ("a name", Box::new(|v| v["files"][0]["name"] = "guesthouse-linux-driver-0.1.7-other.sh".into())),
            ("a name in the package", Box::new(|v| v["files"][0]["as"] = "other.sh".into())),
            ("a size", Box::new(|v| v["files"][1]["size"] = 1.into())),
            ("a size added", Box::new(|v| v["files"][0]["size"] = 1.into())),
            ("a file left out", Box::new(|v| v["files"].as_array_mut().unwrap().truncate(1))),
            (
                "a file added",
                Box::new(|v| {
                    let extra = v["files"][0].clone();
                    v["files"].as_array_mut().unwrap().push(extra);
                }),
            ),
        ];
        for (what, change) in changes {
            assert!(!verify(&changed(&signed, change), LINUX, &[&public]), "{what}");
        }
        // Its layout is free: the same fields, written another way, are the same manifest.
        assert!(verify(&changed(&signed, |_| ()), LINUX, &[&public]));
    }
}
