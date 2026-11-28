//! Frames from the frame socket. A slot is this side's from its frame until its release, and a release
//! goes when GTK has let go of the texture, which it can do on any thread.

use super::frames::{FrameSource, SlotFrame};
use super::guest_cursor::{CursorPicture, no_pointer};
use super::import::texture;
use super::ring::Buffers;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use guesthouse_frame_socket::{
    CursorMove, Format, Frame, FrameSocket, Hello, Light, Message, Packet, Release, SOCKET_NAME,
};
use std::io;
use std::os::fd::AsFd;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

pub(super) const LINEAR: u64 = 0;
/// What a copy can show, so the producer always has something it can send.
const COPYABLE: [u32; 2] = [u32::from_le_bytes(*b"XR24"), u32::from_le_bytes(*b"AR24")];
/// What an HDR desktop is. A guest only sends it to a viewer that offers it, and a build with no way to
/// give a frame its colour doesn't.
const HDR_FORMAT: Format = Format { fourcc: u32::from_le_bytes(*b"XB30"), modifier: LINEAR };

pub struct SocketFrames {
    socket: FrameSocket,
    display: gdk::Display,
    buffers: Option<Buffers>,
    finished: Sender<Release>,
    releases: Receiver<Release>,
    /// The newest frame, taken in but not yet handed out.
    unshown: Option<Frame>,
    alive: bool,
    /// How the newest HDR picture wants to be shown. An SDR picture has none.
    light: Option<Light>,
    /// The guest's hardware cursor, if it has one.
    cursor: Option<CursorPicture>,
    cursor_at: Option<CursorMove>,
    /// The pointer over the screen no longer matches the cursor.
    cursor_changed: bool,
    /// Wakes the viewer as soon as something comes, so a frame goes on screen when the guest made
    /// it and not at the next look. A screen that can change its refresh follows the guest then.
    _watch: SocketWatch,
}

/// The main loop's watch on the socket, through a copy of its descriptor. It ends by itself once
/// the socket has gone.
struct SocketWatch(glib::Source);

impl Drop for SocketWatch {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

/// What this display can show, in the producer's order of preference. The message holds a limited
/// number and a display can offer hundreds, so the formats a guest's desktop uses go first. Frames
/// in memory are copied, which only works for the copyable formats.
///
/// Without `tiles` only layouts in rows are offered, so the producer sends rows. Frames in a card's
/// tiles are for a compositor that can't take its 10-bit rows, and a picture that comes out
/// scrambled or not at all in them still shows in rows.
fn formats(display: &gdk::Display, in_memory: bool, tiles: bool) -> Vec<Format> {
    if in_memory {
        return COPYABLE.map(|fourcc| Format { fourcc, modifier: LINEAR }).to_vec();
    }
    let dmabuf = display.dmabuf_formats();
    let offered = (0..dmabuf.n_formats()).map(|index| {
        let (fourcc, modifier) = dmabuf.format(index);
        Format { fourcc, modifier }
    });
    in_order(layouts(offered.collect(), tiles))
}

/// What's offered, or only its layouts in rows.
fn layouts(offered: Vec<Format>, tiles: bool) -> Vec<Format> {
    offered.into_iter().filter(|format| tiles || format.modifier == LINEAR).collect()
}

/// Whether this build and this display can take an HDR frame at all. A machine whose frames come in
/// memory can't, which its start sees to.
pub fn takes_hdr_frames(display: &gdk::Display) -> bool {
    formats(display, false, false).contains(&HDR_FORMAT)
}

/// HDR is only offered if the display has it, and it comes early, as the list can be cut short. Its
/// other layouts come with the desktop's, as the producer may lay out both in the card's tiles.
fn in_order(offered: Vec<Format>) -> Vec<Format> {
    let hdr = (cfg!(feature = "hdr") && offered.contains(&HDR_FORMAT)).then_some(HDR_FORMAT);
    let desktop = |format: &Format| {
        COPYABLE.contains(&format.fourcc) || (hdr.is_some() && format.fourcc == HDR_FORMAT.fourcc)
    };
    let (usual, others): (Vec<Format>, Vec<Format>) = offered.into_iter().partition(desktop);
    let linear = COPYABLE.map(|fourcc| Format { fourcc, modifier: LINEAR });
    let mut all = linear.to_vec();
    all.extend(hdr);
    let rest = usual.into_iter().chain(others);
    all.extend(rest.filter(|format| !linear.contains(format) && *format != HDR_FORMAT));
    all
}

impl SocketFrames {
    /// Fails if nothing is offering frames yet. `in_memory` asks for frames in memory here, for a
    /// screen on another GPU than the guest's card. `tiles` lets the producer lay frames out in
    /// the card's tiles. `arrived` is called from the main loop whenever the socket has something.
    pub fn connect(
        run_folder: &Path,
        display: &gdk::Display,
        refresh_millihertz: u32,
        in_memory: bool,
        tiles: bool,
        arrived: impl Fn() + 'static,
    ) -> io::Result<Self> {
        let socket = FrameSocket::connect(&run_folder.join(SOCKET_NAME))?;
        let hello = Hello {
            formats: formats(display, in_memory, tiles),
            refresh_millihertz,
            peak_nits: 0,
            wants_memory: in_memory,
        };
        socket.send(&Message::Hello(hello), &[])?;
        socket.send(&Message::CursorWanted, &[])?;
        socket.set_nonblocking(true)?;
        let watch = watch(&socket, arrived)?;
        let (finished, releases) = channel();
        Ok(Self {
            socket,
            display: display.clone(),
            buffers: None,
            finished,
            releases,
            unshown: None,
            alive: true,
            light: None,
            cursor: None,
            cursor_at: None,
            cursor_changed: false,
            _watch: watch,
        })
    }

