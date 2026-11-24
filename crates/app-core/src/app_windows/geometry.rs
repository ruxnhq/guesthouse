//! Where things are, in numbers: a point in a host surface as a point on the guest's screen, the part
//! of a cell that is on the screen, where a popup goes against its parent, and where a drag moves the
//! host window. Pure, so it is tested without a window.
//!
//! What this assumes about scale. Every rectangle in a layout is in the guest's own pixels, the ones
//! its frame is made of, whatever percentage Windows scales its text by: that percentage only says how
//! big things look, and the helper has already laid out in the pixels it means. A host surface is in
//! the units the toolkit reports a pointer in, which on a screen scaled by 1.3 are logical, not device,
//! pixels. The viewer is expected to make each host window as large as its cell in those units, so
//! that a point of the surface is a pixel of the cell, and to leave the scaling to the compositor,
//! which is what the demo did and its pointer was exact. [`Surface`] is for a viewer that shows a cell
//! at any other size: every function that takes a point or hands one back goes through the ratio of
//! the surface to the cell, so nothing here needs to change when one does.

use super::message::{Cell, Point, Rect, Size};

/// The size of a host surface, in the units its pointer events come in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub w: f64,
    pub h: f64,
}

impl Surface {
    /// The surface of a host window made as large as its cell, which is the usual one.
    pub fn of_cell(cell: &Cell) -> Self {
        Self { w: f64::from(cell.cell.w), h: f64::from(cell.cell.h) }
    }

    /// Surface units for each pixel of the cell, on each axis. A cell with no size has a ratio of 1,
    /// so a stray point is left as it is.
    fn per_cell_pixel(self, cell: &Cell) -> (f64, f64) {
        let ratio = |surface: f64, pixels: i32| if pixels > 0 { surface / f64::from(pixels) } else { 1.0 };
        (ratio(self.w, cell.cell.w), ratio(self.h, cell.cell.h))
    }
}

/// The point of the guest's screen that a point of a cell's host surface is, for a pointer event: the
/// frame's origin plus the point. The same rule serves a window and a popup, and for a window the
/// helper has parked under its cell the frame is the cell. It is kept inside the frame, as a pointer
/// at the edge of a host window can be a fraction past it, and then inside the screen, as the
/// tablet has no point beyond it.
pub fn guest_point(cell: &Cell, surface: Surface, x: f64, y: f64, screen: Size) -> Point {
    let (per_x, per_y) = surface.per_cell_pixel(cell);
    let frame = cell.frame;
    let inside = |origin: i32, length: i32, wanted: f64, limit: i32| {
        let mut at = origin.saturating_add(wanted.floor() as i32);
        if length > 0 {
            at = at.clamp(origin, origin.saturating_add(length - 1));
        }
        if limit > 0 {
            at = at.clamp(0, limit - 1);
        }
        at
    };
    Point {
        x: inside(frame.x, frame.w, x / per_x, screen.w),
        y: inside(frame.y, frame.h, y / per_y, screen.h),
    }
}

/// How a cell's picture is cut from the guest's frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    /// What is taken from the frame: the cell where it is on the screen.
    pub source: Rect,
    /// Where in the host window it goes, from the window's own corner. Zero unless the cell hangs over
    /// the screen's top or left edge. The rest of the window has no picture.
    pub offset: Point,
}

/// The part of the cell that is on the screen, or `None` when none is, as for a minimized window,
/// whose cell has no size.
pub fn crop(cell: Rect, screen: Size) -> Option<Crop> {
    let left = cell.x.max(0);
    let top = cell.y.max(0);
    let right = cell.x.saturating_add(cell.w).min(screen.w);
    let bottom = cell.y.saturating_add(cell.h).min(screen.h);
    if right <= left || bottom <= top {
        return None;
    }
    Some(Crop {
        source: Rect { x: left, y: top, w: right - left, h: bottom - top },
        offset: Point { x: left - cell.x, y: top - cell.y },
    })
}

/// Where a popup's host surface goes, in the host surface of the cell it belongs to. `rel` is the
/// popup's frame less its parent's frame, and a host surface starts at its cell's frame origin, so
/// `rel` is the place. The size is the popup's own cell, so a popup keeps its size however the
/// parent is shown.
pub fn popup_place(popup: &Cell, parent: &Cell, parent_surface: Surface) -> Rect {
    let (per_x, per_y) = parent_surface.per_cell_pixel(parent);
    let scaled = |value: i32, per: f64| (f64::from(value) * per).round() as i32;
    Rect { x: scaled(popup.rel.x, per_x), y: scaled(popup.rel.y, per_y), w: popup.cell.w, h: popup.cell.h }
}

/// The strip of a host window that a press-and-drag moves the window by: the top `caption.h` rows,
/// less `buttons_w` at the right, so the buttons still get their clicks. In the cell's own pixels,
/// and `None` when the helper does not know where the title bar is or the buttons leave no room.
pub fn drag_strip(cell: &Cell) -> Option<Rect> {
    let height = cell.caption.h.min(cell.cell.h);
    let width = cell.cell.w.saturating_sub(cell.caption.buttons_w.max(0));
    (height > 0 && width > 0).then_some(Rect { x: 0, y: 0, w: width, h: height })
}

/// Whether a point of the host surface lies on the drag strip.
pub fn on_drag_strip(cell: &Cell, surface: Surface, x: f64, y: f64) -> bool {
    let Some(strip) = drag_strip(cell) else { return false };
    let (per_x, per_y) = surface.per_cell_pixel(cell);
    let (x, y) = (x / per_x, y / per_y);
    (0.0..f64::from(strip.w)).contains(&x) && (0.0..f64::from(strip.h)).contains(&y)
}
