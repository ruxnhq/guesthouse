//! The screen's side of its own surface: when to use it, where it goes, and handing frames to it.
//!
//! It's what a machine's Direct frames setting turns on, wherever it can be made: on Wayland, with
//! a compositor that has subsurfaces, viewports and dmabufs. `GUESTHOUSE_OWN_SURFACE=0` keeps it
//! off whatever the setting, to tell a fault of its own from one elsewhere; Direct frames then
//! means GTK's offload, as it does where the surface can't be made.
//!
//! While it's on, GTK's own offload is off, so only one subsurface ever shows the picture. It sits
//! under the window's surface, and while it has a frame up the window is clear over the screen, so
//! it shows through and what GTK draws over the screen stays on top. Until then, and whenever GTK
//! shows a frame instead, the window is solid, so nothing behind the window shows through.
//!
//! What it does and why it falls back goes in the app's log, a line for each change and none for
//! each frame, so a report from a desktop or card nobody here has can say what happened.

use super::dmabuf_feedback::fourcc_name;
use super::frames::{FrameRoute, SlotFrame};
use super::import::texture;
use super::own_surface::{NotShown, OwnSurface, Stall};
use super::screen::Screen;
use super::slot_hold::HeldFrame;
use adw::prelude::*;
use gtk::{gdk, graphene};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const OWN_SURFACE_VARIABLE: &str = "GUESTHOUSE_OWN_SURFACE";
const OWN_SURFACE_OFF: &str = "0";
/// The window and the screen's background are clear with this.
const SEE_THROUGH: &str = "see-through";
const SEE_THROUGH_STYLE: &str = "window.see-through, .see-through .guest-screen { background: transparent; }";
/// Above the user's own stylesheet. A theme in ~/.config/gtk-4.0/gtk.css that gives every window
/// a background would otherwise win over the app's style, and paint the window over the picture.
const SEE_THROUGH_PRIORITY: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 1;
/// A picture this close to filling the frame on every side fills it, so that a fullscreen picture
/// covers the whole screen and the compositor can put it straight on. The shape is off by less than
/// a pixel then, which nobody sees.
const FILL_WITHIN: f32 = 2.0;

/// The compositor turned down the own surface's HDR frames in this run. One answer for the whole
/// app, as the screen's HDR answer is.
static HDR_REFUSED: AtomicBool = AtomicBool::new(false);

fn own_surface_allowed() -> bool {
    std::env::var(OWN_SURFACE_VARIABLE).map_or(true, |value| value != OWN_SURFACE_OFF)
}

/// Whether the own surface can pass HDR frames on, as far as this run has seen.
pub fn own_surface_takes_hdr() -> bool {
    own_surface_allowed() && !HDR_REFUSED.load(Ordering::Relaxed)
}

/// Who shows the frames, for the log to say when it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShownBy {
    OwnSurface,
    Gtk(NotShown),
}

pub(super) fn add_see_through_style(display: &gdk::Display) {
    let style = gtk::CssProvider::new();
    style.load_from_string(SEE_THROUGH_STYLE);
    gtk::style_context_add_provider_for_display(display, &style, SEE_THROUGH_PRIORITY);
}

impl Screen {
    fn note(&self, text: &str) {
        (self.0.log)(text);
    }

    fn pass_on_notes(&self, own: &mut OwnSurface) {
        for note in own.take_notes() {
            self.note(&note);
        }
    }

    /// Whether the own surface is to show the frames, now or once it's there.
    ///
    /// Only the daemon's frames in video memory are for it. Frames in memory, QEMU's display and a
    /// Linux guest's forwarded frames stay with GTK.
    pub(super) fn own_awaited(&self) -> bool {
        let for_it = self.0.route.get() == FrameRoute::FrameSocket && !self.0.memory_frames.get();
        for_it && self.0.direct_frames.get() && !self.0.own_impossible.get() && own_surface_allowed()
    }

    /// Whether the producer may lay the frames out in the card's tiles. They're for the own surface,
    /// on a compositor that can't take the card's 10-bit rows, so Direct frames off is also the way
    /// out of a picture the tiles scramble.
    pub(super) fn wants_tiles(&self) -> bool {
        self.own_awaited() && !self.0.tiles_failed.get()
    }

