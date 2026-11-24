//! The text work behind the facts a bug report gathers from the host: cutting the Windows event
//! log down to the events about Guesthouse, reading values out of `reg query`, and leaving the
//! lines about other users' refused requests out of the service's log. It is plain text in and
//! out, so it is tested anywhere; running the programs is `windows::collect`'s.

use std::fs;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;

/// The end of the file at `path`: all of it if it's at most `most_bytes` long, and otherwise its last
/// part, from the start of a line, after a line that says how much is left out. A file that is
/// missing, or isn't a plain file of its own (a link is never followed), is an error with the reason
/// in words.
pub fn tail_file(path: &Path, most_bytes: u64) -> Result<String, String> {
    let meta = fs::symlink_metadata(path).map_err(|error| reason(path, &error))?;
    if !meta.is_file() {
        return Err(format!("{} isn't a plain file, so it's left.", path.display()));
    }
    let mut file = fs::File::open(path).map_err(|error| reason(path, &error))?;
    let cut = meta.len().saturating_sub(most_bytes);
    file.seek(SeekFrom::Start(cut)).map_err(|error| reason(path, &error))?;
    let mut bytes = Vec::new();
    file.take(most_bytes).read_to_end(&mut bytes).map_err(|error| reason(path, &error))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if cut == 0 {
        return Ok(text);
    }
    // The cut is mostly inside a line, so the kept part starts at the next one.
    let start = text.find('\n').map_or(0, |at| at + 1);
    Ok(format!("[{} bytes left out at the start]\n{}", cut + start as u64, &text[start..]))
}

fn reason(path: &Path, error: &std::io::Error) -> String {
    if error.kind() == ErrorKind::NotFound {
        format!("{} isn't there.", path.display())
    } else {
        format!("{} couldn't be read: {error}", path.display())
    }
}

