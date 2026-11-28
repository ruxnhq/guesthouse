// SPDX-License-Identifier: GPL-2.0-or-later
//! A display's picture as a file, and a mode list as text.

use std::io::{self, Write};

/// Writes a picture as a PPM file's bytes. `pixels` is four bytes a pixel, blue first, as the
/// display hands it out; the fourth byte is left out.
pub fn write_ppm(out: &mut impl Write, width: u32, height: u32, pixels: &[u8]) -> io::Result<()> {
    if pixels.len() as u64 != u64::from(width) * u64::from(height) * 4 {
        return Err(io::Error::other("the picture's bytes don't fit its size"));
    }
    write!(out, "P6\n{width} {height}\n255\n")?;
    let mut row = Vec::with_capacity(width as usize * 3);
    for line in pixels.chunks_exact(width as usize * 4) {
        row.clear();
        for [blue, green, red, _] in line.as_chunks::<4>().0 {
            row.extend_from_slice(&[*red, *green, *blue]);
        }
        out.write_all(&row)?;
    }
    Ok(())
}

/// A mode as it is written: width, height and refresh rate.
pub type ModeText = (u16, u16, u16);

/// Reads a mode list of the form `1920x1080@60,1280x800@60`. The first is the one a guest should
/// choose.
pub fn read_modes(text: &str) -> Result<Vec<ModeText>, String> {
    let number = |part: &str| part.parse::<u16>().ok().filter(|&value| value != 0);
    text.split(',')
        .map(|mode| {
            let (size, rate) = mode.split_once('@')?;
            let (width, height) = size.split_once('x')?;
            Some((number(width)?, number(height)?, number(rate)?))
        })
        .collect::<Option<Vec<ModeText>>>()
        .filter(|modes| !modes.is_empty())
        .ok_or_else(|| format!("'{text}' is no list of modes like 1920x1080@60,1280x800@60"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_written_red_first_and_without_its_fourth_byte() {
        let mut file = Vec::new();
        write_ppm(&mut file, 2, 1, &[1, 2, 3, 255, 4, 5, 6, 0]).expect("written");
        assert_eq!(file, b"P6\n2 1\n255\n\x03\x02\x01\x06\x05\x04");
        assert!(write_ppm(&mut file, 2, 2, &[0; 8]).is_err());
    }

    #[test]
    fn a_mode_list_is_read_in_its_order() {
        assert_eq!(read_modes("1920x1080@60,640x480@75"), Ok(vec![(1920, 1080, 60), (640, 480, 75)]));
        for broken in ["", "1920x1080", "1920@60", "0x480@60", "640x480@60,", "640x480@sixty", "70000x480@60"]
        {
            assert!(read_modes(broken).is_err(), "{broken}");
        }
    }
}