    fn release(&mut self, generation: u32, slot: u32) {
        let message = Message::Release(Release { generation, slot });
        if self.socket.send(&message, &[]).is_err() {
            self.alive = false;
        }
    }

    fn send_releases(&mut self) {
        while let Ok(release) = self.releases.try_recv() {
            self.release(release.generation, release.slot);
        }
    }

    /// Gives back what has finished and takes in what has come, keeping only the newest frame. Each
    /// frame left unread holds a slot, so a viewer that stops reading leaves the producer none.
    fn catch_up(&mut self) {
        self.send_releases();
        let Some(newest) = self.take_messages() else { return };
        if let Some(older) = self.unshown.replace(newest) {
            self.release(older.generation, older.slot);
        }
    }

    /// Takes in everything waiting, and says which frame is the newest. An older one has been passed
    /// over, so its slot goes straight back.
    fn take_messages(&mut self) -> Option<Frame> {
        let mut newest: Option<Frame> = None;
        loop {
            match self.socket.receive() {
                Ok(None) => return newest,
                Ok(Some(Packet { message: Message::Ring(ring), fds })) => {
                    self.buffers = Buffers::new(&ring, fds);
                    newest = None;
                }
                Ok(Some(Packet { message: Message::CursorShape(shape), fds })) => {
                    self.cursor = fds.into_iter().next().and_then(|file| CursorPicture::read(&shape, file));
                    self.cursor_changed = true;
                }
                Ok(Some(Packet { message: Message::CursorMove(position), .. })) => {
                    let shown = self.cursor_at.is_some_and(|before| before.visible);
                    self.cursor_changed |= shown != position.visible;
                    self.cursor_at = Some(position);
                }
                Ok(Some(Packet { message: Message::Light(light), .. })) => {
                    self.light = Some(light);
                }
                Ok(Some(Packet { message: Message::Frame(frame), .. })) => {
                    if self.light.is_some_and(|light| light.slot != frame.slot) {
                        self.light = None;
                    }
                    if let Some(older) = newest.replace(frame) {
                        self.release(older.generation, older.slot);
                    }
                }
                Ok(Some(_)) => {}
                Err(_) => {
                    self.alive = false;
                    return newest;
                }
            }
        }
    }
}

/// Calls `arrived` each time the socket can be read. A socket that has gone stays readable, so the
/// watch stops there, and the next look finds the source gone.
fn watch(socket: &FrameSocket, arrived: impl Fn() + 'static) -> io::Result<SocketWatch> {
    let copy = socket.as_fd().try_clone_to_owned()?;
    let watched = gio::Socket::from_fd(copy).map_err(io::Error::other)?;
    let ended = glib::IOCondition::HUP | glib::IOCondition::ERR;
    let source = gio::prelude::SocketExtManual::create_source(
        &watched,
        glib::IOCondition::IN | ended,
        None::<&gio::Cancellable>,
        Some("guest frames"),
        glib::Priority::DEFAULT,
        move |_, condition| {
            arrived();
            if condition.intersects(ended) { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
        },
    );
    source.attach(None);
    Ok(SocketWatch(source))
}

impl FrameSource for SocketFrames {
    fn newest(&mut self) -> Option<gdk::Texture> {
        let frame = self.newest_slot()?;
        texture(&self.display, frame.file, &frame.descriptor, frame.release)
    }

    fn newest_slot(&mut self) -> Option<SlotFrame> {
        self.catch_up();
        let frame = self.unshown.take()?;
        let Some(file) = self.buffers.as_ref().and_then(|buffers| buffers.slot(&frame)) else {
            self.release(frame.generation, frame.slot);
            return None;
        };
        let finished = self.finished.clone();
        let released = Release { generation: frame.generation, slot: frame.slot };
        let release = Box::new(move || {
            let _ = finished.send(released);
        });
        let (generation, slot) = (frame.generation, frame.slot);
        Some(SlotFrame { file, descriptor: frame.descriptor, generation, slot, release })
    }

    fn keep_up(&mut self) {
        self.catch_up();
    }

    fn alive(&self) -> bool {
        self.alive
    }

    fn sdr_white_nits(&self) -> Option<u32> {
        self.light.map(|light| light.sdr_white_nits)
    }

    fn drawn_cursor(&self) -> Option<(CursorPicture, CursorMove)> {
        let position = self.cursor_at.filter(|position| position.visible)?;
        Some((self.cursor.clone()?, position))
    }

    fn new_cursor(&mut self) -> Option<gdk::Cursor> {
        if !std::mem::take(&mut self.cursor_changed) {
            return None;
        }
        match (&self.cursor, self.cursor_at) {
            (_, Some(position)) if !position.visible => no_pointer(),
            (Some(cursor), _) => Some(cursor.pointer()),
            (None, _) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(name: &[u8; 4], modifier: u64) -> Format {
        Format { fourcc: u32::from_le_bytes(*name), modifier }
    }

    #[test]
    fn what_every_guest_can_send_comes_first() {
        let found = in_order(vec![format(b"NV12", 0), format(b"XR24", 7)]);
        assert_eq!(found[0], format(b"XR24", LINEAR));
        assert_eq!(found[1], format(b"AR24", LINEAR));
    }

    #[test]
    #[cfg(feature = "hdr")]
    fn hdr_is_offered_only_by_a_display_that_has_it() {
        assert!(!in_order(vec![format(b"XR24", 0)]).contains(&HDR_FORMAT));
        let found = in_order(vec![format(b"NV12", 0), HDR_FORMAT]);
        assert_eq!(found[2], HDR_FORMAT);
        assert_eq!(found.iter().filter(|format| **format == HDR_FORMAT).count(), 1);
    }

    #[test]
    #[cfg(not(feature = "hdr"))]
    fn a_build_with_no_colour_never_offers_hdr() {
        assert!(!in_order(vec![HDR_FORMAT]).contains(&HDR_FORMAT));
    }

    #[test]
    fn tiled_layouts_are_left_out_when_they_arent_wanted() {
        let offered = vec![format(b"XR24", 7), HDR_FORMAT, format(b"XB30", 7)];
        assert_eq!(layouts(offered.clone(), true), offered);
        assert_eq!(layouts(offered, false), vec![HDR_FORMAT]);
    }

    #[test]
    fn hdr_with_a_tiled_layout_is_not_enough() {
        assert!(!in_order(vec![format(b"XB30", 7)]).contains(&HDR_FORMAT));
    }

    #[test]
    #[cfg(feature = "hdr")]
    fn hdr_in_tiles_comes_before_formats_no_desktop_uses() {
        let found = in_order(vec![format(b"NV12", 0), format(b"XB30", 7), HDR_FORMAT]);
        let tiled = found.iter().position(|found| *found == format(b"XB30", 7));
        let video = found.iter().position(|found| *found == format(b"NV12", 0));
        assert!(tiled < video);
    }
}
