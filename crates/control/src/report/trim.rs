//! Cuts a long log down to its end, which is where a run's story usually ends.

/// The text as it is if it fits in `most_bytes`, and otherwise its last part that fits, cut at a
/// character and, where a line starts inside that part, at the start of that line. A line before it
/// says how many bytes are left out at the start. That line is not counted in `most_bytes`.
pub fn keep_end(text: &str, most_bytes: usize) -> String {
    if text.len() <= most_bytes {
        return text.to_owned();
    }
    let mut from = text.len() - most_bytes;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    // A cut right after a line break already starts a line. Otherwise the next line does, if the
    // kept part has one that is not empty.
    if !text[..from].ends_with('\n')
        && let Some(found) = text[from..].find('\n')
        && from + found + 1 < text.len()
    {
        from += found + 1;
    }
    format!("[{from} bytes left out at the start]\n{}", &text[from..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_fits_stays_whole() {
        assert_eq!(keep_end("one\ntwo\n", 8), "one\ntwo\n");
        assert_eq!(keep_end("", 0), "");
    }

    #[test]
    fn the_end_is_kept_from_a_line_start() {
        let text = "alpha\nbravo\ncharlie\ndelta\n";
        // 13 bytes would start inside "charlie", so the kept part starts at "delta".
        assert_eq!(keep_end(text, 13), "[20 bytes left out at the start]\ndelta\n");
    }

    #[test]
    fn a_cut_on_a_line_start_keeps_that_line() {
        let text = "alpha\nbravo\ncharlie\n";
        assert_eq!(keep_end(text, 8), "[12 bytes left out at the start]\ncharlie\n");
    }

    #[test]
    fn text_with_no_line_break_is_cut_where_it_falls() {
        assert_eq!(keep_end("0123456789", 4), "[6 bytes left out at the start]\n6789");
    }

    #[test]
    fn a_last_line_that_is_all_there_is_to_keep_is_not_skipped() {
        // The only line break is the last byte, so cutting after it would keep nothing.
        assert_eq!(keep_end("0123456789\n", 4), "[7 bytes left out at the start]\n789\n");
    }

    #[test]
    fn the_cut_is_on_a_character() {
        let text = "é".repeat(10);
        let kept = keep_end(&text, 5);
        assert_eq!(kept, format!("[{} bytes left out at the start]\n{}", 20 - 4, "é".repeat(2)));
        let kept = keep_end("画面が黒", 4);
        assert_eq!(kept, "[9 bytes left out at the start]\n黒");
    }
}
