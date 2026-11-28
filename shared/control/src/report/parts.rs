//! How a report's files come from the service. A file's text is cut in pieces, each of which is a
//! line on the pipe, and a line may be no longer than the service and the app read. The app puts
//! the pieces back together, in the order the files first came.

/// The most one piece takes on the wire, once it's written as JSON, which is half what a line may
/// be, to leave room for the line's other words.
pub const PIECE_BYTES: usize = 32 * 1024;

/// How many bytes `c` takes in a JSON string, as `serde_json` writes it.
fn json_bytes(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if c < ' ' => 6,
        c => c.len_utf8(),
    }
}

/// `text` cut in pieces that each take at most [`PIECE_BYTES`] as a JSON string, at a character.
/// No pieces for no text.
pub fn pieces(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let (mut start, mut size) = (0, 0);
    for (at, c) in text.char_indices() {
        let bytes = json_bytes(c);
        if size + bytes > PIECE_BYTES {
            found.push(&text[start..at]);
            (start, size) = (at, 0);
        }
        size += bytes;
    }
    if start < text.len() {
        found.push(&text[start..]);
    }
    found
}

/// The files the service handed over, and what it couldn't.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Parts {
    /// Each file's name and text, in the order they first came.
    pub files: Vec<(String, String)>,
    /// Each file that couldn't be handed over, and why.
    pub missing: Vec<(String, String)>,
}

impl Parts {
    /// Adds a piece to the file of that name, or begins the file.
    pub fn piece(&mut self, name: &str, text: &str) {
        match self.files.iter_mut().find(|(known, _)| known == name) {
            Some((_, whole)) => whole.push_str(text),
            None => self.files.push((name.to_owned(), text.to_owned())),
        }
    }

    pub fn miss(&mut self, name: &str, reason: &str) {
        self.missing.push((name.to_owned(), reason.to_owned()));
    }

    /// The text of the file called `name`, if it came.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.files.iter().find(|(known, _)| known == name).map(|(_, text)| text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_text_is_no_pieces_and_short_text_is_one() {
        assert!(pieces("").is_empty());
        assert_eq!(pieces("a line\n"), ["a line\n"]);
    }

    #[test]
    fn long_text_is_cut_into_pieces_that_join_back() {
        let text = "0123456789\n".repeat(10_000);
        let found = pieces(&text);
        assert!(found.len() > 1);
        assert_eq!(found.concat(), text);
        for piece in &found {
            assert!(serde_json::to_string(piece).unwrap().len() <= PIECE_BYTES + 2);
        }
    }

    #[test]
    fn a_piece_is_cut_at_a_character_and_counted_as_json_writes_it() {
        let text = "画".repeat(20_000);
        for piece in pieces(&text) {
            assert!(piece.len() <= PIECE_BYTES && piece.chars().all(|c| c == '画'));
        }
        let widest = pieces(&"\u{1}".repeat(10_000)).iter().map(|piece| piece.len()).max();
        assert_eq!(widest, Some(PIECE_BYTES / 6));
        assert_eq!(pieces(&"\n".repeat(PIECE_BYTES)).len(), 2);
    }

    #[test]
    fn pieces_with_one_name_are_one_file_in_the_order_files_first_came() {
        let mut parts = Parts::default();
        parts.piece("b.txt", "one ");
        parts.piece("a.txt", "x");
        parts.piece("b.txt", "two");
        parts.miss("c.txt", "none");
        assert_eq!(
            parts.files,
            [("b.txt".to_owned(), "one two".to_owned()), ("a.txt".to_owned(), "x".to_owned())]
        );
        assert_eq!(parts.text("a.txt"), Some("x"));
        assert_eq!(parts.text("z"), None);
        assert_eq!(parts.missing, [("c.txt".to_owned(), "none".to_owned())]);
    }
}