/// Whether a line starts an event in `wevtutil`'s text output: `Event[` and a number, then `]:`.
fn starts_event(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("Event[") else {
        return false;
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && rest[digits..].starts_with("]:")
}

/// The events of `wevtutil qe ... /f:text` that mention `needle`, in any case, a blank line between
/// them. Whatever comes before the first event is dropped.
pub fn keep_events(text: &str, needle: &str) -> String {
    let needle = needle.to_lowercase();
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        if starts_event(line) {
            blocks.push(vec![line]);
        } else if let Some(block) = blocks.last_mut() {
            block.push(line);
        }
    }
    blocks
        .iter()
        .map(|block| block.join("\n").trim_end().to_string())
        .filter(|block| block.to_lowercase().contains(&needle))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The values `names` ask for from `reg query`'s lines (`NAME    REG_SZ    VALUE`), in the order
/// asked, with a number as a decimal. A name that isn't there is left out.
pub fn registry_values(text: &str, names: &[&str]) -> Vec<(String, String)> {
    let parsed: Vec<(&str, String)> = text.lines().filter_map(parse_value).collect();
    names
        .iter()
        .filter_map(|name| {
            parsed
                .iter()
                .find(|(found, _)| found.eq_ignore_ascii_case(name))
                .map(|(_, value)| (name.to_string(), value.clone()))
        })
        .collect()
}

/// A name and value from one line of `reg query`, if it is one.
fn parse_value(line: &str) -> Option<(&str, String)> {
    let line = line.trim();
    let (name, rest) = line.split_once(char::is_whitespace)?;
    let rest = rest.trim_start();
    let (kind, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let value = value.trim();
    if !kind.starts_with("REG_") {
        return None;
    }
    if kind == "REG_DWORD" || kind == "REG_QWORD" {
        let hex = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X"))?;
        return Some((name, u64::from_str_radix(hex, 16).ok()?.to_string()));
    }
    Some((name, value.to_string()))
}

/// The service's log without the lines about other users' refused requests, which are tagged
/// `refused: `, so a report doesn't carry what someone else asked.
pub fn without_refusals(log: &str) -> String {
    let mut kept: Vec<&str> = log.lines().filter(|line| !line.contains("refused: ")).collect();
    if log.ends_with('\n') && !kept.is_empty() {
        kept.push("");
    }
    kept.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENTS: &str = "\
Some header line
Event[0]:
  Log Name: Microsoft-Windows-Hyper-V-Compute-Admin
  Level: Information
  Description:
  [vm1] Created compute system guesthouse-ubuntu.

Event[1]:
  Log Name: Microsoft-Windows-Hyper-V-Compute-Admin
  Level: Error
  Description:
  Compute system docker-desktop failed to start.

  Event[12]:
  Log Name: Microsoft-Windows-Hyper-V-Compute-Admin
  Level: Information
  Description:
  WSL utility VM started.
";

    #[test]
    fn keeps_only_matching_events() {
        let kept = keep_events(EVENTS, "guesthouse-");
        assert!(kept.starts_with("Event[0]:"));
        assert!(kept.contains("guesthouse-ubuntu"));
        assert!(!kept.contains("docker-desktop"));
        assert!(!kept.contains("WSL"));
        assert!(!kept.contains("Some header"));
    }

    #[test]
    fn matches_in_any_case_and_joins_with_a_blank_line() {
        let kept = keep_events(EVENTS, "COMPUTE SYSTEM");
        let parts: Vec<&str> = kept.split("\n\n").collect();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].starts_with("Event[0]:"));
        assert!(parts[1].starts_with("Event[1]:"));
    }

    #[test]
    fn nothing_matching_is_empty() {
        assert_eq!(keep_events(EVENTS, "nothing like this"), "");
        assert_eq!(keep_events("", "x"), "");
    }

    const REG: &str = "
HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion
    ProductName    REG_SZ    Windows 11 Pro
    EditionID    REG_SZ    Professional
    DisplayVersion    REG_SZ    24H2
    CurrentBuild    REG_SZ    26100
    UBR    REG_DWORD    0x1a2b
    InstallDate    REG_DWORD    0x65a1b2c3
";

    #[test]
    fn reads_named_values_in_the_order_asked() {
        let got = registry_values(REG, &["UBR", "ProductName", "CurrentBuild"]);
        assert_eq!(
            got,
            vec![
                ("UBR".to_string(), "6699".to_string()),
                ("ProductName".to_string(), "Windows 11 Pro".to_string()),
                ("CurrentBuild".to_string(), "26100".to_string()),
            ]
        );
    }

    #[test]
    fn skips_missing_values() {
        let got = registry_values(REG, &["Nope", "EditionID"]);
        assert_eq!(got, vec![("EditionID".to_string(), "Professional".to_string())]);
        assert!(registry_values("", &["UBR"]).is_empty());
    }

    fn folder(test: &str) -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("gh-tail-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn a_file_that_fits_is_read_whole() {
        let folder = folder("whole");
        fs::write(folder.join("a.log"), "one\ntwo\n").unwrap();
        assert_eq!(tail_file(&folder.join("a.log"), 100).unwrap(), "one\ntwo\n");
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_long_file_is_cut_to_its_end_from_a_line_start() {
        let folder = folder("long");
        fs::write(folder.join("a.log"), "alpha\nbravo\ncharlie\ndelta\n").unwrap();
        // 13 bytes start inside "charlie", so the kept part starts at "delta".
        let text = tail_file(&folder.join("a.log"), 13).unwrap();
        assert_eq!(text, "[20 bytes left out at the start]\ndelta\n");
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_missing_file_and_a_folder_are_errors_with_reasons() {
        let folder = folder("missing");
        let said = tail_file(&folder.join("none.log"), 10).unwrap_err();
        assert!(said.ends_with("none.log isn't there."), "{said}");
        let said = tail_file(&folder, 10).unwrap_err();
        assert!(said.contains("isn't a plain file"), "{said}");
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn leaves_out_refusals() {
        let log = "started\nrequest refused: pid 4 asked for a thing\nboot done\n";
        assert_eq!(without_refusals(log), "started\nboot done\n");
        assert_eq!(without_refusals("a\nb"), "a\nb");
        assert_eq!(without_refusals("refused: x\n"), "");
    }
}
