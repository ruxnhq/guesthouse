//! Small text helpers: shell quoting, QEMU's option values, and base64 for the guest's agent.

pub use guesthouse_basics::text::{base64_decode, base64_encode, qemu_value, shell_quote};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_stay_plain() {
        assert_eq!(shell_quote("br0"), "br0");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn base64_goes_there_and_back() {
        for (data, text) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")]
        {
            assert_eq!(base64_encode(data.as_bytes()), text);
            assert_eq!(base64_decode(text).unwrap(), data.as_bytes());
        }
        assert_eq!(base64_decode("not base64!"), None);
    }
}
