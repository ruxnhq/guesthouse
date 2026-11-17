//! File names that Windows takes, from names Linux allows.

/// What Windows keeps for itself, with or without an ending.
const DEVICES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
const REFUSED: &str = "<>:\"/\\|?*";
const INSTEAD: char = '_';

/// The name as Windows can hold it: the same wherever it can be.
pub fn for_windows(name: &str) -> String {
    let swapped =
        |letter: char| if REFUSED.contains(letter) || letter.is_control() { INSTEAD } else { letter };
    let mut safe: String = name.chars().map(swapped).collect();
    // Windows drops dots and spaces from the end, so two names could become one.
    while safe.ends_with(['.', ' ']) {
        safe.pop();
        safe.push(INSTEAD);
    }
    let stem = safe.split('.').next().unwrap_or_default();
    if safe.is_empty() || DEVICES.iter().any(|device| stem.eq_ignore_ascii_case(device)) {
        safe.insert(0, INSTEAD);
    }
    safe
}

/// A name no entry of `taken` has, which Windows compares without case.
pub fn not_among(name: String, taken: &[String]) -> String {
    let is_taken = |name: &str| taken.iter().any(|other| other.eq_ignore_ascii_case(name));
    if !is_taken(&name) {
        return name;
    }
    let (stem, ending) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name.as_str(), ""),
    };
    (2..).map(|copy| format!("{stem} ({copy}){ending}")).find(|name| !is_taken(name)).unwrap_or_default()
}

/// Whether a name from the guest is one plain name: no folder, and no way out of one.
pub fn is_one_name(part: &str) -> bool {
    !part.is_empty() && part != "." && part != ".." && !part.contains(['/', '\0'])
}
