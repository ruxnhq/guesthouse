//! The place of a cell's part of the guest's frame in a host window, at a ratio of guest pixels to
//! logical units that follows the scale the compositor gives the surface it is on.
//!
//! GTK draws nothing of the picture. The widget only holds the cell's size, the next whole logical
//! size up from `pixels / ratio`, and says where the picture goes: the frame's buffer goes on a
//! surface of the view's own over the window (`OwnSurface`), its viewport's source the cell and its
//! destination the widget's size. The compositor does the scaling, so a source of the destination's
//! size in the monitor's pixels is each guest pixel on one pixel of the monitor.

use crate::viewer::crop::Crop;
use gtk::gdk;
use gtk::glib;
use gtk::graphene;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use guesthouse_app_core::app_windows::{Fit, Point, Rect, logical_length, shown_ratio};
use std::cell::{Cell, RefCell};

/// What decides the ratio: Windows' percentage and the steps it offers, and the rule for a monitor
/// whose scale Windows is not at.
#[derive(Debug, Clone, PartialEq)]
pub struct Guest {
    pub percent: u32,
    pub offered: Vec<u32>,
    pub fit: Fit,
}

impl Default for Guest {
    fn default() -> Self {
        Self { percent: 100, offered: vec![100], fit: Fit::Sharp }
    }
}

