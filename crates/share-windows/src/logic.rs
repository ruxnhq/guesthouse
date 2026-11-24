//! The parts of the share program that are only arithmetic and rules: what a 9P error is as an
//! NTSTATUS, what a Unix time is as a FILETIME, what a WinFsp path is as 9P names, which Linux open
//! flags an access mask is, the order a directory is listed in, and the command line. Nothing here
//! calls Windows, so all of it is tested on Linux.

use guesthouse_p9::{Attr, Errno, Error, getattr, mode, open};
use std::path::PathBuf;

/// NTSTATUS values, as `i32`, which is what WinFsp's callbacks return. On Windows each is checked
/// against `windows-sys`'s own.
pub mod status {
    pub const SUCCESS: i32 = 0;
    pub const BUFFER_OVERFLOW: i32 = 0x8000_0005_u32 as i32;
    pub const INVALID_PARAMETER: i32 = 0xC000_000D_u32 as i32;
    pub const END_OF_FILE: i32 = 0xC000_0011_u32 as i32;
    pub const ACCESS_DENIED: i32 = 0xC000_0022_u32 as i32;
    pub const OBJECT_NAME_INVALID: i32 = 0xC000_0033_u32 as i32;
    pub const OBJECT_NAME_NOT_FOUND: i32 = 0xC000_0034_u32 as i32;
    pub const OBJECT_NAME_COLLISION: i32 = 0xC000_0035_u32 as i32;
    pub const OBJECT_PATH_NOT_FOUND: i32 = 0xC000_003A_u32 as i32;
    pub const DISK_FULL: i32 = 0xC000_007F_u32 as i32;
    pub const FILE_IS_A_DIRECTORY: i32 = 0xC000_00BA_u32 as i32;
    pub const MEDIA_WRITE_PROTECTED: i32 = 0xC000_00A2_u32 as i32;
    pub const DIRECTORY_NOT_EMPTY: i32 = 0xC000_0101_u32 as i32;
    pub const NOT_A_DIRECTORY: i32 = 0xC000_0103_u32 as i32;
    pub const IO_DEVICE_ERROR: i32 = 0xC000_0185_u32 as i32;
}

/// The NTSTATUS for a Linux errno from the 9P server.
pub fn errno_status(errno: Errno) -> i32 {
    match errno {
        Errno::NoEntry => status::OBJECT_NAME_NOT_FOUND,
        Errno::Exists => status::OBJECT_NAME_COLLISION,
        Errno::AccessDenied => status::ACCESS_DENIED,
        Errno::NotEmpty => status::DIRECTORY_NOT_EMPTY,
        Errno::NotDir => status::NOT_A_DIRECTORY,
        Errno::IsDir => status::FILE_IS_A_DIRECTORY,
        Errno::NoSpace => status::DISK_FULL,
        Errno::ReadOnly => status::MEDIA_WRITE_PROTECTED,
        Errno::Invalid | Errno::Io | Errno::Other(_) => status::IO_DEVICE_ERROR,
    }
}

/// The NTSTATUS for any 9P client error: the server's no is its errno's status, and anything else,
/// a broken stream or a server that breaks the protocol, is an I/O error.
pub fn error_status(error: &Error) -> i32 {
    match error.errno() {
        Some(errno) => errno_status(errno),
        None => status::IO_DEVICE_ERROR,
    }
}

/// Whether an error means the connection can't be trusted any more: anything but the server
/// answering no to one request.
pub fn is_fatal(error: &Error) -> bool {
    !matches!(error, Error::Remote(_))
}

/// Seconds between 1601-01-01, where FILETIME starts, and 1970-01-01.
const EPOCH_DIFFERENCE: u64 = 11_644_473_600;
const TICKS_PER_SECOND: u64 = 10_000_000;