    /// Asks for the frames again when the connection there is offered something else than is
    /// wanted now. The producer then makes this viewer a new ring, in tiles or in rows.
    pub(super) fn follow_tiles(&self) {
        let tiles = self.wants_tiles();
        let connected = self.0.gpu.borrow().is_some() && self.0.route.get() == FrameRoute::FrameSocket;
        if !connected || self.0.tiles_offered.get() == tiles {
            return;
        }
        self.0.handover.borrow_mut().take();
        self.0.gpu.borrow_mut().take();
        self.follow_gpu();
    }

    /// Brings the own surface and GTK's offload in line with the setting. This is called often, and
    /// changes nothing that's already right.
    pub(super) fn follow_direct_frames(&self) {
        let awaited = self.own_awaited();
        let offload = self.0.direct_frames.get() && !awaited;
        let mode = if offload {
            gtk::GraphicsOffloadEnabled::Enabled
        } else {
            gtk::GraphicsOffloadEnabled::Disabled
        };
        self.0.offload.set_enabled(mode);
        // Its black background is drawn even while it's off, and would cover the surface below.
        // Without the own surface, the black bars beside a frame of another shape would otherwise
        // be GTK's to draw, and the compositor couldn't put the frame straight on the display.
        #[cfg(feature = "hdr")]
        self.0.offload.set_black_background(!awaited);
        if awaited {
            self.start_own_surface();
        } else {
            self.stop_own_surface();
        }
    }

    fn start_own_surface(&self) {
        if !self.0.page_settled.get() || self.0.own.borrow().is_some() {
            return;
        }
        let root = &self.0.root;
        let Some(surface) = root.native().and_then(|native| native.surface()) else { return };
        let own = OwnSurface::new(&WidgetExt::display(root), &surface);
        let Some(mut own) = own else {
            self.note("own surface: it can't be made on this display, so Direct frames is GTK's offload");
            self.0.own_impossible.set(true);
            self.follow_direct_frames();
            return;
        };
        self.pass_on_notes(&mut own);
        *self.0.own.borrow_mut() = Some(own);
    }

    /// The page has stopped moving, so the own surface can go where the picture is. It's placed on
    /// the window and not in the page: while the page slides, it would trail behind the picture and
    /// stick out past the window's edge. GTK draws the frames until now, and they move with the page.
    pub fn page_shown(&self) {
        self.0.page_settled.set(true);
        self.follow_direct_frames();
    }

    /// The page is on its way out, and GTK draws the frames again, for the same reason. It may
    /// come back, as after a swipe that was let go half way.
    pub fn page_leaving(&self) {
        self.0.page_settled.set(false);
        self.stop_own_surface();
    }

    /// Takes the own surface away. GTK shows the frame it had, so the picture stays, and keeps it
    /// to hand back if the surface returns.
    pub(super) fn stop_own_surface(&self) {
        self.see_through(false);
        let Some(mut own) = self.0.own.borrow_mut().take() else { return };
        self.pass_on_notes(&mut own);
        self.note(&format!("own surface: stopped, after {:?}", own.shown()));
        drop(own);
        self.0.shown_by.set(None);
        let shown = self.0.on_own_surface.borrow_mut().take();
        if let Some(frame) = shown {
            self.show_with_gtk(frame, true);
        }
    }

    /// Clears the window over the screen, or makes it solid again.
    fn see_through(&self, clear: bool) {
        let Some(window) = self.0.root.root() else { return };
        if window.has_css_class(SEE_THROUGH) != clear {
            if clear {
                window.add_css_class(SEE_THROUGH);
            } else {
                window.remove_css_class(SEE_THROUGH);
            }
        }
    }

    /// Says in the log who shows the frames, when that changes.
    fn shown_by(&self, by: ShownBy) {
        if self.0.shown_by.replace(Some(by)) == Some(by) {
            return;
        }
        match by {
            ShownBy::OwnSurface => self.note("own surface: it shows the frames"),
            ShownBy::Gtk(why) => self.note(&format!("own surface: GTK shows the frames, as {}", why.why())),
        }
    }