/// What is called with a surface's new scale.
type ScaleSaid = Box<dyn Fn(f64)>;
/// What is called with the widget's new size, in logical units.
type SizeSaid = Box<dyn Fn(i32, i32)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CellPicture {
        /// The part of the frame shown, in guest pixels.
        pub source: Cell<Option<Rect>>,
        /// Where in the cell that part starts: nothing unless the cell hangs over the screen's edge.
        pub offset: Cell<Point>,
        /// The whole cell's size, in guest pixels, which is what the widget is made to hold.
        pub cell: Cell<(i32, i32)>,
        pub guest: RefCell<Guest>,
        /// The ratio the last measure went by, so a change of it resizes the window.
        pub measured_at: Cell<f64>,
        pub watching: RefCell<Option<(gdk::Surface, glib::SignalHandlerId)>>,
        pub on_scale: RefCell<Option<ScaleSaid>>,
        /// The size the widget was last given, in logical units: the window's, which the compositor
        /// may have chosen (a tiling layout, a maximize) rather than the cell's.
        pub allocated: Cell<Option<(i32, i32)>>,
        pub on_size: RefCell<Option<SizeSaid>>,
        /// Lab only: what the last place went by, said once each time it changes.
        pub traced: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CellPicture {
        const NAME: &'static str = "GuesthouseCellPicture";
        type Type = super::CellPicture;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CellPicture {}

    impl WidgetImpl for CellPicture {
        fn realize(&self) {
            self.parent_realize();
            let widget = self.obj();
            let Some(surface) = widget.native().and_then(|native| native.surface()) else { return };
            let weak = widget.downgrade();
            let handler = surface.connect_scale_notify(move |surface| {
                let Some(widget) = weak.upgrade() else { return };
                widget.queue_resize();
                if let Some(said) = widget.imp().on_scale.borrow().as_ref() {
                    said(surface.scale());
                }
            });
            self.watching.replace(Some((surface, handler)));
        }

        fn unrealize(&self) {
            if let Some((surface, handler)) = self.watching.take() {
                surface.disconnect(handler);
            }
            self.parent_unrealize();
        }

        /// The cell's size is what the widget would like; it takes any size it is given down to a
        /// unit, as a compositor may give a window another size than its cell (a tiling layout, a
        /// maximize, the user's drag). A window that may not be resized keeps its size by GTK's rule.
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let ratio = self.obj().ratio();
            self.measured_at.set(ratio);
            let (width, height) = self.cell.get();
            let pixels = if orientation == gtk::Orientation::Horizontal { width } else { height };
            let length = logical_length(pixels, ratio);
            (1, length, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            if self.allocated.replace(Some((width, height))) != Some((width, height))
                && let Some(said) = self.on_size.borrow().as_ref()
            {
                said(width, height);
            }
        }
    }
}

glib::wrapper! {
    pub struct CellPicture(ObjectSubclass<imp::CellPicture>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for CellPicture {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl CellPicture {
    /// The scale the compositor gave the surface this is on: 1 until it is on one.
    pub fn surface_scale(&self) -> f64 {
        self.native().and_then(|native| native.surface()).map_or(1.0, |surface| surface.scale())
    }

    /// Guest pixels for each logical unit, as the cell is shown now.
    pub fn ratio(&self) -> f64 {
        let guest = self.imp().guest.borrow();
        shown_ratio(self.surface_scale(), guest.percent, &guest.offered, guest.fit)
    }

    /// Where this widget is on its surface, in logical units.
    pub fn surface_origin(&self) -> (f64, f64) {
        let Some(native) = self.native() else { return (0.0, 0.0) };
        let corner = self.compute_point(&native, &graphene::Point::zero()).unwrap_or(graphene::Point::zero());
        let (shift_x, shift_y) = native.surface_transform();
        (f64::from(corner.x()) + shift_x, f64::from(corner.y()) + shift_y)
    }

    /// Where the picture goes on the widget's surface, in its logical units, as X, Y, width and
    /// height, and the rectangle of the frame shown there, in the frame's pixels. The rectangle is as
    /// many of the frame's pixels as the place covers of the monitor's at the surface's scale (the
    /// compositor makes a destination of D logical units round(D * scale) pixels), so at a ratio of the
    /// scale no pixel is stretched. It may reach a pixel or two past the cell; `None` until the cell is known.
    pub fn picture_place(&self) -> Option<((i32, i32, i32, i32), Crop)> {
        let imp = self.imp();
        let source = imp.source.get()?;
        let offset = imp.offset.get();
        let scale = self.surface_scale();
        let ratio = self.ratio();
        let (origin_x, origin_y) = self.surface_origin();
        let (shift_x, shift_y) = self.picture_shift();
        let left = (f64::from(offset.x) / ratio).round() as i32;
        let top = (f64::from(offset.y) / ratio).round() as i32;
        let x = origin_x.round() as i32 + shift_x + left;
        let y = origin_y.round() as i32 + shift_y + top;
        // A window smaller than its cell shows the cell's top left, title bar and all, until the guest
        // has the size: the picture is cut, never squeezed.
        let (room_w, room_h) = imp.allocated.get().unwrap_or((i32::MAX, i32::MAX));
        let width = logical_length(source.w, ratio).min(room_w - shift_x - left).max(1);
        let height = logical_length(source.h, ratio).min(room_h - shift_y - top).max(1);
        let covered = |logical: i32, pixels: i32| {
            if std::env::var_os("GUESTHOUSE_LAB_SOURCE_CELL").is_some() {
                return pixels;
            }
            ((f64::from(logical) * scale).round() * ratio / scale).round() as i32
        };
        let crop = Crop {
            x: source.x,
            y: source.y,
            width: covered(width, source.w),
            height: covered(height, source.h),
        };
        let place = ((x, y, width, height), crop);
        if std::env::var_os("GUESTHOUSE_APP_WINDOWS_TRACE").is_some() {
            let said = format!("scale {scale} ratio {ratio} place {:?} source {crop:?}", place.0);
            if imp.traced.replace(said.clone()) != said {
                eprintln!("app windows: cell picture {said}");
            }
        }
        Some(place)
    }

    /// The cell's size at the ratio now, in logical units: the size the window would like.
    pub fn wanted_size(&self) -> (i32, i32) {
        let ratio = self.ratio();
        let (width, height) = self.imp().cell.get();
        (logical_length(width, ratio), logical_length(height, ratio))
    }

    /// The size the widget has, once it has been given one.
    pub fn allocated(&self) -> Option<(i32, i32)> {
        self.imp().allocated.get()
    }

    /// How far the picture is from the widget's corner, in logical units: half of what the widget has
    /// over the cell on each side, so a window larger than its cell (a guest window that cannot be that
    /// large, or not yet) shows it in the middle with black around.
    pub fn picture_shift(&self) -> (i32, i32) {
        let Some((width, height)) = self.imp().allocated.get() else { return (0, 0) };
        let (wanted_w, wanted_h) = self.wanted_size();
        (((width - wanted_w) / 2).max(0), ((height - wanted_h) / 2).max(0))
    }

    /// The cell's size, the part of the frame shown and where in the cell it starts.
    pub fn set_cell(&self, cell: (i32, i32), source: Rect, offset: Point) {
        let imp = self.imp();
        let resized = imp.cell.replace(cell) != cell;
        imp.source.set(Some(source));
        imp.offset.set(offset);
        if resized {
            self.queue_resize();
        }
    }

    pub fn set_guest(&self, guest: Guest) {
        if self.imp().guest.replace(guest.clone()) != guest {
            self.check_ratio();
        }
    }

    /// Resizes if the ratio is no longer what the last measure went by.
    pub fn check_ratio(&self) {
        if (self.ratio() - self.imp().measured_at.get()).abs() > 1e-9 {
            self.queue_resize();
        }
    }

    /// Called with the widget's new size whenever it is given another.
    pub fn connect_size(&self, said: impl Fn(i32, i32) + 'static) {
        self.imp().on_size.replace(Some(Box::new(said)));
    }

    /// Called with the surface's new scale whenever the compositor changes it.
    pub fn connect_scale(&self, said: impl Fn(f64) + 'static) {
        self.imp().on_scale.replace(Some(Box::new(said)));
    }
}
