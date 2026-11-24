//! Holds the guest's picture at the frame's shape, like `gtk::AspectFrame`, but placed so that GTK
//! can hand its frames to the compositor.
//!
//! GTK does that only when the picture's corners are on whole logical pixels, which is all Wayland
//! can say, and also on whole pixels of the screen. At a scale of p/q in lowest terms both hold only
//! on multiples of q logical pixels: 2 at 1.5, 4 at 1.25, 20 at 1.45. So the picture's size and place
//! go in steps of q, and at a scale like 1.45 a thin black border is the price.

use super::crop::{Crop, picture_place, rectangle_place};
use gtk::glib;
use gtk::graphene;
use gtk::gsk;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::cell::{Cell, RefCell};

/// Wayland's fractional scales are in 120ths.
const SCALE_STEPS: i64 = 120;
/// How much smaller than the plain fit the picture may be for GTK to hand it over, as a share of its
/// size, and how far from the frame's shape.
const MOST_LOST: f64 = 0.01;
/// Or this many steps. One side can lose a step, and the other then loses as much again for the shape.
const STEPS_LOST: f64 = 2.0;

/// Where the picture goes, in the frame's own logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub width: i32,
    pub height: i32,
}

/// The fewest logical pixels that are also a whole number of the screen's at `scale`.
pub fn pixel_step(scale: f64) -> i64 {
    let steps = ((scale * SCALE_STEPS as f64).round() as i64).max(1);
    let (mut a, mut b) = (steps, SCALE_STEPS);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    SCALE_STEPS / a
}

/// The largest rectangle close to `ratio` that fits `width` by `height`, centred. It's in steps GTK can
/// hand over at `scale` when that costs little, and otherwise the plain fit that GTK draws itself.
/// `origin` is where the frame starts on its surface, in logical pixels, as the steps count from the
/// surface's corner.
pub fn place(width: i32, height: i32, ratio: f64, scale: f64, origin: (f64, f64)) -> Placement {
    let plain = plain_fit(width, height, ratio);
    let Some(stepped) = in_steps(width, height, ratio, scale, origin) else { return plain };
    let lost = |whole: i32, kept: i32| f64::from(whole - kept);
    let allowed = |whole: i32| (STEPS_LOST * pixel_step(scale) as f64).max(f64::from(whole) * MOST_LOST);
    let shape = f64::from(stepped.width) / f64::from(stepped.height);
    let cheap = lost(plain.width, stepped.width) <= allowed(plain.width)
        && lost(plain.height, stepped.height) <= allowed(plain.height)
        && (shape / ratio - 1.0).abs() <= MOST_LOST;
    if cheap { stepped } else { plain }
}

/// What `gtk::AspectFrame` would do: the largest of `ratio` that fits, centred.
fn plain_fit(width: i32, height: i32, ratio: f64) -> Placement {
    let (room_x, room_y) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let (fit_x, fit_y) = if room_x / room_y > ratio {
        ((room_y * ratio).round().max(1.0), room_y)
    } else {
        (room_x, (room_x / ratio).round().max(1.0))
    };
    Placement {
        x: ((room_x - fit_x) / 2.0).floor() as f32,
        y: ((room_y - fit_y) / 2.0).floor() as f32,
        width: fit_x as i32,
        height: fit_y as i32,
    }
}

/// The largest fit in whole steps that GTK's own arithmetic accepts, if there's one. The shape is off by
/// up to half a step, and at an awkward scale the nearest size GTK takes can be several steps short.
fn in_steps(width: i32, height: i32, ratio: f64, scale: f64, origin: (f64, f64)) -> Option<Placement> {
    let step = pixel_step(scale) as f64;
    // The whole steps inside the frame. Its own edges can fall between two.
    let first_x = (origin.0 / step).ceil();
    let first_y = (origin.1 / step).ceil();
    let room_x = (((origin.0 + f64::from(width)) / step).floor() - first_x).max(1.0);
    let room_y = (((origin.1 + f64::from(height)) / step).floor() - first_y).max(1.0);
    let (steps_x, _) = if room_x / room_y > ratio {
        ((room_y * ratio).round().clamp(1.0, room_x), room_y)
    } else {
        (room_x, (room_x / ratio).round().clamp(1.0, room_y))
    };
    let takes = |steps: f64| gtk_takes(steps * step, scale);
    let steps_x = nearest(steps_x, 1.0, steps_x, takes)?;
    let steps_y = nearest((steps_x / ratio).round().clamp(1.0, room_y), 1.0, room_y, takes)?;
    let centred_x = first_x + ((room_x - steps_x) / 2.0).floor();
    let centred_y = first_y + ((room_y - steps_y) / 2.0).floor();
    let left = nearest(centred_x, first_x, first_x + room_x - steps_x, takes)? * step;
    let top = nearest(centred_y, first_y, first_y + room_y - steps_y, takes)? * step;
    Some(Placement {
        x: (left - origin.0) as f32,
        y: (top - origin.1) as f32,
        width: (steps_x * step) as i32,
        height: (steps_y * step) as i32,
    })
}

