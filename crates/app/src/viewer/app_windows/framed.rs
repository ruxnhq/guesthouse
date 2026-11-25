//! A rectangle of the guest's frame shown in a host window or popup with no copy: the frame's own
//! buffer goes on a surface of this view's own over GTK's (`OwnSurface`), its viewport's source the
//! rectangle and its destination the place `CellPicture` says. GTK draws none of it. Every consumer
//! of a part of the frame (an app window, a popup, the secure desktop's window) shows it this way.

use super::cell_picture::CellPicture;
use super::hub::Hub;
use crate::viewer::own_surface::OwnSurface;
use crate::viewer::slot_hold::HeldFrame;
use gtk::gdk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

/// Lab only: how often what the compositor showed is said.
const SAY_SHOWN: Duration = Duration::from_secs(5);

pub struct Framed {
    /// For the log.
    name: String,
    picture: CellPicture,
    /// The surface the frames go on, over GTK's surface it was made for.
    own: RefCell<Option<(gdk::Surface, OwnSurface)>>,
    /// The newest frame is still to be shown here: the surface wasn't there yet, the compositor
    /// hadn't made its buffer, or the window was out of sight.
    behind: Cell<bool>,
    /// The picture was taken off while the window is out of sight, so it holds no slot.
    hidden: Cell<bool>,
    /// Lab only: when what the compositor showed was last said.
    said_at: Cell<Option<Instant>>,
}

impl Framed {
    pub fn new(name: &str, picture: &CellPicture) -> Self {
        Self {
            name: name.to_owned(),
            picture: picture.clone(),
            own: RefCell::default(),
            behind: Cell::new(true),
            hidden: Cell::new(false),
            said_at: Cell::new(None),
        }
    }

    /// Puts the frame's buffer on the own surface, cut to the rectangle. A window out of sight shows
    /// none and holds no slot: the compositor would keep the buffer of a surface it never draws.
    pub fn show(&self, hub: &Hub, frame: &HeldFrame, sdr_white_nits: Option<u32>, out_of_sight: bool) {
        if out_of_sight {
            self.behind.set(true);
            if !self.hidden.replace(true)
                && let Some((_, own)) = self.own.borrow_mut().as_mut()
            {
                own.hide();
            }
            return;
        }
        self.hidden.set(false);
        let Some(((x, y, width, height), crop)) = self.picture.picture_place() else {
            self.behind.set(true);
            return;
        };
        if !self.own_surface(hub) {
            self.behind.set(true);
            return;
        }
        let mut own = self.own.borrow_mut();
        let Some((_, own)) = own.as_mut() else { return };
        own.place(x, y, width, height);
        own.set_crop(Some(crop));
        match own.show(frame.for_reader(), sdr_white_nits) {
            Ok(()) => self.behind.set(false),
            Err(not_shown) => {
                self.behind.set(not_shown.may_later());
                if !not_shown.may_later() {
                    hub.note(&format!("{}: the frame isn't shown, as {}", self.name, not_shown.why()));
                }
            }
        }
        for note in own.take_notes() {
            hub.trace(&format!("{}: {note}", self.name));
        }
    }

    /// Takes in what the compositor gave back, follows a new place under a still guest, and shows
    /// `newest` if this is behind or back in sight.
    pub fn tick(
        &self,
        hub: &Hub,
        newest: Option<&HeldFrame>,
        sdr_white_nits: Option<u32>,
        out_of_sight: bool,
    ) {
        if let Some((_, own)) = self.own.borrow_mut().as_mut() {
            own.catch_up();
            let now = Instant::now();
            if self.said_at.get().is_none_or(|at| now - at >= SAY_SHOWN) {
                self.said_at.set(Some(now));
                hub.trace(&format!("{}: shown so far {:?}", self.name, own.shown()));
            }
            if !self.hidden.get()
                && let Some(((x, y, width, height), crop)) = self.picture.picture_place()
            {
                own.place(x, y, width, height);
                own.set_crop(Some(crop));
            }
        }
        let back_in_sight = self.hidden.get() && !out_of_sight;
        if (self.behind.get() || back_in_sight)
            && let Some(frame) = newest
        {
            self.show(hub, frame, sdr_white_nits, out_of_sight);
        }
    }

    /// Lets go of the own surface, before GTK's goes.
    pub fn end(&self) {
        self.own.borrow_mut().take();
    }

    /// The own surface over GTK's, made when GTK's is there, and again if GTK made another.
    fn own_surface(&self, hub: &Hub) -> bool {
        let Some(surface) = self.picture.native().and_then(|native| native.surface()) else { return false };
        if !surface.is_mapped() {
            return false;
        }
        let mut own = self.own.borrow_mut();
        if own.as_ref().is_some_and(|(made_for, _)| *made_for == surface) {
            return true;
        }
        own.take();
        match OwnSurface::new_above(&surface.display(), &surface) {
            Some(made) => {
                *own = Some((surface, made));
                true
            }
            None => {
                hub.note(&format!("{}: the compositor can't take a surface of the view's own", self.name));
                false
            }
        }
    }
}
