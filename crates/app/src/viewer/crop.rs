//! Lab only: one rectangle of the guest's screen as the whole of a window.
//!
//! A helper in the guest packs each program's window into a rectangle of its own on the guest's
//! screen, and this is what shows one of them by itself. The picture stays the guest's whole screen,
//! laid out larger than the window so that only the rectangle is in view. GTK clips the rest, the
//! own surface is given the rectangle as its viewport's source, and the pointer needs no mapping of
//! its own: a point on the picture is already a point on the guest's whole screen.

use super::screen_frame::Placement;

/// A rectangle of the guest's frame, in the frame's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Crop {
    /// Four whole numbers, X Y W H, apart by spaces or commas. A rectangle with no area is none.
    pub fn parse(text: &str) -> Option<Self> {
        let numbers: Vec<i32> = text
            .split([' ', ',', '\t', '\n'])
            .filter(|part| !part.is_empty())
            .map(|part| part.parse().ok())
            .collect::<Option<_>>()?;
        let [x, y, width, height] = numbers[..] else { return None };
        (x >= 0 && y >= 0 && width > 0 && height > 0).then_some(Self { x, y, width, height })
    }

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

/// Where the guest's whole frame goes so that `crop` fills `room`, in the room's logical pixels. The
/// room is normally the rectangle's own size, and then the frame is at its own size too.
pub fn picture_place(
    crop: Crop,
    (frame_width, frame_height): (i32, i32),
    (room_width, room_height): (i32, i32),
) -> Placement {
    let across = f64::from(room_width) / f64::from(crop.width);
    let down = f64::from(room_height) / f64::from(crop.height);
    Placement {
        x: (-f64::from(crop.x) * across) as f32,
        y: (-f64::from(crop.y) * down) as f32,
        width: (f64::from(frame_width) * across).round() as i32,
        height: (f64::from(frame_height) * down).round() as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::super::input::tablet_axis;
    use super::*;
    use guesthouse_app_core::input::TABLET_RANGE;

    const FRAME: (i32, i32) = (1920, 1080);

    /// What the guest is told for a point in the window: the picture's own coordinates, as the
    /// pointer's controller on the picture sees them, through the same arithmetic as `on_motion`.
    fn tablet_at(crop: Crop, room: (i32, i32), (x, y): (f64, f64)) -> (i64, i64) {
        let placed = picture_place(crop, FRAME, room);
        (
            tablet_axis(x - f64::from(placed.x), placed.width),
            tablet_axis(y - f64::from(placed.y), placed.height),
        )
    }

    /// The tablet's value for a pixel of the guest's screen.
    fn tablet_of(pixel: f64, side: i32) -> i64 {
        (pixel * TABLET_RANGE as f64 / f64::from(side)) as i64
    }

    #[test]
    fn a_rectangle_is_four_numbers_with_area() {
        let crop = Crop { x: 10, y: 20, width: 300, height: 400 };
        assert_eq!(Crop::parse("10 20 300 400"), Some(crop));
        assert_eq!(Crop::parse(" 10,20, 300 400\n"), Some(crop));
        assert_eq!(Crop::parse("10 20 300"), None);
        assert_eq!(Crop::parse("10 20 300 400 5"), None);
        assert_eq!(Crop::parse("10 20 0 400"), None);
        assert_eq!(Crop::parse("-1 20 300 400"), None);
        assert_eq!(Crop::parse("ten 20 300 400"), None);
    }

    #[test]
    fn a_rectangle_keeps_only_what_is_on_the_frame() {
        let inside = Crop { x: 100, y: 100, width: 320, height: 500 };
        assert_eq!(inside.within(FRAME), Some(inside));
        let over = Crop { x: 1800, y: 1000, width: 320, height: 500 };
        assert_eq!(over.within(FRAME), Some(Crop { x: 1800, y: 1000, width: 120, height: 80 }));
        assert_eq!(Crop { x: 1920, y: 0, width: 10, height: 10 }.within(FRAME), None);
    }

    #[test]
    fn at_its_own_size_the_frame_is_at_its_own_size_and_moved_by_the_corner() {
        let crop = Crop { x: 700, y: 300, width: 320, height: 500 };
        let placed = picture_place(crop, FRAME, (320, 500));
        assert_eq!(placed, Placement { x: -700.0, y: -300.0, width: 1920, height: 1080 });
    }

    #[test]
    fn a_point_in_the_window_is_the_same_point_of_the_rectangle_on_the_guest() {
        let crop = Crop { x: 700, y: 300, width: 320, height: 500 };
        let room = (320, 500);
        assert_eq!(tablet_at(crop, room, (0.0, 0.0)), (tablet_of(700.0, 1920), tablet_of(300.0, 1080)));
        assert_eq!(tablet_at(crop, room, (100.5, 42.0)), (tablet_of(800.5, 1920), tablet_of(342.0, 1080)));
        // The far edge of the window is the far edge of the rectangle.
        assert_eq!(tablet_at(crop, room, (320.0, 500.0)), (tablet_of(1020.0, 1920), tablet_of(800.0, 1080)));
    }

    #[test]
    fn a_rectangle_in_the_screens_corners_reaches_the_tablets_ends() {
        let top_left = Crop { x: 0, y: 0, width: 200, height: 100 };
        assert_eq!(tablet_at(top_left, (200, 100), (0.0, 0.0)), (0, 0));
        let bottom_right = Crop { x: 1720, y: 980, width: 200, height: 100 };
        let placed = picture_place(bottom_right, FRAME, (200, 100));
        assert_eq!((placed.x + placed.width as f32, placed.y + placed.height as f32), (200.0, 100.0));
        assert_eq!(tablet_at(bottom_right, (200, 100), (200.0, 100.0)), (TABLET_RANGE, TABLET_RANGE));
    }

    #[test]
    fn a_window_of_another_size_stretches_the_rectangle_to_fill_it() {
        // Twice the rectangle's size, as on a screen at a scale of 2 if the window were in pixels.
        let crop = Crop { x: 700, y: 300, width: 320, height: 500 };
        let placed = picture_place(crop, FRAME, (640, 1000));
        assert_eq!(placed, Placement { x: -1400.0, y: -600.0, width: 3840, height: 2160 });
        assert_eq!(
            tablet_at(crop, (640, 1000), (200.0, 500.0)),
            (tablet_of(800.0, 1920), tablet_of(550.0, 1080))
        );
        // A share that isn't whole: within a pixel of the guest's, which is all the tablet can say.
        let wide = picture_place(crop, FRAME, (464, 725));
        assert_eq!((wide.width, wide.height), (2784, 1566));
        let (x, y) = tablet_at(crop, (464, 725), (232.0, 362.5));
        assert!((x - tablet_of(860.0, 1920)).abs() <= tablet_of(1.0, 1920), "{x}");
        assert!((y - tablet_of(550.0, 1080)).abs() <= tablet_of(1.0, 1080), "{y}");
    }
}