/// A Unix time as a FILETIME: 100 ns ticks since 1601. A time too far ahead to be one is the latest.
pub fn unix_to_filetime(sec: u64, nsec: u64) -> u64 {
    sec.saturating_add(EPOCH_DIFFERENCE)
        .saturating_mul(TICKS_PER_SECOND)
        .saturating_add(nsec.min(999_999_999) / 100)
}

/// A FILETIME as seconds and nanoseconds since 1970. One before 1970 is 1970.
pub fn filetime_to_unix(filetime: u64) -> (u64, u64) {
    let ticks = filetime.saturating_sub(EPOCH_DIFFERENCE * TICKS_PER_SECOND);
    (ticks / TICKS_PER_SECOND, (ticks % TICKS_PER_SECOND) * 100)
}

pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
pub const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;

/// The sector size the volume has, and the unit a file's allocation is counted in.
pub const SECTOR_SIZE: u64 = 4096;

/// What WinFsp's `FSP_FSCTL_FILE_INFO` holds, from a 9P `getattr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileInfoData {
    pub attributes: u32,
    pub allocation_size: u64,
    pub file_size: u64,
    pub creation_time: u64,
    pub access_time: u64,
    pub write_time: u64,
    pub change_time: u64,
    pub index: u64,
}

pub fn is_dir(attr: &Attr) -> bool {
    attr.mode & mode::S_IFMT == mode::S_IFDIR
}

/// A file's information. A directory is a directory with no size, as NTFS has it; anything else is
/// a file with the archive bit, and one with a dot in front isn't hidden. The server doesn't say
/// when a file was created, so that is when it last changed, unless it does say.
pub fn file_info(attr: &Attr) -> FileInfoData {
    let dir = is_dir(attr);
    let size = if dir { 0 } else { attr.size };
    let ft = |t: guesthouse_p9::Timespec| unix_to_filetime(t.sec, t.nsec);
    let creation =
        if attr.valid & getattr::BTIME != 0 && attr.btime.sec != 0 { attr.btime } else { attr.mtime };
    FileInfoData {
        attributes: if dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_ARCHIVE },
        allocation_size: if dir { 0 } else { size.div_ceil(SECTOR_SIZE).saturating_mul(SECTOR_SIZE) },
        file_size: size,
        creation_time: ft(creation),
        access_time: ft(attr.atime),
        write_time: ft(attr.mtime),
        change_time: ft(attr.ctime),
        index: attr.qid.path,
    }
}

