//! The XML a WebDAV client expects.

use crate::share::anchored::Info;
use crate::share::inside::quote;
use jiff::Timestamp;

/// File times Windows sets, and is told it has, which it needs to finish a copy.
const WINDOWS_TIMES: [&str; 4] =
    ["Win32CreationTime", "Win32LastAccessTime", "Win32LastModifiedTime", "Win32FileAttributes"];
const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>";

pub fn http_date(seconds: i64) -> String {
    // A time jiff can't hold is none a file has, so it becomes the start of 1970.
    let time = Timestamp::from_second(seconds).unwrap_or(Timestamp::UNIX_EPOCH);
    time.strftime("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn entry(info: Info, href: &[u8]) -> String {
    let folder = info.is_folder;
    let kind = if folder { "<D:collection/>" } else { "" };
    let size = if folder {
        String::new()
    } else {
        format!("<D:getcontentlength>{}</D:getcontentlength>", info.size)
    };
    let slash = if folder && !href.ends_with(b"/") { "/" } else { "" };
    format!(
        "<D:response><D:href>{}{slash}</D:href><D:propstat><D:prop>\
         <D:resourcetype>{kind}</D:resourcetype>{size}\
         <D:getlastmodified>{}</D:getlastmodified>\
         <D:creationdate>{}</D:creationdate>\
         </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
        escape(&quote(href)),
        http_date(info.modified),
        http_date(info.changed),
    )
}

pub fn xml_response(entries: &str) -> String {
    format!("{XML_HEADER}<D:multistatus xmlns:D=\"DAV:\">{entries}</D:multistatus>")
}

pub fn proppatch_body(href: &str, request: &str) -> String {
    let named = WINDOWS_TIMES.iter().filter(|name| request.contains(**name));
    let names: String = named.map(|name| format!("<{name} xmlns=\"urn:schemas-microsoft-com:\"/>")).collect();
    xml_response(&format!(
        "<D:response><D:href>{}</D:href><D:propstat><D:prop>{names}</D:prop>\
         <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
        escape(href)
    ))
}

pub fn lock_body(token: &str) -> String {
    format!(
        "{XML_HEADER}<D:prop xmlns:D=\"DAV:\"><D:lockdiscovery><D:activelock>\
         <D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/></D:lockscope><D:depth>0</D:depth>\
         <D:timeout>Second-3600</D:timeout><D:locktoken><D:href>{token}</D:href></D:locktoken>\
         </D:activelock></D:lockdiscovery></D:prop>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_is_written_the_way_http_wants_it() {
        // The example date from the HTTP standard.
        assert_eq!(http_date(784_111_777), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
    }

    #[test]
    fn a_time_too_far_off_to_hold_becomes_the_start_of_1970() {
        assert_eq!(http_date(i64::MAX), "Thu, 01 Jan 1970 00:00:00 GMT");
    }
}