    /// Shows the newest GPU frame on the app's own surface, and says if there was one to show. A
    /// frame the surface can't take is shown by GTK instead, and so is one that comes before the
    /// surface is there.
    pub(super) fn show_on_own_surface(&self) -> bool {
        let mut own = self.0.own.borrow_mut();
        if own.is_none() && !self.own_awaited() {
            return false;
        }
        if let Some(own) = own.as_mut() {
            own.catch_up();
            self.place_own_surface(own);
            self.watch_own_surface(own);
        }
        let mut gpu = self.0.gpu.borrow_mut();
        let Some(source) = gpu.as_mut() else { return false };
        let newest = source.newest_slot();
        let sdr_white_nits = source.sdr_white_nits();
        drop(gpu);
        let Some(frame) = newest else {
            if let Some(own) = own.as_mut() {
                self.hand_over(own, sdr_white_nits);
                self.pass_on_notes(own);
            }
            return false;
        };
        // A newer frame takes the place of one still waiting to move over.
        self.0.handover.borrow_mut().take();
        let (width, height) = (frame.descriptor.width, frame.descriptor.height);
        self.0.aspect.set_ratio(f64::from(width) / f64::from(height));
        self.0.spinner.set_visible(false);
        self.0.booting.set(false);
        self.0.qemu.borrow_mut().take();
        let frame = HeldFrame::new(frame);
        let Some(own) = own.as_mut() else {
            self.show_with_gtk(frame, true);
            return true;
        };
        let showing = self.0.on_own_surface.borrow().is_some();
        match own.show(frame.for_reader(), sdr_white_nits) {
            Ok(()) => self.on_own_surface(frame),
            // The surface's last frame stays up for the moment the compositor needs, as when a
            // slot is used for the first time. Going to GTK and back would flicker.
            Err(not_shown) if showing && not_shown.may_later() => {
                *self.0.handover.borrow_mut() = Some(frame);
            }
            Err(not_shown) => self.leave_to_gtk(own, frame, not_shown),
        }
        self.pass_on_notes(own);
        true
    }

    /// Says in the log when the frames sent stop reaching the screen, and when they do again.
    fn watch_own_surface(&self, own: &mut OwnSurface) {
        let window = self.0.root.root().and_downcast::<gtk::Window>();
        // A window that is minimised, covered or on another desktop is shown no frames.
        let seen = window.as_ref().is_none_or(|window| !window.is_suspended());
        match own.watch(Instant::now(), seen) {
            Some(Stall::Began { waited, sent }) => {
                let shown = own.shown();
                let (active, fullscreen) = window
                    .as_ref()
                    .map_or((false, false), |window| (window.is_active(), window.is_fullscreen()));
                self.note(&format!(
                    "own surface: no frame has reached the screen for {} ms, with {sent} sent since \
                     ({} shown and {} dropped so far; window active {active}, fullscreen {fullscreen})",
                    waited.as_millis(),
                    shown.presented,
                    shown.discarded,
                ));
            }
            Some(Stall::Over { after }) => self.note(&format!(
                "own surface: frames reach the screen again, {} ms after that was seen",
                after.as_millis()
            )),
            None => {}
        }
    }

    /// The own surface isn't showing this frame, so it comes off the screen and GTK shows it.
    fn leave_to_gtk(&self, own: &mut OwnSurface, frame: HeldFrame, not_shown: NotShown) {
        own.hide();
        self.see_through(false);
        self.shown_by(ShownBy::Gtk(not_shown));
        // Rows may be what this compositor takes, and GTK can always draw them.
        if not_shown == NotShown::BufferRefused && frame.is_tiled() {
            self.give_up_on_tiles(&frame, "the compositor refused");
        }
        self.note_refused_hdr(&frame, not_shown);
        self.show_with_gtk(frame, not_shown.may_later());
    }

    /// The own surface has the picture now, so GTK lets go of its own and the window clears. The
    /// frame is kept, so GTK can show the same one when the surface has to go, and for a thumbnail.
    /// The compositor has its slot until the next frame anyway.
    fn on_own_surface(&self, frame: HeldFrame) {
        *self.0.on_own_surface.borrow_mut() = Some(frame);
        // GTK's last texture would keep its slot from the ring for as long as the surface shows.
        self.0.latest.borrow_mut().take();
        if self.0.picture.paintable().is_some() {
            self.0.picture.set_paintable(None::<&gdk::Paintable>);
        }
        self.see_through(true);
        self.0.frames.set(self.0.frames.get() + 1);
        self.shown_by(ShownBy::OwnSurface);
    }

