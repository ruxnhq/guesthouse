//! A Windows guest's `autounattend.xml` on the Linux host: Setup from a disc, which partitions the
//! disk, picks the edition by its image's name and its key, names the host's time zone, runs the
//! answer disc's `SetupComplete.cmd`, and makes one administrator who signs in by itself.

use super::editions::{WINDOWS_EDITION, edition_key};
use super::xml_text;

/// The template, as `guest/windows/autounattend.xml` holds it. The host reads its own copy from the program
/// folder when it installs.
pub const TEMPLATE: &str = include_str!("../../../../guest/windows/autounattend.xml");

/// Where the template takes the account's password, and its name. The guest's own scripts name the
/// account by the same mark.
pub const PASSWORD_MARK: &str = "@PASSWORD@";
pub const USER_MARK: &str = "@USER@";

/// The answer file from `template`, for a host whose zone Windows names `zone_name`, making this
/// account. The template says UTC. The edition is the disc's own when it was read, and Pro when it
/// wasn't, as it always was. Its key tells Setup the edition so it doesn't stop to ask.
pub fn fill(template: &str, zone_name: &str, user: &str, password: &str, edition: Option<&str>) -> String {
    let name = zone_name;
    let edition = edition.unwrap_or(WINDOWS_EDITION);
    let key = edition_key(edition).map_or(String::new(), |key| {
        format!("<ProductKey><Key>{key}</Key><WillShowUI>OnError</WillShowUI></ProductKey>")
    });
    let filled = template
        .replace("<TimeZone>UTC<", &format!("<TimeZone>{name}<"))
        .replace("@EDITION@", &xml_text(edition))
        .replace("@PRODUCT_KEY@", &key);
    filled.replace(USER_MARK, &xml_text(user)).replace(PASSWORD_MARK, &xml_text(password))
}
