//! The plain logic of a download, apart from doing it: where its files go and what they're called,
//! the SHA-256 of what was downloaded, the words curl is given, and what its answers mean. The doing
//! is in system.rs.
//!
//! A download goes into the app's own folder for updates, `updates` in this user's local app data,
//! which only this user (and the system and administrators, as for any of a user's folders) can
//! write to: never into a shared temporary folder under a name another program could take first.
//! Each download has a name of its own, from the version the latest file gave and its attempt's
//! number, and goes to a `.part` file until curl has ended, then to an `.exe` of that name, which
//! is checked and run. Nothing in the folder is kept from one run of the app to the next: it's
//! emptied as the app starts, and whenever a download is put aside.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The longest an installer can be. A release's is about 70 MB.
pub const LARGEST: u64 = 256 * 1024 * 1024;
/// The longest a download may take, in seconds.
pub const LONGEST_SECONDS: u64 = 60 * 60;
/// How long the file drop has to answer, in seconds.
pub const CONNECT_SECONDS: u64 = 30;
/// A download slower than this many bytes a second, for `STALLED_SECONDS`, has stalled and is ended.
pub const SLOWEST: u64 = 1024;
pub const STALLED_SECONDS: u64 = 120;

/// The folder in the app's own where updates are downloaded.
pub fn folder(app_folder: &Path) -> PathBuf {
    app_folder.join("updates")
}

/// The file download `attempt` of `version` is written to while it comes.
pub fn partial(folder: &Path, version: &str, attempt: u64) -> PathBuf {
    folder.join(format!("guesthouse-setup-{version}-{attempt}.part"))
}

/// The file curl writes the file drop's headers to, for the download's length.
pub fn headers(folder: &Path, version: &str, attempt: u64) -> PathBuf {
    folder.join(format!("guesthouse-setup-{version}-{attempt}.headers"))
}

/// The installer download `attempt` of `version` becomes once curl has ended: an .exe, as Windows
/// asks for an administrator only for a program.
pub fn installer(folder: &Path, version: &str, attempt: u64) -> PathBuf {
    folder.join(format!("guesthouse-setup-{version}-{attempt}.exe"))
}

/// Deletes everything in `folder`, as nothing in it is trusted from one run to the next. What can't
/// be deleted now, as a program still holds it, is left; it's never trusted either.
pub fn clear(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The SHA-256 of everything `reader` holds, in lowercase hex, as a release's latest file gives it.
pub fn sha256(mut reader: impl Read) -> io::Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The words curl is given to download `url` to `output`, writing the answer's headers to
/// `headers` and the answer's status on its standard output. Only HTTPS, from `url` itself, as no
/// redirect is followed; the file drop has at most `CONNECT_SECONDS` to answer; a download that
/// stalls, says it's larger than `LARGEST` or takes longer than `LONGEST_SECONDS` is ended. The app
/// watches the file's size and the time too, for an answer that doesn't say its length.
pub fn curl_words(url: &str, output: &Path, headers: &Path) -> Vec<String> {
    let mut words: Vec<String> = [
        "--fail",
        "--silent",
        "--show-error",
        "--globoff",
        "--proto",
        "=https",
        "--write-out",
        "%{http_code}",
    ]
    .iter()
    .map(|word| (*word).to_owned())
    .collect();
    for (option, value) in [
        ("--connect-timeout", CONNECT_SECONDS.to_string()),
        ("--max-time", LONGEST_SECONDS.to_string()),
        ("--speed-limit", SLOWEST.to_string()),
        ("--speed-time", STALLED_SECONDS.to_string()),
        ("--max-filesize", LARGEST.to_string()),
    ] {
        words.push(option.to_owned());
        words.push(value);
    }
    words.push("--dump-header".to_owned());
    words.push(headers.display().to_string());
    words.push("--output".to_owned());
    words.push(output.display().to_string());
    words.push(url.to_owned());
    words
}

/// The length the file drop gave for what it sends, from the headers curl wrote, if it gave one.
pub fn content_length(headers: &str) -> Option<u64> {
    headers.lines().rev().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse().ok()).flatten()
    })
}

/// Why a download stopped.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Failed {
    /// The file drop couldn't be reached, or stopped answering.
    Unreached,
    /// It answered with this HTTP status, not the installer.
    Refused(u16),
    /// It was larger than `LARGEST`.
    TooLarge,
    /// It took longer than `LONGEST_SECONDS`, or stalled.
    TooLong,
    /// It couldn't be written to this PC's disk.
    Disk(String),
    /// Something else, in curl's words.
    Other(String),
}

impl Failed {
    /// What the person is told.
    pub fn words(&self) -> String {
        match self {
            Failed::Unreached => {
                "The download stopped: files.ruxn.com couldn't be reached, or stopped answering.".to_owned()
            }
            Failed::Refused(status) => format!("files.ruxn.com wouldn't give the installer (HTTP {status})."),
            Failed::TooLarge => {
                format!("It was larger than an installer can be ({} MB), so it was stopped.", LARGEST >> 20)
            }
            Failed::TooLong => "It took too long, or stalled, so it was stopped.".to_owned(),
            Failed::Disk(why) => format!("It couldn't be saved on this PC: {why}."),
            Failed::Other(why) => format!("The download stopped: {why}."),
        }
    }
}