    /// The frame on the own surface as a texture, for a thumbnail. It holds the slot only as long
    /// as the texture lives.
    pub(super) fn own_surface_texture(&self) -> Option<gdk::Texture> {
        let shown = self.0.on_own_surface.borrow();
        let SlotFrame { file, descriptor, release, .. } = shown.as_ref()?.for_reader();
        texture(&WidgetExt::display(&self.0.root), file, &descriptor, release)
    }

    /// HDR frames the compositor turns down for good leave HDR to GTK. Where GTK passes it on,
    /// nothing is lost. Where it would flatten it, the screen's answer becomes that the toolkit
    /// can't, and Windows goes back to SDR.
    fn note_refused_hdr(&self, frame: &HeldFrame, not_shown: NotShown) {
        // A refused frame in the card's tiles says nothing yet: the same frame in rows may be taken.
        let refused = match not_shown {
            NotShown::BufferRefused => !frame.is_tiled(),
            NotShown::DescriptionRefused => true,
            _ => false,
        };
        if frame.is_pq() && refused && !HDR_REFUSED.swap(true, Ordering::Relaxed) {
            self.note("own surface: the compositor takes no HDR frames from it, so HDR is GTK's to pass on");
        }
    }

    /// GTK shows a frame the own surface didn't take. One it may take later is kept as well, and
    /// its slot goes back when both have finished with it.
    fn show_with_gtk(&self, frame: HeldFrame, may_later: bool) {
        self.0.on_own_surface.borrow_mut().take();
        let SlotFrame { file, descriptor, release, .. } = frame.for_reader();
        let display = WidgetExt::display(&self.0.root);
        match texture(&display, file, &descriptor, release) {
            Some(picture) => {
                self.0.picture.set_paintable(Some(&picture));
                *self.0.latest.borrow_mut() = Some(picture);
            }
            None if frame.is_tiled() && !may_later => self.give_up_on_tiles(&frame, "GTK couldn't take"),
            None => {}
        }
        *self.0.handover.borrow_mut() = may_later.then_some(frame);
    }

    /// The card's tiles don't work out on this desktop, so the daemon is asked again for rows,
    /// which GTK can always show. A compositor that refuses the tiles may take the rows, and tiles
    /// GTK can't take either would leave the screen black.
    fn give_up_on_tiles(&self, frame: &HeldFrame, why: &str) {
        if self.0.tiles_failed.replace(true) {
            return;
        }
        let format = frame.format();
        self.note(&format!(
            "frames: {why} {} {:#x}, so rows are asked for",
            fourcc_name(format.fourcc),
            format.modifier
        ));
    }

    /// Moves the frame GTK is showing onto the own surface, as soon as it can take it. A still
    /// guest sends no other frame, and GTK's would stay up: GTK can't say where the guest's SDR
    /// white is, so on an HDR screen its picture is brighter than the own surface's, and the change
    /// would come whenever the guest next drew.
    fn hand_over(&self, own: &mut OwnSurface, sdr_white_nits: Option<u32>) {
        let Some(frame) = self.0.handover.borrow_mut().take() else { return };
        match own.show(frame.for_reader(), sdr_white_nits) {
            Ok(()) => self.on_own_surface(frame),
            Err(not_shown) if not_shown.may_later() => *self.0.handover.borrow_mut() = Some(frame),
            Err(not_shown) => self.leave_to_gtk(own, frame, not_shown),
        }
    }

    /// Puts the surface where GTK has the picture. The compositor rounds it to the screen's pixels.
    fn place_own_surface(&self, own: &mut OwnSurface) {
        let picture = &self.0.picture;
        let Some(native) = picture.native() else { return };
        let Some(bounds) = picture.compute_bounds(&native) else { return };
        let frame = self.0.aspect.compute_bounds(&native);
        let fills = |frame: &graphene::Rect| {
            (frame.width() - bounds.width()).abs() <= FILL_WITHIN
                && (frame.height() - bounds.height()).abs() <= FILL_WITHIN
        };
        let bounds = frame.filter(fills).unwrap_or(bounds);
        let (shift_x, shift_y) = native.surface_transform();
        let corner = graphene::Point::new(bounds.x() + shift_x as f32, bounds.y() + shift_y as f32);
        let moved = own.place(
            corner.x().round() as i32,
            corner.y().round() as i32,
            bounds.width().round() as i32,
            bounds.height().round() as i32,
        );
        // A subsurface moves when its parent commits, and GTK commits when it draws.
        if moved {
            picture.queue_draw();
        }
    }
}
