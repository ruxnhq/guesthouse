//! A screen's pixels as QEMU's display gives them: a whole frame, rectangles of one, or a file of
//! shared memory that QEMU draws in. Nothing here needs GTK, so a test can check it alone.

use rustix::io::pread;
use std::os::fd::OwnedFd;

const BYTES_PER_PIXEL: usize = 4;
/// Pixman's names for the formats QEMU uses.
pub const PIXMAN_X8R8G8B8: u32 = 0x2002_0888;
pub const PIXMAN_A8R8G8B8: u32 = 0x2002_8888;

pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
    pub pixels: Vec<u8>,
    /// QEMU's shared memory and where the picture starts in it, if it draws there.
    shared: Option<(OwnedFd, u64)>,
}

/// The most a side of a picture may be.
pub const MOST_SIDE: u32 = 16_384;

/// The bytes a picture of this size takes, or `None` for a size that has no picture: a side of
/// nothing or past [`MOST_SIDE`], or rows too short for the width.
pub fn picture_bytes(width: u32, height: u32, stride: u32) -> Option<usize> {
    let sides = 1..=MOST_SIDE;
    let row = (width as usize).checked_mul(BYTES_PER_PIXEL)?;
    (sides.contains(&width) && sides.contains(&height) && stride as usize >= row)
        .then(|| (stride as usize).checked_mul(height as usize))
        .flatten()
        .filter(|bytes| *bytes <= MOST_SIDE as usize * MOST_SIDE as usize * BYTES_PER_PIXEL)
}

impl Surface {
    /// `None` for a picture its own data can't hold.
    pub fn new(width: u32, height: u32, stride: u32, format: u32, pixels: Vec<u8>) -> Option<Self> {
        let size = picture_bytes(width, height, stride)?;
        (pixels.len() >= size).then_some(Self { width, height, stride, format, pixels, shared: None })
    }

    /// `None` for a size that has no picture.
    pub fn shared(
        width: u32,
        height: u32,
        stride: u32,
        format: u32,
        file: OwnedFd,
        offset: u64,
    ) -> Option<Self> {
        let pixels = vec![0; picture_bytes(width, height, stride)?];
        Some(Self { width, height, stride, format, pixels, shared: Some((file, offset)) })
    }

    /// Copies in a rectangle. One that isn't for this surface is left out, as QEMU sends a whole frame
    /// after any change of surface.
    pub fn write(&mut self, x: i32, y: i32, width: i32, height: i32, stride: u32, data: &[u8]) {
        let fits_x = x >= 0 && width >= 0 && i64::from(x) + i64::from(width) <= i64::from(self.width);
        let fits_y = y >= 0 && height >= 0 && i64::from(y) + i64::from(height) <= i64::from(self.height);
        if !fits_x || !fits_y {
            return;
        }
        let row_bytes = width as usize * BYTES_PER_PIXEL;
        let wanted = (height as usize).saturating_sub(1) * stride as usize + row_bytes;
        if data.len() < wanted {
            return;
        }
        for row in 0..height as usize {
            let from = row * stride as usize;
            let to = (y as usize + row) * self.stride as usize + x as usize * BYTES_PER_PIXEL;
            self.pixels[to..to + row_bytes].copy_from_slice(&data[from..from + row_bytes]);
        }
    }

    /// Brings the pixels up to date with shared memory, if this surface has any.
    pub fn refresh(&mut self) {
        if let Some((file, offset)) = &self.shared {
            let mut done = 0;
            while done < self.pixels.len() {
                match pread(file, &mut self.pixels[done..], offset + done as u64) {
                    Ok(read) if read > 0 => done += read,
                    _ => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fs::{MemfdFlags, memfd_create};
    use rustix::io::pwrite;

    fn surface() -> Surface {
        Surface::new(4, 2, 16, PIXMAN_X8R8G8B8, vec![0; 32]).unwrap()
    }

    #[test]
    fn a_frame_too_short_for_its_size_is_refused() {
        assert!(Surface::new(4, 2, 16, PIXMAN_X8R8G8B8, vec![0; 31]).is_none());
    }

    #[test]
    fn a_rectangle_lands_where_it_says() {
        let mut surface = surface();
        surface.write(1, 1, 2, 1, 8, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(surface.pixels[20..28], [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(surface.pixels[..20], [0; 20]);
    }

    #[test]
    fn rows_are_taken_by_their_own_stride() {
        let mut surface = surface();
        let data = [1, 1, 1, 1, 9, 9, 9, 9, 2, 2, 2, 2];
        surface.write(0, 0, 1, 2, 8, &data);
        assert_eq!(surface.pixels[..4], [1; 4]);
        assert_eq!(surface.pixels[16..20], [2; 4]);
    }

    #[test]
    fn a_rectangle_outside_the_surface_changes_nothing() {
        let mut surface = surface();
        for (x, y, width, height) in [(3, 0, 2, 1), (0, 1, 1, 2), (-1, 0, 1, 1), (0, 0, -1, 1)] {
            surface.write(x, y, width, height, 16, &[7; 64]);
        }
        surface.write(0, 0, 4, 2, 16, &[7; 16]);
        assert_eq!(surface.pixels, [0; 32], "short data counts too");
    }

    #[test]
    fn shared_memory_is_read_from_its_offset() {
        let file = memfd_create("screen", MemfdFlags::CLOEXEC).unwrap();
        pwrite(&file, &[5; 40], 0).unwrap();
        let mut surface = Surface::shared(4, 2, 16, PIXMAN_A8R8G8B8, file, 8).unwrap();
        surface.refresh();
        assert_eq!(surface.pixels, [5; 32]);
    }

    #[test]
    fn a_size_that_has_no_picture_is_refused_before_anything_is_made() {
        assert_eq!(picture_bytes(4, 2, 16), Some(32));
        assert_eq!(picture_bytes(4, 2, 15), None, "rows shorter than the width");
        assert_eq!(picture_bytes(0, 2, 16), None);
        assert_eq!(picture_bytes(4, 0, 16), None);
        assert_eq!(picture_bytes(MOST_SIDE + 1, 1, u32::MAX), None);
        assert_eq!(picture_bytes(4, MOST_SIDE + 1, 16), None);
        assert_eq!(picture_bytes(4, MOST_SIDE, u32::MAX), None, "more bytes than the largest picture");
        assert_eq!(picture_bytes(MOST_SIDE, MOST_SIDE, MOST_SIDE * 4), Some(1 << 30));
        let file = memfd_create("screen", MemfdFlags::CLOEXEC).unwrap();
        assert!(Surface::shared(4, 2, 15, PIXMAN_A8R8G8B8, file, 0).is_none());
        let file = memfd_create("screen", MemfdFlags::CLOEXEC).unwrap();
        assert!(Surface::shared(1 << 20, 1 << 20, 1 << 22, PIXMAN_A8R8G8B8, file, 0).is_none());
        assert!(Surface::new(4, 2, 15, PIXMAN_X8R8G8B8, vec![0; 64]).is_none());
    }

    #[test]
    fn a_rectangle_at_the_end_of_the_number_line_changes_nothing() {
        let mut surface = surface();
        surface.write(i32::MAX, 0, 1, 1, 4, &[7; 4]);
        surface.write(0, i32::MAX, 1, 1, 4, &[7; 4]);
        assert_eq!(surface.pixels, [0; 32]);
    }
}
