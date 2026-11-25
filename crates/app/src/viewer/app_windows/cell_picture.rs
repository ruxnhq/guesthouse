//! A cell's part of the guest's frame, drawn at a ratio of guest pixels to logical units that follows
//! the scale the compositor gives the surface it is on.
//!
//! A widget's size is in whole logical units, which at a scale of 1.3 is never a whole number of the
//! monitor's pixels for most cells. So the picture is not a child widget at its own size: it is drawn
//! here at `pixels / ratio` logical units, a fraction, from a corner that is on a pixel of the monitor.
//! With the ratio at the surface's scale that is each guest pixel on one pixel of the monitor. The widget is the next whole
//! logical size up, and the strip it has over (under one logical unit) is black.

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

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CellPicture {
        pub texture: RefCell<Option<gdk::Texture>>,
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
        /// Lab only: what the last drawing went by, said once each time it changes.
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
                widget.queue_draw();
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

        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let ratio = self.obj().ratio();
            self.measured_at.set(ratio);
            let (width, height) = self.cell.get();
            let pixels = if orientation == gtk::Orientation::Horizontal { width } else { height };
            let length = logical_length(pixels, ratio);
            (length, length, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (widget.width() as f32, widget.height() as f32);
            snapshot.append_color(&gdk::RGBA::BLACK, &graphene::Rect::new(0.0, 0.0, width, height));
            let (Some(texture), Some(source)) = (self.texture.borrow().clone(), self.source.get()) else {
                return;
            };
            let scale = widget.surface_scale();
            let ratio = widget.ratio();
            // GTK draws at exactly the surface's scale; the buffer is the logical size times it,
            // rounded, so what is drawn past the last whole pixel is cut. Measured: correcting for the
            // rounding made every pixel a mix, drawing at the scale made them exact.
            let per = 1.0 / ratio;
            // The corner the picture starts at, moved onto a pixel of the buffer.
            let (origin_x, origin_y) = widget.surface_origin();
            let on_pixel = |origin: f64, at: f64| ((origin + at) * scale).round() / scale - origin;
            let offset = self.offset.get();
            let left = on_pixel(origin_x, f64::from(offset.x) * per);
            let top = on_pixel(origin_y, f64::from(offset.y) * per);
            let shown = graphene::Rect::new(
                left as f32,
                top as f32,
                (f64::from(source.w) * per) as f32,
                (f64::from(source.h) * per) as f32,
            );
            let whole = graphene::Rect::new(
                (left - f64::from(source.x) * per) as f32,
                (top - f64::from(source.y) * per) as f32,
                (f64::from(texture.width()) * per) as f32,
                (f64::from(texture.height()) * per) as f32,
            );
            if std::env::var_os("GUESTHOUSE_APP_WINDOWS_TRACE").is_some() {
                let said = format!(
                    "{}x{} scale {scale} ratio {ratio} shown {shown:?} whole {whole:?}",
                    widget.width(),
                    widget.height()
                );
                if self.traced.replace(said.clone()) != said {
                    eprintln!("app windows: cell picture {said}");
                }
            }
            snapshot.push_clip(&shown);
            // A plain texture node: drawn one texel on one pixel it is exact. A texture-scale node with
            // the nearest filter was not: measured at 2.0 in a nested KWin, every pixel came out mixed.
            snapshot.append_texture(&texture, &whole);
            snapshot.pop();
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
    fn surface_origin(&self) -> (f64, f64) {
        let Some(native) = self.native() else { return (0.0, 0.0) };
        let corner = self.compute_point(&native, &graphene::Point::zero()).unwrap_or(graphene::Point::zero());
        let (shift_x, shift_y) = native.surface_transform();
        (f64::from(corner.x()) + shift_x, f64::from(corner.y()) + shift_y)
    }

    pub fn set_texture(&self, texture: &gdk::Texture) {
        self.imp().texture.replace(Some(texture.clone()));
        self.queue_draw();
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
        self.queue_draw();
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
        self.queue_draw();
    }

    /// Called with the surface's new scale whenever the compositor changes it.
    pub fn connect_scale(&self, said: impl Fn(f64) + 'static) {
        self.imp().on_scale.replace(Some(Box::new(said)));
    }
}
