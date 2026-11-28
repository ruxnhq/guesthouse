//! Where things are, in numbers: a point in a host surface as a point on the guest's screen, the part
//! of a cell that is on the screen, where a popup goes against its parent, where a drag moves the
//! host window, and how large a cell is shown at the scale of the monitor it is on. Pure, so it is
//! tested without a window.
//!
//! Scale. Every rectangle in a layout is in the guest's own pixels, the ones its frame is made of,
//! whatever percentage Windows scales its text by. A host surface is in the toolkit's logical units,
//! and the compositor tells each surface its own scale, which the viewer reads at run time and again
//! when it changes. A cell is shown at [`shown_ratio`] guest pixels for each logical unit: the
//! surface's own scale when Windows is at the step that scale asks for, so that each guest pixel is
//! one pixel of the monitor, and Windows' own scale otherwise, so that the window has the size it
//! means and the compositor scales it. [`Surface::shown`] is the cell at that ratio, and every
//! function that takes a point or hands one back goes through it.

use super::message::{Cell, Point, Rect, Size};

/// The percentages Windows offers for a monitor, from the smallest. Which of them a monitor gets is
/// up to its resolution: the helper reports the ones it offers now.
pub const WINDOWS_STEPS: [u32; 12] = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

/// The step of `offered` nearest to a monitor's scale, the smaller of two as near. `None` when none is
/// offered.
pub fn nearest_step(scale: f64, offered: &[u32]) -> Option<u32> {
    let wanted = scale * 100.0;
    offered.iter().copied().min_by(|a, b| {
        let (da, db) = ((f64::from(*a) - wanted).abs(), (f64::from(*b) - wanted).abs());
        da.total_cmp(&db).then(a.cmp(b))
    })
}

/// How a cell is shown on a monitor whose scale Windows is not at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// One guest pixel on one pixel of the monitor whenever Windows is at the step nearest the
    /// monitor's scale, though the step is not the scale (125 % on a monitor at 1.3): sharp, and as
    /// much smaller or larger as the step is off. On a monitor that asks for another step, the
    /// window keeps the size Windows means and the compositor scales it.
    Sharp,
    /// One guest pixel on one pixel of the monitor only when Windows is at exactly the monitor's
    /// scale. Otherwise the window keeps the size Windows means and the compositor scales it.
    Size,
}

/// Guest pixels for each logical unit of a surface whose scale is `surface_scale`, with Windows at
/// `guest_percent`, out of the steps it offers now.
pub fn shown_ratio(surface_scale: f64, guest_percent: u32, offered: &[u32], fit: Fit) -> f64 {
    let guest = f64::from(guest_percent.max(1)) / 100.0;
    let surface_scale = if surface_scale > 0.0 { surface_scale } else { 1.0 };
    let exact = (surface_scale - guest).abs() < 1.0 / 240.0;
    let served = match fit {
        Fit::Sharp => exact || nearest_step(surface_scale, offered).is_some_and(|step| step == guest_percent),
        Fit::Size => exact,
    };
    if served { surface_scale } else { guest }
}

/// The logical length a surface needs to show `pixels` guest pixels at `ratio`: whole, and never
/// short, so the last pixel is not cut. What it has over is under one logical unit.
pub fn logical_length(pixels: i32, ratio: f64) -> i32 {
    let exact = f64::from(pixels.max(0)) / ratio;
    // A float a hair over a whole number is that number.
    (exact - 1e-6).ceil().max(1.0) as i32
}

/// The size of a host surface, in the units its pointer events come in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub w: f64,
    pub h: f64,
}

impl Surface {
    /// The cell shown at `ratio` guest pixels for each logical unit (see [`shown_ratio`]).
    pub fn shown(cell: &Cell, ratio: f64) -> Self {
        let ratio = if ratio > 0.0 { ratio } else { 1.0 };
        Self { w: f64::from(cell.cell.w) / ratio, h: f64::from(cell.cell.h) / ratio }
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
/// `rel` is the place, at the parent's ratio. The size is the popup's own cell at that ratio, as
/// [`logical_length`] makes it. A place is in whole logical units, so at a fractional ratio it can be
/// off by under one.
pub fn popup_place(popup: &Cell, parent: &Cell, parent_surface: Surface) -> Rect {
    let (per_x, per_y) = parent_surface.per_cell_pixel(parent);
    let scaled = |value: i32, per: f64| (f64::from(value) * per).round() as i32;
    Rect {
        x: scaled(popup.rel.x, per_x),
        y: scaled(popup.rel.y, per_y),
        w: logical_length(popup.cell.w, 1.0 / per_x),
        h: logical_length(popup.cell.h, 1.0 / per_y),
    }
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
