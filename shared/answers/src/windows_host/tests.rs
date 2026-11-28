//! The Windows host's pieces, as its own tests in `host-windows` held them. Its answer files are
//! pinned byte for byte there, in `tests/answers-golden/`.

use super::ubuntu::account;
use super::*;

#[test]
fn the_answer_file_makes_the_named_administrator() {
    let file = answer_file("guest", "p&ss<word>");
    assert!(file.contains("<Name>guest</Name>"));
    assert!(file.contains("<Value>p&amp;ss&lt;word&gt;</Value>"));
    assert!(file.contains("<Group>Administrators</Group>"));
}

#[test]
fn the_escape_leaves_the_apostrophe() {
    assert_eq!(escape("a<b>&\"c'"), "a&lt;b&gt;&amp;&quot;c'");
    assert_eq!(escape("&amp;"), "&amp;amp;");
}

#[test]
fn the_account_line_is_user_then_password() {
    assert_eq!(account("ana", "0123abcd"), "ana 0123abcd\n");
}