/// What curl's exit `code`, the status it wrote, and what it said on its standard error mean.
pub fn curl_answer(code: Option<i32>, status: &str, said: &str) -> Result<(), Failed> {
    let status: u16 = status.trim().parse().unwrap_or(0);
    let said = said.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("").to_owned();
    match code {
        Some(0) if status == 200 => Ok(()),
        // A redirect, or anything but the file itself, isn't the installer.
        Some(0) | Some(22) => Err(Failed::Refused(status)),
        Some(5..=7) | Some(35) | Some(52) | Some(55) | Some(56) => Err(Failed::Unreached),
        Some(23) => Err(Failed::Disk(said)),
        Some(28) => Err(Failed::TooLong),
        Some(63) => Err(Failed::TooLarge),
        Some(code) if said.is_empty() => Err(Failed::Other(format!("curl ended with {code}"))),
        Some(_) => Err(Failed::Other(said)),
        None => Err(Failed::Other("curl was ended".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_download_has_names_of_its_own_in_the_apps_folder() {
        let folder = folder(Path::new("app"));
        assert_eq!(folder, Path::new("app").join("updates"));
        let part = partial(&folder, "0.14.0", 3);
        let exe = installer(&folder, "0.14.0", 3);
        assert_eq!(part.file_name().unwrap(), "guesthouse-setup-0.14.0-3.part");
        assert_eq!(exe.file_name().unwrap(), "guesthouse-setup-0.14.0-3.exe");
        assert_eq!(headers(&folder, "0.14.0", 3).file_name().unwrap(), "guesthouse-setup-0.14.0-3.headers");
        assert_ne!(installer(&folder, "0.14.0", 4), exe, "another attempt, another file");
        assert_eq!(part.parent(), Some(folder.as_path()));
    }

    #[test]
    fn the_hash_is_sha_256_in_lowercase_hex() {
        assert_eq!(
            sha256(&b""[..]).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(&b"abc"[..]).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Larger than one read.
        let big = vec![7u8; (1 << 20) + 5];
        assert_eq!(sha256(&big[..]).unwrap(), sha256(std::io::Cursor::new(big.clone())).unwrap());
    }

    #[test]
    fn curl_takes_only_https_from_the_address_itself_with_limits() {
        let words = curl_words(
            "https://files.ruxn.com/p/guesthouse/guesthouse-setup-0.14.0.exe",
            Path::new("out.part"),
            Path::new("out.headers"),
        );
        let has =
            |option: &str, value: &str| words.windows(2).any(|pair| pair[0] == option && pair[1] == value);
        assert!(has("--proto", "=https"));
        assert!(!words.iter().any(|word| word == "--location" || word == "-L"), "no redirect is followed");
        assert!(words.iter().any(|word| word == "--globoff"));
        assert!(has("--max-filesize", &LARGEST.to_string()));
        assert!(has("--max-time", "3600"));
        assert!(has("--speed-limit", "1024") && has("--speed-time", "120"));
        assert!(has("--output", "out.part"));
        assert!(has("--dump-header", "out.headers"));
        assert_eq!(words.last().unwrap(), "https://files.ruxn.com/p/guesthouse/guesthouse-setup-0.14.0.exe");
    }

    #[test]
    fn the_length_is_the_last_answers() {
        let headers = "HTTP/2 301\r\ncontent-length: 12\r\n\r\nHTTP/2 200\r\nContent-Type: x\r\nContent-Length: 68084056\r\n\r\n";
        assert_eq!(content_length(headers), Some(68_084_056));
        assert_eq!(content_length("HTTP/2 200\r\ncontent-type: x\r\n"), None);
        assert_eq!(content_length("content-length: lots"), None);
    }

    #[test]
    fn curls_answers_say_why() {
        assert_eq!(curl_answer(Some(0), "200", ""), Ok(()));
        assert_eq!(curl_answer(Some(0), "302", ""), Err(Failed::Refused(302)), "a redirect isn't the file");
        assert_eq!(
            curl_answer(Some(22), "404", "curl: (22) The requested URL returned error: 404"),
            Err(Failed::Refused(404))
        );
        assert_eq!(curl_answer(Some(6), "000", "curl: (6) Could not resolve host"), Err(Failed::Unreached));
        assert_eq!(curl_answer(Some(7), "000", ""), Err(Failed::Unreached));
        assert_eq!(curl_answer(Some(28), "200", ""), Err(Failed::TooLong));
        assert_eq!(curl_answer(Some(63), "200", ""), Err(Failed::TooLarge));
        assert_eq!(
            curl_answer(Some(23), "200", "curl: (23) Failure writing output\n"),
            Err(Failed::Disk("curl: (23) Failure writing output".to_owned()))
        );
        assert_eq!(curl_answer(Some(60), "000", ""), Err(Failed::Other("curl ended with 60".to_owned())));
        assert_eq!(curl_answer(None, "", ""), Err(Failed::Other("curl was ended".to_owned())));
        for failed in [Failed::Unreached, Failed::Refused(404), Failed::TooLarge, Failed::TooLong] {
            assert!(failed.words().ends_with('.'), "{failed:?}");
        }
    }

    #[test]
    fn clearing_the_folder_leaves_nothing_to_trust() {
        let folder = std::env::temp_dir().join(format!("gh-updates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        clear(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        // A half download, and a whole one, from a run of the app that was closed.
        std::fs::write(partial(&folder, "0.14.0", 1), b"half").unwrap();
        std::fs::write(installer(&folder, "0.14.0", 2), b"whole").unwrap();
        clear(&folder);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
