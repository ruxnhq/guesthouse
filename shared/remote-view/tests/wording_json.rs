//! `wording.json` is what the sentences make today, byte for byte, so an app that reads the file
//! shows the same words as one that calls the functions.
//! `GUESTHOUSE_WRITE_GOLDEN=1 cargo test -p guesthouse-remote-view-core --test wording_json` writes it
//! afresh, after a sentence was added or changed on purpose.

use guesthouse_remote_view_core::wording::data::{FILE, json};
use std::fs;
use std::path::Path;

const WRITE: &str = "GUESTHOUSE_WRITE_GOLDEN";

#[test]
fn the_kept_file_is_what_the_sentences_make() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(FILE);
    let made = json();
    if std::env::var_os(WRITE).is_some() {
        fs::write(&path, &made).unwrap();
        return;
    }
    let kept = fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert!(
        kept == made.as_bytes(),
        "{FILE} is not what the sentences make. If a sentence changed on purpose, run \
         {WRITE}=1 cargo test -p guesthouse-remote-view-core --test wording_json"
    );
}
