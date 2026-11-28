//! A rectangle of the guest's screen: the part of the frame that is one program window's picture.
//! The own surface is given it as its viewport's source, so the rest of the frame is not shown.

/// A rectangle of the guest's frame, in the frame's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Crop {
    /// The part of it that is on a frame of this size, if any is. A guest can change its screen's
    /// size under a rectangle that was right for the old one, and the own surface's viewport must
    /// never reach past its buffer: the compositor ends the whole connection for that.
    pub fn within(self, (frame_width, frame_height): (i32, i32)) -> Option<Self> {
        let right = (self.x + self.width).min(frame_width);
        let bottom = (self.y + self.height).min(frame_height);
        let (width, height) = (right - self.x, bottom - self.y);
        (width > 0 && height > 0).then_some(Self { width, height, ..self })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: (i32, i32) = (1920, 1080);

    #[test]
    fn a_rectangle_keeps_only_what_is_on_the_frame() {
        let inside = Crop { x: 100, y: 100, width: 320, height: 500 };
        assert_eq!(inside.within(FRAME), Some(inside));
        let over = Crop { x: 1800, y: 1000, width: 320, height: 500 };
        assert_eq!(over.within(FRAME), Some(Crop { x: 1800, y: 1000, width: 120, height: 80 }));
        assert_eq!(Crop { x: 1920, y: 0, width: 10, height: 10 }.within(FRAME), None);
    }
}