/// Whether GTK finds `logical` pixels to be whole on the screen. It works in floats, and a scale like
/// 1.45 is only roughly a float, so some sizes that are whole on paper come out a hair over and are
/// refused (`scaled_rect_is_integral` in GTK's Wayland subsurfaces).
fn gtk_takes(logical: f64, scale: f64) -> bool {
    let device = logical as f32 * scale as f32;
    device == device.trunc()
}

/// The closest number of steps to `wanted`, from `least` to `most`, that `takes` accepts, preferring
/// the smaller of two as close.
fn nearest(wanted: f64, least: f64, most: f64, takes: impl Fn(f64) -> bool) -> Option<f64> {
    let farthest = (wanted - least).max(most - wanted).max(0.0) as i64;
    (0..=farthest)
        .flat_map(|away| [wanted - away as f64, wanted + away as f64])
        .find(|steps| (least..=most).contains(steps) && takes(*steps))
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ScreenFrame {
        pub child: glib::WeakRef<gtk::Widget>,
        pub ratio: Cell<f64>,
        /// The scale and the place on the surface the last layout went by.
        pub laid_out_at: Cell<(f64, (f64, f64))>,
        /// The window's clock, and the check on it while the frame is realized.
        pub watching: RefCell<Option<(gtk::gdk::FrameClock, glib::SignalHandlerId)>>,
        /// Where the picture is in view, for the bars beside it. Without a rectangle, that's where it
        /// was put.
        pub picture_at: Cell<Placement>,
        /// The picture reaches past where it's in view, which is all that is drawn of it.
        pub clipped: Cell<bool>,
        /// The guest's frame's size, once one has come.
        pub frame: Cell<Option<(i32, i32)>>,
        /// Lab only: the rectangle of the frame that is all there is to see.
        pub crop: Cell<Option<Crop>>,
    }

    impl ScreenFrame {
        /// The rectangle as it is on the frame shown now. Until a frame comes it's as it was asked for.
        pub fn shown_crop(&self) -> Option<Crop> {
            let crop = self.crop.get()?;
            match self.frame.get() {
                Some(frame) => crop.within(frame),
                None => Some(crop),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ScreenFrame {
        const NAME: &'static str = "GuesthouseScreenFrame";
        type Type = super::ScreenFrame;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ScreenFrame {
        fn dispose(&self) {
            if let Some(child) = self.child.upgrade() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for ScreenFrame {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            // A rectangle is shown at its own size, and the window is made to fit it.
            if let Some(crop) = self.shown_crop() {
                let side = if orientation == gtk::Orientation::Horizontal { crop.width } else { crop.height };
                return (side, side, -1, -1);
            }
            // It shrinks to anything, as the picture inside it can.
            match self.child.upgrade() {
                Some(child) => {
                    let (_, natural, _, _) = child.measure(orientation, for_size);
                    (0, natural, -1, -1)
                }
                None => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some(child) = self.child.upgrade() else { return };
            let widget = self.obj();
            let (scale, origin) = surface_place(widget.upcast_ref());
            self.laid_out_at.set((scale, origin));
            let cropped = self.shown_crop().zip(self.frame.get());
            let (placed, in_view) = match cropped {
                Some((crop, frame)) => {
                    (picture_place(crop, frame, (width, height)), rectangle_place(crop, (width, height)))
                }
                None => {
                    let placed = place(width, height, self.ratio.get(), scale, origin);
                    (placed, placed)
                }
            };
            let transform = gsk::Transform::new().translate(&graphene::Point::new(placed.x, placed.y));
            child.allocate(placed.width, placed.height, -1, Some(transform));
            self.picture_at.set(in_view);
            self.clipped.set(cropped.is_some());
        }

        /// The bars beside a picture of another shape are black, and only the bars: the picture's
        /// own place is left alone, so a surface under the window can show through it.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (widget.width() as f32, widget.height() as f32);
            let at = self.picture_at.get();
            let (left, top) = (at.x, at.y);
            let (right, bottom) = (at.x + at.width as f32, at.y + at.height as f32);
            // Something that spans the whole frame, though it shows nothing. GTK clears only as far
            // as what a window draws reaches. A window has its shadow and border round everything,
            // but in fullscreen or maximised the see-through window has neither, and bars left
            // from the picture's last shape stayed on screen outside what was still drawn.
            let all = graphene::Rect::new(0.0, 0.0, width, height);
            snapshot.append_node(gsk::ColorNode::new(&gtk::gdk::RGBA::TRANSPARENT, &all));
            let black = gtk::gdk::RGBA::BLACK;
            for (x, y, w, h) in [
                (0.0, 0.0, width, top),
                (0.0, bottom, width, height - bottom),
                (0.0, top, left, bottom - top),
                (right, top, width - right, bottom - top),
            ] {
                if w > 0.0 && h > 0.0 {
                    snapshot.append_color(&black, &graphene::Rect::new(x, y, w, h));
                }
            }
            let Some(child) = self.child.upgrade() else { return };
            if self.clipped.get() {
                snapshot.push_clip(&graphene::Rect::new(left, top, right - left, bottom - top));
                widget.snapshot_child(&child, snapshot);
                snapshot.pop();
            } else {
                widget.snapshot_child(&child, snapshot);
            }
        }
    }
}

/// The surface's scale, and where `widget` is on that surface in logical pixels. Before the widget is
/// on a surface it's 1 and the corner, and the next layout corrects it.
fn surface_place(widget: &gtk::Widget) -> (f64, (f64, f64)) {
    let Some(native) = widget.native() else { return (1.0, (0.0, 0.0)) };
    let scale = native.surface().map_or(1.0, |surface| surface.scale());
    let corner = widget.compute_point(&native, &graphene::Point::zero()).unwrap_or(graphene::Point::zero());
    let (shift_x, shift_y) = native.surface_transform();
    (scale, (f64::from(corner.x()) + shift_x, f64::from(corner.y()) + shift_y))
}

glib::wrapper! {
    pub struct ScreenFrame(ObjectSubclass<imp::ScreenFrame>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ScreenFrame {
    pub fn new(child: &impl IsA<gtk::Widget>, ratio: f64) -> Self {
        let frame: Self = glib::Object::new();
        child.set_parent(&frame);
        frame.imp().child.set(Some(child.upcast_ref()));
        frame.imp().ratio.set(ratio);
        // A layout goes by where the frame is on the surface, and that can change with no new layout:
        // the page slides in after the first one, and a move to a screen of another scale moves the
        // pixels. Each drawn frame checks, which costs nothing when the screen is still.
        frame.connect_realize(|frame| {
            let Some(clock) = frame.frame_clock() else { return };
            let weak = frame.downgrade();
            let handler = clock.connect_after_paint(move |_| {
                let Some(frame) = weak.upgrade() else { return };
                if surface_place(frame.upcast_ref()) != frame.imp().laid_out_at.get() {
                    frame.queue_allocate();
                }
            });
            frame.imp().watching.replace(Some((clock, handler)));
        });
        frame.connect_unrealize(|frame| {
            if let Some((clock, handler)) = frame.imp().watching.take() {
                clock.disconnect(handler);
            }
        });
        frame
    }

    /// The size of the guest's frame now shown.
    pub fn set_frame(&self, width: i32, height: i32) {
        let imp = self.imp();
        let size = (width, height);
        if imp.frame.replace(Some(size)) == Some(size) {
            return;
        }
        imp.ratio.set(f64::from(width) / f64::from(height.max(1)));
        // A rectangle partly off a frame of the new size takes another size in the window.
        if imp.crop.get().is_some() {
            self.queue_resize();
        } else {
            self.queue_allocate();
        }
    }

    /// Lab only: shows just this rectangle of the guest's frame, or the whole frame again with `None`.
    /// What lies outside it is clipped away, drawing and the pointer both.
    pub fn set_crop(&self, crop: Option<Crop>) {
        if self.imp().crop.replace(crop) != crop {
            self.set_overflow(if crop.is_some() { gtk::Overflow::Hidden } else { gtk::Overflow::Visible });
            self.queue_resize();
        }
    }

    /// The rectangle as it was asked for.
    pub fn crop(&self) -> Option<Crop> {
        self.imp().crop.get()
    }

    /// Where the picture is in view, in this widget's logical pixels. With a rectangle that is
    /// narrower than the room, the picture reaches on under the bars beside it.
    pub fn in_view(&self) -> graphene::Rect {
        let at = self.imp().picture_at.get();
        graphene::Rect::new(at.x, at.y, at.width as f32, at.height as f32)
    }

    /// The rectangle in view, as it is on the frame now shown.
    pub fn shown_crop(&self) -> Option<Crop> {
        self.imp().shown_crop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIXTEEN_NINE: f64 = 16.0 / 9.0;

    /// The corners on the surface, in logical pixels.
    fn corners(placed: Placement, origin: (f64, f64)) -> [f64; 4] {
        let left = f64::from(placed.x) + origin.0;
        let top = f64::from(placed.y) + origin.1;
        [left, top, left + f64::from(placed.width), top + f64::from(placed.height)]
    }

    #[test]
    fn the_step_is_the_scales_denominator() {
        assert_eq!([1.0, 2.0, 1.5, 1.25, 1.45, 1.75].map(pixel_step), [1, 1, 2, 4, 20, 4]);
    }

    #[test]
    fn corners_are_whole_on_the_surface_and_on_the_screen_when_that_is_cheap() {
        for (scale, origin) in
            [(1.5, (58.0, 71.0)), (1.25, (3.0, 7.0)), (2.0, (1.0, 1.0)), (1.45, (40.0, 80.0))]
        {
            for (width, height) in [(1915, 1077), (1121, 630), (800, 801)] {
                let placed = place(width, height, SIXTEEN_NINE, scale, origin);
                let found = corners(placed, origin);
                let [left, top, right, bottom] = found;
                assert!(left >= origin.0 && top >= origin.1, "{scale} {found:?}");
                assert!(right <= origin.0 + f64::from(width) && bottom <= origin.1 + f64::from(height));
                assert!(
                    ((right - left) / (bottom - top) / SIXTEEN_NINE - 1.0).abs() < 0.01,
                    "{scale} {found:?}"
                );
                if placed == plain_fit(width, height, SIXTEEN_NINE) {
                    continue;
                }
                // GTK checks the corner and the size, each in its own floats.
                let checked = [left, top, f64::from(placed.width), f64::from(placed.height)];
                assert!(checked.iter().all(|value| gtk_takes(*value, scale)), "{scale} {found:?}");
            }
        }
    }

    #[test]
    fn at_whole_and_easy_scales_it_is_always_handed_over() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for (width, height) in [(1915, 1077), (1121, 630), (2560, 1440)] {
                let placed = place(width, height, SIXTEEN_NINE, scale, (24.0, 60.0));
                let checked = [placed.x + 24.0, placed.y + 60.0, placed.width as f32, placed.height as f32];
                assert!(
                    checked.iter().all(|value| gtk_takes(f64::from(*value), scale)),
                    "{scale} {placed:?}"
                );
            }
        }
    }

    #[test]
    fn a_whole_screen_of_the_frames_shape_is_covered_at_a_scale_that_allows_it() {
        // 4K at 1.5 is 2560 by 1440.
        let placed = place(2560, 1440, SIXTEEN_NINE, 1.5, (0.0, 0.0));
        assert_eq!(placed, Placement { x: 0.0, y: 0.0, width: 2560, height: 1440 });
    }

    #[test]
    fn a_wide_window_at_145_percent_is_handed_over() {
        // The owner's window: the frame is 1981 by 1077 at (25, 71) on the surface.
        let origin = (25.0, 71.0);
        let placed = place(1981, 1077, SIXTEEN_NINE, 1.45, origin);
        assert_ne!(placed, plain_fit(1981, 1077, SIXTEEN_NINE));
        let checked = [f64::from(placed.x) + origin.0, f64::from(placed.y) + origin.1];
        let sizes = [f64::from(placed.width), f64::from(placed.height)];
        assert!(checked.iter().chain(&sizes).all(|value| gtk_takes(*value, 1.45)), "{placed:?}");
    }

    #[test]
    fn at_145_percent_fullscreen_stays_whole_rather_than_shrink() {
        // 4K at 1.45 is 2649 by 1490. The nearest GTK takes is 2560 by 1440, 3% smaller: not worth it.
        let placed = place(2649, 1490, SIXTEEN_NINE, 1.45, (0.0, 0.0));
        assert_eq!(placed, plain_fit(2649, 1490, SIXTEEN_NINE));
    }
}
