//! Whether a picture a guest sends can be an icon. It ends up as a file that the desktop's image
//! loaders open, so only a PNG that is whole and plain is taken: its signature, a sane header, chunks of
//! the usual kinds whose lengths and checksums hold, picture data, and the end, with nothing after.

/// The most an icon may take, in bytes, and the most it may measure each way.
pub const ICON_MOST_BYTES: usize = 256 << 10;
const ICON_MOST_SIDE: u32 = 512;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
/// The chunks a plain icon has. Text, profiles and anything unknown are not taken.
const ALLOWED: [&[u8; 4]; 10] =
    [b"PLTE", b"tRNS", b"gAMA", b"cHRM", b"sRGB", b"sBIT", b"pHYs", b"bKGD", b"IDAT", b"IEND"];
const MOST_CHUNKS: usize = 256;

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Whether the header's colour type and bit depth go together, as PNG has them.
fn depth_fits(colour: u8, depth: u8) -> bool {
    match colour {
        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(depth, 8 | 16),
        _ => false,
    }
}

/// Whether these bytes are a PNG of a sane size that is whole: each side from 1 to 512, a header that
/// makes sense, only the usual chunks each with the checksum it says, at least one of picture data,
/// IEND last and nothing after it.
pub fn valid_icon(png: &[u8]) -> bool {
    if png.len() > ICON_MOST_BYTES || png.len() < 8 || png[..8] != PNG_SIGNATURE {
        return false;
    }
    let mut at = 8;
    let (mut first, mut data, mut chunks) = (true, false, 0);
    while at + 12 <= png.len() {
        let length = u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]) as usize;
        let name = &png[at + 4..at + 8];
        let end = at + 8 + length + 4;
        chunks += 1;
        if end > png.len() || chunks > MOST_CHUNKS {
            return false;
        }
        let body = &png[at + 8..at + 8 + length];
        let sum = u32::from_be_bytes([png[end - 4], png[end - 3], png[end - 2], png[end - 1]]);
        if crc32(&png[at + 4..at + 8 + length]) != sum {
            return false;
        }
        if first {
            let side = |value: &[u8]| {
                let value = u32::from_be_bytes([value[0], value[1], value[2], value[3]]);
                (1..=ICON_MOST_SIDE).contains(&value)
            };
            let fine = name == b"IHDR"
                && length == 13
                && side(&body[0..4])
                && side(&body[4..8])
                && depth_fits(body[9], body[8])
                && body[10] == 0
                && body[11] == 0
                && body[12] <= 1;
            if !fine {
                return false;
            }
            first = false;
        } else if name == b"IEND" {
            return length == 0 && data && end == png.len();
        } else if ALLOWED.iter().all(|allowed| allowed.as_slice() != name) {
            return false;
        } else if name == b"IDAT" {
            data = true;
        }
        at = end;
    }
    false
}

/// A whole PNG of this size for a test: a header, a little picture data, and the end.
#[cfg(test)]
pub fn sample_png(width: u32, height: u32) -> Vec<u8> {
    sample_png_with(width, height, 0)
}

/// The same with this many more bytes of picture data.
#[cfg(test)]
pub fn sample_png_with(width: u32, height: u32, more: usize) -> Vec<u8> {
    let chunk = |name: &[u8; 4], body: &[u8]| {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        let mut checked = name.to_vec();
        checked.extend(body);
        out.extend(&checked);
        out.extend(crc32(&checked).to_be_bytes());
        out
    };
    let mut header = width.to_be_bytes().to_vec();
    header.extend(height.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    let mut png = PNG_SIGNATURE.to_vec();
    png.extend(chunk(b"IHDR", &header));
    let mut data = vec![0x78, 0x9c, 1, 2, 3, 4];
    data.resize(6 + more, 7);
    png.extend(chunk(b"IDAT", &data));
    png.extend(chunk(b"IEND", &[]));
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        let mut checked = name.to_vec();
        checked.extend(body);
        out.extend(&checked);
        out.extend(crc32(&checked).to_be_bytes());
        out
    }

    /// The icon with one more chunk put before its end.
    fn with_chunk(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut png = sample_png(48, 48);
        let end = png.split_off(png.len() - 12);
        png.extend(chunk(name, body));
        png.extend(end);
        png
    }

    #[test]
    fn a_whole_small_png_is_an_icon() {
        assert!(valid_icon(&sample_png(48, 48)));
        assert!(valid_icon(&sample_png(512, 1)));
        assert!(valid_icon(&with_chunk(b"pHYs", &[0; 9])));
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
    }

    #[test]
    fn what_is_not_a_png_is_not() {
        assert!(!valid_icon(b""));
        assert!(!valid_icon(
            b"<svg xmlns='http://www.w3.org/2000/svg'/> and some more bytes to be long enough"
        ));
        let mut wrong = sample_png(48, 48);
        wrong[1] = b'X';
        assert!(!valid_icon(&wrong));
    }

    #[test]
    fn a_png_cut_short_or_followed_by_more_is_not_whole() {
        let whole = sample_png(48, 48);
        assert!(!valid_icon(&whole[..whole.len() - 1]));
        assert!(!valid_icon(&whole[..20]));
        assert!(!valid_icon(&PNG_SIGNATURE));
        let mut more = whole.clone();
        more.push(0);
        assert!(!valid_icon(&more));
    }

    #[test]
    fn a_picture_that_is_huge_or_has_no_area_is_not_an_icon() {
        for (w, h) in [(513, 48), (48, 513), (65_000, 65_000), (0, 48), (48, 0)] {
            assert!(!valid_icon(&sample_png(w, h)), "{w}x{h}");
        }
    }

    #[test]
    fn a_png_over_the_byte_cap_is_not_an_icon() {
        let big = with_chunk(b"IDAT", &vec![7; ICON_MOST_BYTES]);
        assert!(!valid_icon(&big));
    }

    #[test]
    fn a_checksum_that_is_wrong_or_a_chunk_that_is_not_plain_is_not_an_icon() {
        let mut bad = sample_png(48, 48);
        bad[20] ^= 1;
        assert!(!valid_icon(&bad), "the header's checksum");
        assert!(!valid_icon(&with_chunk(b"tEXt", b"k\0v")));
        assert!(!valid_icon(&with_chunk(b"iCCP", &[0; 4])));
        let mut no_data = PNG_SIGNATURE.to_vec();
        let mut header = 48u32.to_be_bytes().to_vec();
        header.extend(48u32.to_be_bytes());
        header.extend([8, 6, 0, 0, 0]);
        no_data.extend(chunk(b"IHDR", &header));
        no_data.extend(chunk(b"IEND", &[]));
        assert!(!valid_icon(&no_data), "no picture data");
        let mut odd = sample_png(48, 48);
        odd[24] = 5;
        assert!(!valid_icon(&odd), "a depth the colour type does not have");
    }
}