/// The names of a WinFsp path, `\a\b` as `["a", "b"]` and the root `\` as none. A name that is `.`
/// or `..`, or holds `/` or a NUL, is refused: the 9P server would take them as a way out of the
/// share.
pub fn split_path(path: &str) -> Result<Vec<String>, i32> {
    let mut names = Vec::new();
    for name in path.split('\\').filter(|name| !name.is_empty()) {
        if name == "." || name == ".." || name.contains(['/', '\0']) {
            return Err(status::OBJECT_NAME_INVALID);
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

/// Text from a WinFsp wide string's units. Units that aren't valid UTF-16 can't be a name on the
/// host, and are refused.
pub fn utf16_name(units: &[u16]) -> Result<String, i32> {
    String::from_utf16(units).map_err(|_| status::OBJECT_NAME_INVALID)
}

/// Windows access bits: `FILE_READ_DATA` and the like.
const FILE_WRITE_DATA: u32 = 0x2;
const FILE_APPEND_DATA: u32 = 0x4;

/// The Linux flags to `lopen` a file with, for the access Windows granted. Writing, in either of its
/// two forms, is read-write, since Windows may read a file it has write access to. A read-only
/// share opens everything for reading whatever was granted, and refuses the writes themselves.
/// Never `O_TRUNC`, as overwrite is its own call, nor `O_APPEND`, as WinFsp gives every offset.
pub fn open_flags(granted_access: u32, is_dir: bool, read_only: bool) -> u32 {
    if is_dir {
        return open::O_RDONLY | open::O_DIRECTORY;
    }
    if !read_only && granted_access & (FILE_WRITE_DATA | FILE_APPEND_DATA) != 0 {
        open::O_RDWR
    } else {
        open::O_RDONLY
    }
}

/// Where a write goes and how much of it, or none of it. Constrained I/O, which is the cache
/// manager flushing, never grows the file; otherwise a write to the end starts at the size.
pub fn plan_write(
    write_to_end: bool,
    constrained: bool,
    offset: u64,
    len: usize,
    size: u64,
) -> Option<(u64, usize)> {
    let (offset, len) = if constrained {
        if offset >= size {
            return None;
        }
        (offset, len.min(usize::try_from(size - offset).unwrap_or(usize::MAX)))
    } else if write_to_end {
        (size, len)
    } else {
        (offset, len)
    };
    if len == 0 || offset.checked_add(len as u64).is_none() { None } else { Some((offset, len)) }
}

/// A directory's entries in the order they're given to WinFsp, which finds its place again by name
/// with the `marker`: `.` and `..` first, and only for a directory that isn't the root, then the
/// rest by their UTF-16 units. Whatever `.` and `..` the server sent are dropped, and a name sent
/// twice is one. With a marker, the entries after it, and not it.
pub fn order_directory(names: Vec<String>, is_root: bool, marker: Option<&str>) -> Vec<String> {
    let mut rest: Vec<String> = names.into_iter().filter(|n| n != "." && n != "..").collect();
    rest.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    rest.dedup();
    let mut all = Vec::with_capacity(rest.len() + 2);
    if !is_root {
        all.push(".".to_owned());
        all.push("..".to_owned());
    }
    all.extend(rest);
    match marker {
        None => all,
        Some(marker) => {
            let rank = |n: &str| match n {
                "." => 0,
                ".." => 1,
                _ => 2,
            };
            all.into_iter()
                .filter(|n| {
                    rank(n)
                        .cmp(&rank(marker))
                        .then_with(|| {
                            if rank(n) == 2 {
                                n.encode_utf16().cmp(marker.encode_utf16())
                            } else {
                                std::cmp::Ordering::Equal
                            }
                        })
                        .is_gt()
                })
                .collect()
        }
    }
}

/// The volume's size and free space in bytes from a `statfs`, which counts in blocks.
pub fn volume_sizes(block_size: u32, blocks: u64, available: u64) -> (u64, u64) {
    let unit = if block_size == 0 { SECTOR_SIZE } else { u64::from(block_size) };
    (blocks.saturating_mul(unit), available.min(blocks).saturating_mul(unit))
}

/// A self-relative security descriptor in SDDL for everything on the share: SYSTEM,
/// Administrators and Authenticated Users have full control, or on a read-only share
/// Authenticated Users read and execute only. WinFsp's volume has no ACLs of its own to keep.
pub fn security_descriptor_sddl(read_only: bool) -> &'static str {
    if read_only {
        "O:BAG:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFX;;;AU)"
    } else {
        "O:BAG:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;AU)"
    }
}

/// The parts of a Hyper-V service ID for a vsock `port`: `{PORT-facb-11e6-bd58-64006a7986d3}` as a
/// GUID's `Data1`, `Data2`, `Data3` and `Data4`.
pub fn vsock_service_guid(port: u32) -> (u32, u16, u16, [u8; 8]) {
    (port, 0xFACB, 0x11E6, [0xBD, 0x58, 0x64, 0x00, 0x6A, 0x79, 0x86, 0xD3])
}

/// `HV_GUID_PARENT`, the host as a Hyper-V socket reaches it from a guest:
/// `{a42e7cda-d03f-480c-9cc2-a4de20abb878}`.
pub const HV_GUID_PARENT: (u32, u16, u16, [u8; 8]) =
    (0xa42e_7cda, 0xd03f, 0x480c, [0x9c, 0xc2, 0xa4, 0xde, 0x20, 0xab, 0xb8, 0x78]);

/// The first drive letter of S: to Z: that isn't in `used`, which is `GetLogicalDrives`' bits, A as
/// bit 0.
pub fn first_free_drive(used: u32) -> Option<String> {
    ('S'..='Z')
        .find(|letter| used & (1 << (*letter as u32 - 'A' as u32)) == 0)
        .map(|letter| format!("{letter}:"))
}

/// A drive as the user wrote it, `s`, `S` or `S:`, as `S:`.
pub fn normalize_drive(text: &str) -> Option<String> {
    let letter = text.strip_suffix(':').unwrap_or(text);
    let mut chars = letter.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(format!("{}:", c.to_ascii_uppercase())),
        _ => None,
    }
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub port: u32,
    pub share: String,
    pub read_only: bool,
    pub drive: Option<String>,
    pub log: Option<PathBuf>,
    pub check: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Run(Args),
}

pub const USAGE: &str = "\
guesthouse-guest-share --port PORT --share NAME [--read-only] [--drive LETTER:] [--log FILE] [--check]

Mounts the host folder that Hyper-V's Plan 9 server shares as NAME, which it serves on vsock PORT,
as a drive in this Windows guest, through WinFsp.

  --port PORT      the vsock port the host's Plan 9 server listens on
  --share NAME     the share's name, as the host gave it to the Plan 9 device
  --read-only      refuse every write
  --drive LETTER:  the drive to mount as; the first free of S: to Z: if not given
  --log FILE       also write what happens to FILE
  --check          only connect, attach and list the share's root, then exit
  --help           this text
";

/// Reads the command line, without the program's own name.
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut port = None;
    let mut share = None;
    let mut read_only = false;
    let mut drive = None;
    let mut log = None;
    let mut check = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--help" | "-h" | "-?" => return Ok(Command::Help),
            "--port" => {
                let text = value("--port")?;
                port =
                    Some(text.parse::<u32>().map_err(|_| format!("--port {text:?} is not a port number"))?);
            }
            "--share" => share = Some(value("--share")?),
            "--read-only" => read_only = true,
            "--drive" => {
                let text = value("--drive")?;
                drive = Some(
                    normalize_drive(&text)
                        .ok_or_else(|| format!("--drive {text:?} is not a drive letter"))?,
                );
            }
            "--log" => log = Some(PathBuf::from(value("--log")?)),
            "--check" => check = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(Command::Run(Args {
        port: port.ok_or("--port is required")?,
        share: share.filter(|s| !s.is_empty()).ok_or("--share is required")?,
        read_only,
        drive,
        log,
        check,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_p9::{Qid, Timespec};

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn errnos_become_the_statuses_the_task_names() {
        let cases = [
            (Errno::ENOENT, status::OBJECT_NAME_NOT_FOUND),
            (Errno::EEXIST, status::OBJECT_NAME_COLLISION),
            (Errno::EACCES, status::ACCESS_DENIED),
            (Errno::ENOTEMPTY, status::DIRECTORY_NOT_EMPTY),
            (Errno::ENOTDIR, status::NOT_A_DIRECTORY),
            (Errno::EISDIR, status::FILE_IS_A_DIRECTORY),
            (Errno::ENOSPC, status::DISK_FULL),
            (Errno::EROFS, status::MEDIA_WRITE_PROTECTED),
            (Errno::EIO, status::IO_DEVICE_ERROR),
            (Errno::EINVAL, status::IO_DEVICE_ERROR),
            (999, status::IO_DEVICE_ERROR),
        ];
        for (errno, want) in cases {
            assert_eq!(error_status(&Error::Remote(errno)), want, "errno {errno}");
        }
    }

    #[test]
    fn only_a_server_saying_no_leaves_the_connection_usable() {
        assert!(!is_fatal(&Error::Remote(2)));
        assert!(is_fatal(&Error::Truncated));
        assert!(is_fatal(&Error::TooLarge));
        assert_eq!(error_status(&Error::Truncated), status::IO_DEVICE_ERROR);
    }

    #[test]
    fn a_unix_time_is_a_filetime_and_back() {
        assert_eq!(unix_to_filetime(0, 0), 116_444_736_000_000_000);
        assert_eq!(unix_to_filetime(1, 500), 116_444_736_000_000_000 + 10_000_000 + 5);
        let ft = unix_to_filetime(1_700_000_000, 123_456_700);
        assert_eq!(filetime_to_unix(ft), (1_700_000_000, 123_456_700));
        // 100 ns is as fine as FILETIME goes.
        assert_eq!(filetime_to_unix(unix_to_filetime(5, 199)), (5, 100));
    }

    #[test]
    fn times_out_of_range_saturate_and_never_panic() {
        assert_eq!(unix_to_filetime(u64::MAX, u64::MAX), u64::MAX);
        assert_eq!(filetime_to_unix(0), (0, 0));
        assert_eq!(filetime_to_unix(u64::MAX).0, (u64::MAX - 116_444_736_000_000_000) / 10_000_000);
    }

    fn attr(mode: u32, size: u64) -> Attr {
        Attr {
            mode,
            size,
            qid: Qid { ty: 0, version: 0, path: 77 },
            atime: Timespec { sec: 10, nsec: 0 },
            mtime: Timespec { sec: 20, nsec: 0 },
            ctime: Timespec { sec: 30, nsec: 0 },
            ..Attr::default()
        }
    }

    #[test]
    fn a_file_is_archive_and_rounded_up_to_a_sector() {
        let info = file_info(&attr(mode::S_IFREG | 0o644, 5000));
        assert_eq!(info.attributes, FILE_ATTRIBUTE_ARCHIVE);
        assert_eq!(info.file_size, 5000);
        assert_eq!(info.allocation_size, 8192);
        assert_eq!(info.index, 77);
        assert_eq!(info.write_time, unix_to_filetime(20, 0));
        assert_eq!(info.change_time, unix_to_filetime(30, 0));
        assert_eq!(info.access_time, unix_to_filetime(10, 0));
        assert_eq!(info.creation_time, unix_to_filetime(20, 0));
        assert_eq!(file_info(&attr(mode::S_IFREG, 0)).allocation_size, 0);
        assert_eq!(file_info(&attr(mode::S_IFREG, u64::MAX)).allocation_size, u64::MAX);
    }

    #[test]
    fn a_directory_has_no_size_and_a_birth_time_is_used_when_there_is_one() {
        let info = file_info(&attr(mode::S_IFDIR | 0o755, 4096));
        assert_eq!(info.attributes, FILE_ATTRIBUTE_DIRECTORY);
        assert_eq!((info.file_size, info.allocation_size), (0, 0));
        let mut a = attr(mode::S_IFREG, 1);
        a.valid = getattr::BTIME;
        a.btime = Timespec { sec: 5, nsec: 0 };
        assert_eq!(file_info(&a).creation_time, unix_to_filetime(5, 0));
    }

    #[test]
    fn a_winfsp_path_is_its_names() {
        assert_eq!(split_path("\\"), Ok(vec![]));
        assert_eq!(split_path("\\a\\b c\\.hidden"), Ok(names(&["a", "b c", ".hidden"])));
        assert_eq!(split_path("\\a\\"), Ok(vec!["a".to_owned()]));
        assert_eq!(split_path(""), Ok(vec![]));
    }

    #[test]
    fn dots_slashes_and_nuls_are_refused() {
        for bad in ["\\..", "\\a\\..\\b", "\\.", "\\a\\.\\b", "\\a/b", "\\a\\b\0c"] {
            assert_eq!(split_path(bad), Err(status::OBJECT_NAME_INVALID), "{bad:?}");
        }
        assert_eq!(split_path("\\...\\a.."), Ok(vec!["...".to_owned(), "a..".to_owned()]));
    }

    #[test]
    fn unpaired_surrogates_are_not_names() {
        assert_eq!(utf16_name(&[0x61, 0x62]), Ok("ab".to_owned()));
        assert_eq!(utf16_name(&[0xD83D, 0xDE00]), Ok("\u{1F600}".to_owned()));
        assert_eq!(utf16_name(&[0xD800, 0x61]), Err(status::OBJECT_NAME_INVALID));
    }

    #[test]
    fn access_becomes_open_flags() {
        assert_eq!(open_flags(0x1, false, false), open::O_RDONLY);
        assert_eq!(open_flags(0x0010_0000, false, false), open::O_RDONLY);
        assert_eq!(open_flags(0x2, false, false), open::O_RDWR);
        assert_eq!(open_flags(0x4, false, false), open::O_RDWR);
        assert_eq!(open_flags(0x1_0006, false, false), open::O_RDWR);
        assert_eq!(open_flags(0x1f01ff, true, false), open::O_RDONLY | open::O_DIRECTORY);
        // A read-only share opens for reading whatever was granted.
        assert_eq!(open_flags(0x1f01ff, false, true), open::O_RDONLY);
        for access in [0, 0x2, 0x4, 0x1f01ff] {
            let flags = open_flags(access, false, false);
            assert_eq!(flags & (open::O_TRUNC | open::O_APPEND | open::O_CREAT), 0);
        }
    }

    #[test]
    fn a_write_goes_where_it_is_asked_to() {
        assert_eq!(plan_write(false, false, 10, 5, 100), Some((10, 5)));
        assert_eq!(plan_write(false, false, 200, 5, 100), Some((200, 5)));
        assert_eq!(plan_write(true, false, 0, 5, 100), Some((100, 5)));
        assert_eq!(plan_write(false, false, 0, 0, 100), None);
    }

    #[test]
    fn constrained_io_never_grows_the_file() {
        assert_eq!(plan_write(false, true, 90, 20, 100), Some((90, 10)));
        assert_eq!(plan_write(false, true, 100, 5, 100), None);
        assert_eq!(plan_write(false, true, 500, 5, 100), None);
        // Constrained wins over write-to-end.
        assert_eq!(plan_write(true, true, 10, 5, 100), Some((10, 5)));
        assert_eq!(plan_write(false, false, u64::MAX, 5, 0), None);
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_directory_lists_dots_first_unless_it_is_the_root() {
        let given = names(&["b", ".", "a", "..", "B", "a"]);
        assert_eq!(order_directory(given.clone(), false, None), names(&[".", "..", "B", "a", "b"]));
        assert_eq!(order_directory(given, true, None), names(&["B", "a", "b"]));
    }

    #[test]
    fn a_marker_resumes_after_its_entry() {
        let given = names(&["c", "a", "b"]);
        assert_eq!(order_directory(given.clone(), false, Some(".")), names(&["..", "a", "b", "c"]));
        assert_eq!(order_directory(given.clone(), false, Some("..")), names(&["a", "b", "c"]));
        assert_eq!(order_directory(given.clone(), false, Some("a")), names(&["b", "c"]));
        assert_eq!(order_directory(given.clone(), true, Some("c")), Vec::<String>::new());
        // A marker that has since gone still puts the listing back in its place.
        assert_eq!(order_directory(given, true, Some("aa")), names(&["b", "c"]));
    }

    #[test]
    fn names_order_by_utf16_units() {
        // U+FF5E is one unit above 0xD800; U+1F600 is a pair starting at 0xD83D, which is below it.
        let listed = order_directory(names(&["\u{FF5E}", "\u{1F600}", "z"]), true, None);
        assert_eq!(listed, names(&["z", "\u{1F600}", "\u{FF5E}"]));
    }

    #[test]
    fn volume_sizes_are_in_bytes_and_never_overflow() {
        assert_eq!(volume_sizes(4096, 100, 40), (409_600, 163_840));
        assert_eq!(volume_sizes(0, 1, 1), (4096, 4096));
        assert_eq!(volume_sizes(4096, 10, 50), (40_960, 40_960));
        assert_eq!(volume_sizes(u32::MAX, u64::MAX, u64::MAX), (u64::MAX, u64::MAX));
    }

    #[test]
    fn the_security_descriptor_is_read_and_execute_only_on_a_read_only_share() {
        assert!(security_descriptor_sddl(false).contains("(A;;FA;;;AU)"));
        assert!(security_descriptor_sddl(true).contains("(A;;FRFX;;;AU)"));
        assert!(security_descriptor_sddl(true).contains("(A;;FA;;;SY)(A;;FA;;;BA)"));
    }

    #[test]
    fn a_port_is_the_first_field_of_the_service_guid() {
        let (d1, d2, d3, d4) = vsock_service_guid(564);
        assert_eq!((d1, d2, d3), (564, 0xFACB, 0x11E6));
        assert_eq!(d4, [0xBD, 0x58, 0x64, 0x00, 0x6A, 0x79, 0x86, 0xD3]);
        assert_eq!(HV_GUID_PARENT.0, 0xa42e7cda);
    }

    #[test]
    fn the_default_drive_is_the_first_free_from_s() {
        assert_eq!(first_free_drive(0), Some("S:".to_owned()));
        assert_eq!(first_free_drive(1 << 18), Some("T:".to_owned()));
        assert_eq!(first_free_drive((1 << 18) | (1 << 19) | (1 << 21)), Some("U:".to_owned()));
        let s_to_z: u32 = (18..=25).map(|bit| 1u32 << bit).sum();
        assert_eq!(first_free_drive(s_to_z), None);
        // Lower letters in use don't matter.
        assert_eq!(first_free_drive(0b111), Some("S:".to_owned()));
    }

    #[test]
    fn a_drive_is_a_letter_with_or_without_a_colon() {
        assert_eq!(normalize_drive("s"), Some("S:".to_owned()));
        assert_eq!(normalize_drive("Z:"), Some("Z:".to_owned()));
        for bad in ["", ":", "SS", "S:\\", "1:", "S::"] {
            assert_eq!(normalize_drive(bad), None, "{bad:?}");
        }
    }

    fn parse(line: &str) -> Result<Command, String> {
        parse_args(words(line))
    }

    #[test]
    fn the_command_line_is_read() {
        let Command::Run(args) =
            parse("--port 564 --share files --read-only --drive t --log C:\\x.log --check").unwrap()
        else {
            panic!("not a run")
        };
        assert_eq!(args.port, 564);
        assert_eq!(args.share, "files");
        assert!(args.read_only && args.check);
        assert_eq!(args.drive.as_deref(), Some("T:"));
        assert_eq!(args.log, Some(PathBuf::from("C:\\x.log")));
        let Command::Run(args) = parse("--share a --port 1").unwrap() else { panic!() };
        assert!(!args.read_only && !args.check && args.drive.is_none() && args.log.is_none());
    }

    #[test]
    fn help_wins_and_mistakes_are_said() {
        assert_eq!(parse("--help"), Ok(Command::Help));
        assert_eq!(parse("--port 1 -h"), Ok(Command::Help));
        assert!(parse("--share a").unwrap_err().contains("--port"));
        assert!(parse("--port 1").unwrap_err().contains("--share"));
        assert!(parse("--port x --share a").unwrap_err().contains("port number"));
        assert!(parse("--port").unwrap_err().contains("needs a value"));
        assert!(parse("--port 1 --share a --drive 9").unwrap_err().contains("drive letter"));
        assert!(parse("--port 1 --share a --bogus").unwrap_err().contains("--bogus"));
        assert!(parse("--port 1 --share").unwrap_err().contains("needs a value"));
    }
}
