//! A WIM's XML block, which lists the images in a Windows disc's `install.wim` or `install.esd`, read
//! from a stream of the file as the Linux host gets it from 7z.

use std::io::Read;

pub const FIRST_WINDOWS_11_BUILD: u32 = 22000;
pub const WIM_AMD64: &str = "9";
pub const WIM_MAGIC: &[u8; 8] = b"MSWIM\0\0\0";
pub const WIM_HEADER_SIZE: usize = 208;
/// Where the header says the XML block is: its size, then its offset.
pub const WIM_XML_ENTRY: usize = 0x48;
/// The top byte of the size holds flags.
const WIM_SIZE_MASK: u64 = (1 << 56) - 1;
/// The XML block is a few hundred KiB. The size comes from the file, so it's held to something sane.
pub const MOST_WIM_XML: u64 = 64 << 20;
const SKIP_CHUNK: usize = 1 << 24;

/// The text between `<tag>` and `</tag>`.
pub fn field<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let start = text.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = text[start..].find(&format!("</{tag}>"))?;
    Some(&text[start..start + end])
}

/// Each image's name, build and architecture, from a WIM's XML block.
pub fn images_in(text: &str) -> Vec<(String, String, u32, String)> {
    let mut images = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<IMAGE ") {
        let Some(end) = rest[start..].find("</IMAGE>") else { break };
        let image = &rest[start..start + end];
        let build = field(image, "BUILD").and_then(|text| text.parse().ok());
        if let (Some(name), Some(build), Some(arch)) = (field(image, "NAME"), build, field(image, "ARCH")) {
            let shown = field(image, "DISPLAYNAME").unwrap_or(name);
            images.push((name.to_owned(), shown.to_owned(), build, arch.to_owned()));
        }
        rest = &rest[start + end..];
    }
    images
}

pub fn utf16_text(data: &[u8]) -> String {
    let units: Vec<u16> = data.as_chunks().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    String::from_utf16_lossy(&units)
}

/// The XML block of a WIM or ESD, which is near its end. 7z can only stream the file, which takes
/// seconds. `None` if this isn't a WIM.
pub fn wim_xml(stream: &mut impl Read) -> Option<String> {
    let mut header = [0; WIM_HEADER_SIZE];
    stream.read_exact(&mut header).ok()?;
    if &header[..WIM_MAGIC.len()] != WIM_MAGIC {
        return None;
    }
    let number = |at: usize| Some(u64::from_le_bytes(header[at..at + 8].try_into().ok()?));
    let size = (number(WIM_XML_ENTRY)? & WIM_SIZE_MASK).min(MOST_WIM_XML);
    let mut left = number(WIM_XML_ENTRY + 8)?.checked_sub(WIM_HEADER_SIZE as u64)?;
    let mut chunk = vec![0; SKIP_CHUNK];
    while left > 0 {
        let wanted = left.min(SKIP_CHUNK as u64) as usize;
        match stream.read(&mut chunk[..wanted]) {
            Ok(0) | Err(_) => return None,
            Ok(read) => left -= read as u64,
        }
    }
    let mut data = Vec::new();
    stream.take(size).read_to_end(&mut data).ok()?;
    Some(utf16_text(&data))
}
