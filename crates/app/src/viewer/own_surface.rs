//! The guest's frames on a Wayland surface of the app's own, put under the window's as a subsurface.
//!
//! GTK can hand a frame to the compositor too, but only in whole steps at fractional scales, and only
//! with the compositor's colour management, which it turns down on KWin 6.8. Here the compositor gets
//! the picture's size in logical pixels and does the rounding itself, and an HDR frame says it's PQ
//! in BT.2020 through the colour management protocol. Each slot of the ring is made into a buffer
//! once, and goes back to the producer when the compositor lets go of it.
//!
//! Like the pointer lock, this lives on GTK's own connection with a queue of its own. GTK goes on
//! reading the socket for everyone, and `catch_up` takes in what landed on this queue.

use super::buffer_holds::BufferHolds;
use super::crop::Crop;
use super::dmabuf_feedback::{self, Reading};
use super::frames::SlotFrame;
pub use super::stall::Stall;
use super::stall::StallWatch;
use gdk_wayland::prelude::*;
use gdk_wayland::{WaylandDisplay, WaylandSurface};
use gtk::gdk;
use std::collections::{HashMap, HashSet};
use std::os::fd::AsFd;
use std::time::Instant;
use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::color_management::v1::client::wp_color_management_surface_feedback_v1::{
    self, WpColorManagementSurfaceFeedbackV1,
};
use wayland_protocols::wp::color_management::v1::client::wp_color_management_surface_v1::WpColorManagementSurfaceV1;
use wayland_protocols::wp::color_management::v1::client::wp_color_manager_v1::{
    self, Feature, Primaries, RenderIntent, TransferFunction, WpColorManagerV1,
};
use wayland_protocols::wp::color_management::v1::client::wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1;
use wayland_protocols::wp::color_management::v1::client::wp_image_description_info_v1::{
    self, WpImageDescriptionInfoV1,
};
use wayland_protocols::wp::color_management::v1::client::wp_image_description_v1::{
    self, WpImageDescriptionV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{
    self, ZwpLinuxBufferParamsV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_feedback_v1::{
    self, TrancheFlags, ZwpLinuxDmabufFeedbackV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::{
    self, Kind, WpPresentationFeedback,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

/// `damage_buffer` came in version 4.
const COMPOSITOR_VERSION: u32 = 4;
/// Version 3 takes a modifier with each plane, and `create_immed` is from 2.
const DMABUF_VERSION: u32 = 3;
/// Version 4 says, for one surface, what could go straight on the display.
const DMABUF_FEEDBACK_VERSION: u32 = 4;
const COLOUR_VERSION: u32 = 1;
/// H.273's code points, as the frame socket carries them.
pub(super) const PQ_TRANSFER: u8 = 16;
const BT2020_PRIMARIES: u8 = 9;
const SINGLE_PLANE: u32 = 0;
/// What `wp_viewport.set_source` takes to show the whole buffer again.
const WHOLE_BUFFER: f64 = -1.0;
/// PQ's own reference white, for a guest that hasn't said where its SDR white is.
const PQ_REFERENCE_NITS: u32 = 203;
/// The darkest a PQ signal goes, in the protocol's units of 0.0001 cd/m². Its peak is fixed at
/// 10,000 cd/m² above this, whatever is sent.
const PQ_FLOOR: u32 = 50;
const PQ_PEAK_NITS: u32 = 10_000;

/// A PQ description, by the guest's reference white and the screen's peak, in cd/m².
type PqKey = (u32, u32);

/// Which description the compositor is answering about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Described {
    Pq(PqKey),
    /// What the compositor would like this surface to be, which says how bright its screen goes.
    Preferred,
}

/// Why the surface didn't show a frame. The guest's screen then has GTK show it; an app window, whose
/// picture GTK never draws, shows nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotShown {
    NotPlaced,
    PeakNotKnown,
    DescriptionNotMade,
    BufferNotMade,
    BufferRefused,
    HdrNotOffered,
    DescriptionRefused,
}

impl NotShown {
    /// The surface is still waiting for something from the compositor, and can take the frame
    /// once it has it.
    pub fn may_later(self) -> bool {
        matches!(self, Self::NotPlaced | Self::PeakNotKnown | Self::DescriptionNotMade | Self::BufferNotMade)
    }

    /// For the log.
    pub fn why(self) -> &'static str {
        match self {
            Self::NotPlaced => "the picture has no place in the window yet",
            Self::PeakNotKnown => "the compositor hasn't said how bright the screen goes",
            Self::DescriptionNotMade => "the compositor hasn't made the HDR description yet",
            Self::BufferNotMade => "the compositor hasn't made the slot's buffer yet",
            Self::BufferRefused => "the compositor refused the slot's buffer",
            Self::HdrNotOffered => "the compositor doesn't offer PQ in BT.2020",
            Self::DescriptionRefused => "the compositor refused the HDR description",
        }
    }
}

/// A ring's slot, which names the buffer made from it.
type SlotKey = (u32, u32);

/// A slot as the log names it.
fn slot_name((generation, slot): SlotKey) -> String {
    format!("slot {slot} of ring {generation}")
}

/// The part of a frame a buffer is cut to, as x, y, width and height in its pixels: the buffer starts at
/// that pixel of the slot (its plane's offset) and is that large, so the compositor is given no
/// source rectangle away from the buffer's corner. `None` for a buffer of the whole frame.
type Window = Option<(i32, i32, i32, i32)>;
/// A buffer: its slot, and the part of the frame it is cut to.
type BufferKey = (SlotKey, Window);

/// The bytes a pixel takes in the single-plane formats a buffer can be cut from by its offset.
fn pixel_bytes(fourcc: u32) -> Option<u32> {
    match &fourcc.to_le_bytes() {
        b"XR24" | b"AR24" | b"XB24" | b"AB24" | b"RX24" | b"RA24" | b"BX24" | b"BA24" | b"XR30" | b"AR30"
        | b"XB30" | b"AB30" => Some(4),
        b"XR4H" | b"AR4H" | b"XB4H" | b"AB4H" => Some(8),
        _ => None,
    }
}

/// DRM's modifier for rows one after another, the only layout whose pixel's place is offset + y * stride
/// + x * bytes.
const LINEAR: u64 = 0;
/// The boundary a cut buffer's start is put on.
const CUT_ALIGNMENT: u64 = 256;

/// What the compositor has said, gathered while GTK reads.
#[derive(Default)]
struct Events {
    /// The slots each buffer still holds for the compositor, given back on its `release`, and the
    /// buffers no longer wanted that wait for it.
    holds: BufferHolds<ObjectId, WlBuffer>,
    /// Buffers the compositor refused: a slot's whole frame (its frames go to GTK instead), or a cut
    /// one (cut buffers are not asked for again).
    refused: Vec<BufferKey>,
    /// Buffers the compositor has made, not yet taken in.
    made: Vec<(BufferKey, WlBuffer)>,
    /// The PQ descriptions the compositor has made, or couldn't.
    ready: HashSet<PqKey>,
    failed: HashSet<PqKey>,
    /// How bright the screen under the surface goes, once the compositor has said.
    peak: Option<u32>,
    reading_peak: Option<u32>,
    /// The screen under the surface has changed, so its peak has to be asked for again.
    preferred_changed: bool,
    mastering_offered: bool,
    pq_offered: bool,
    parametric_offered: bool,
    luminances_offered: bool,
    presented: u64,
    zero_copy: u64,
    discarded: u64,
    feedback: Reading,
    /// The format of the frames being shown, for the feedback's log line.
    showing_format: Option<(u32, u64)>,
    /// What the feedback last said, so the log only gets what's new.
    feedback_said: String,
    /// Lines for the app's log, which the screen takes away.
    notes: Vec<String>,
}

impl Dispatch<WlRegistry, GlobalListContents> for Events {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Nothing these send needs an answer.
macro_rules! quiet {
    ($($interface:ty),*) => {$(
        impl Dispatch<$interface, ()> for Events {
            fn event(_: &mut Self, _: &$interface, _: <$interface as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
quiet!(
    WlCompositor,
    WlSubcompositor,
    WlSubsurface,
    WlSurface,
    WlRegion,
    WpViewporter,
    WpViewport,
    ZwpLinuxDmabufV1,
    WpPresentation,
    WpColorManagementSurfaceV1,
    WpImageDescriptionCreatorParamsV1
);

impl Dispatch<WpColorManagerV1, ()> for Events {
    fn event(
        events: &mut Self,
        _: &WpColorManagerV1,
        event: wp_color_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_color_manager_v1::Event::SupportedTfNamed { tf: WEnum::Value(TransferFunction::St2084Pq) } => {
                events.pq_offered = true;
            }
            wp_color_manager_v1::Event::SupportedFeature { feature: WEnum::Value(Feature::Parametric) } => {
                events.parametric_offered = true;
            }
            wp_color_manager_v1::Event::SupportedFeature {
                feature: WEnum::Value(Feature::SetLuminances),
            } => {
                events.luminances_offered = true;
            }
            wp_color_manager_v1::Event::SupportedFeature {
                feature: WEnum::Value(Feature::SetMasteringDisplayPrimaries),
            } => events.mastering_offered = true,
            _ => {}
        }
    }
}

impl Dispatch<WpImageDescriptionV1, Described> for Events {
    fn event(
        events: &mut Self,
        description: &WpImageDescriptionV1,
        event: wp_image_description_v1::Event,
        described: &Described,
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        let ready = matches!(
            event,
            wp_image_description_v1::Event::Ready { .. } | wp_image_description_v1::Event::Ready2 { .. }
        );
        let failed = matches!(event, wp_image_description_v1::Event::Failed { .. });
        match described {
            Described::Pq(key) if ready => {
                events.ready.insert(*key);
            }
            Described::Pq(key) if failed => {
                events.notes.push("own surface: the compositor refused the HDR description".to_owned());
                events.failed.insert(*key);
            }
            Described::Preferred if ready => {
                description.get_information(handle, ());
                description.destroy();
            }
            Described::Preferred if failed => description.destroy(),
            _ => {}
        }
    }
}

impl Dispatch<WpImageDescriptionInfoV1, ()> for Events {
    fn event(
        events: &mut Self,
        _: &WpImageDescriptionInfoV1,
        event: wp_image_description_info_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // What the screen itself reaches, which is what a picture for it should stay within.
            wp_image_description_info_v1::Event::TargetLuminance { max_lum, .. } => {
                events.reading_peak = Some(max_lum);
            }
            wp_image_description_info_v1::Event::Luminances { max_lum, .. } => {
                events.reading_peak = events.reading_peak.or(Some(max_lum));
            }
            wp_image_description_info_v1::Event::Done => {
                events.peak = events.reading_peak.take().filter(|peak| *peak > 0);
            }
            _ => {}
        }
    }
}

impl Dispatch<WpColorManagementSurfaceFeedbackV1, ()> for Events {
    fn event(
        events: &mut Self,
        _: &WpColorManagementSurfaceFeedbackV1,
        event: wp_color_management_surface_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(
            event,
            wp_color_management_surface_feedback_v1::Event::PreferredChanged { .. }
                | wp_color_management_surface_feedback_v1::Event::PreferredChanged2 { .. }
        ) {
            events.preferred_changed = true;
        }
    }
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for Events {
    fn event(
        events: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let reading = &mut events.feedback;
        match event {
            zwp_linux_dmabuf_feedback_v1::Event::FormatTable { fd, size } => reading.format_table(&fd, size),
            zwp_linux_dmabuf_feedback_v1::Event::TrancheFormats { indices } => {
                reading.tranche_formats(&indices)
            }
            zwp_linux_dmabuf_feedback_v1::Event::TrancheFlags { flags } => {
                let scanout = matches!(flags, WEnum::Value(flags) if flags.contains(TrancheFlags::Scanout));
                reading.tranche_scanout(scanout);
            }
            zwp_linux_dmabuf_feedback_v1::Event::TrancheDone => reading.tranche_done(),
            zwp_linux_dmabuf_feedback_v1::Event::Done => {
                let tranches = reading.done();
                let said = dmabuf_feedback::describe(&tranches, events.showing_format);
                if said != events.feedback_said {
                    events.notes.push(format!("own surface feedback: {said}"));
                    events.feedback_said = said;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlBuffer, ()> for Events {
    fn event(
        events: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event
            && let Some(retired) = events.holds.released(&buffer.id())
        {
            retired.destroy();
        }
    }
}

/// The compositor's answer to a buffer asked for with `create`. A refusal only sends that slot's
/// frames to GTK. `create_immed` would make a refusal a protocol error, which ends the connection
/// GTK shares with this: GNOME refuses NVIDIA's 10-bit frames in rows that way.
impl Dispatch<ZwpLinuxBufferParamsV1, BufferKey> for Events {
    fn event(
        events: &mut Self,
        params: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        key: &BufferKey,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => {
                let name = slot_name(key.0);
                events.notes.push(format!(
                    "own surface: the compositor made the buffer of {name} cut to {:?}",
                    key.1
                ));
                events.made.push((*key, buffer));
            }
            zwp_linux_buffer_params_v1::Event::Failed => {
                let refused = slot_name(key.0);
                events.notes.push(format!(
                    "own surface: the compositor refused the buffer of {refused} cut to {:?}",
                    key.1
                ));
                events.refused.push(*key);
            }
            _ => return,
        }
        params.destroy();
    }

    wayland_client::event_created_child!(Events, ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (WlBuffer, ()),
    ]);
}

impl Dispatch<WpPresentationFeedback, ()> for Events {
    fn event(
        events: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented { flags, .. } => {
                events.presented += 1;
                if let WEnum::Value(flags) = flags
                    && flags.contains(Kind::ZeroCopy)
                {
                    events.zero_copy += 1;
                }
            }
            wp_presentation_feedback::Event::Discarded => events.discarded += 1,
            _ => {}
        }
    }
}

fn bind<I>(globals: &GlobalList, handle: &QueueHandle<Events>, version: u32) -> Option<I>
where
    I: Proxy + 'static,
    Events: Dispatch<I, ()>,
{
    globals.bind(handle, version..=version, ()).ok()
}

/// Frames the compositor has shown, and how many of those went to the screen with no copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub presented: u64,
    pub zero_copy: u64,
    pub discarded: u64,
}

pub struct OwnSurface {
    queue: EventQueue<Events>,
    events: Events,
    dmabuf: ZwpLinuxDmabufV1,
    dmabuf_feedback: Option<ZwpLinuxDmabufFeedbackV1>,
    presentation: Option<WpPresentation>,
    surface: WlSurface,
    /// GTK's own surface, which this one sits under.
    parent: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
    manager: Option<WpColorManagerV1>,
    colour: Option<WpColorManagementSurfaceV1>,
    feedback: Option<WpColorManagementSurfaceFeedbackV1>,
    /// PQ in BT.2020, one for each reference white and screen peak used.
    descriptions: HashMap<PqKey, WpImageDescriptionV1>,
    buffers: HashMap<BufferKey, WlBuffer>,
    /// Buffers asked for, made or not.
    asked: HashSet<BufferKey>,
    /// Whether frames go on buffers cut to the crop by their offset (`set_cut_buffers`), and whether
    /// the compositor refused one, which leaves the viewport's source to do the cutting.
    cut_buffers: bool,
    cut_refused: bool,
    /// The buffer on the surface, and the size of its frame.
    attached_key: Option<(BufferKey, (i32, i32))>,
    /// Which ring the buffers are of. A new one makes them all stale.
    generation: Option<u32>,
    /// The PQ description on the surface, or `None` for plain sRGB.
    showing: Option<PqKey>,
    /// The guest's reference white last used, for a frame that comes without one.
    last_reference: Option<u32>,
    place: Option<(i32, i32, i32, i32)>,
    /// Whether the frames sent reach the screen.
    stall: StallWatch,
    /// The rectangle of each frame to show, as it was asked for: a program window's cell of the guest's screen.
    crop: Option<Crop>,
    /// The viewport's source now: the rectangle as it fits the buffer, or `None` for all of it.
    source: Option<Crop>,
    /// The size of the buffer on the surface, while there is one.
    attached: Option<(i32, i32)>,
    /// The viewport's source is the whole buffer, as set for a cut buffer.
    whole_source: bool,
}

impl OwnSurface {
    /// Under `parent`, which must be a Wayland toplevel. `None` if this isn't Wayland or the
    /// compositor lacks something this needs.
    pub fn new(display: &gdk::Display, parent: &gdk::Surface) -> Option<Self> {
        Self::made(display, parent, false).ok()
    }

    /// Over `parent`, a toplevel or a popup, for a window whose whole picture this is: GTK draws
    /// nothing there that has to stay on top, so its surface needs no clear part over the picture.
    /// When it can't be made, says why, for the user.
    pub fn new_above(display: &gdk::Display, parent: &gdk::Surface) -> Result<Self, &'static str> {
        Self::made(display, parent, true)
    }

    fn made(display: &gdk::Display, parent: &gdk::Surface, above: bool) -> Result<Self, &'static str> {
        const NOT_WAYLAND: &str = "the desktop isn't a Wayland one";
        let display = display.downcast_ref::<WaylandDisplay>().ok_or(NOT_WAYLAND)?;
        let parent = parent.downcast_ref::<WaylandSurface>().and_then(|parent| parent.wl_surface());
        let parent = parent.ok_or("the window has no Wayland surface")?;
        let backend =
            display.wl_display().and_then(|display| display.backend().upgrade()).ok_or(NOT_WAYLAND)?;
        let connection = Connection::from_backend(backend);
        let lost = "the connection to the desktop failed";
        let (globals, mut queue) = registry_queue_init::<Events>(&connection).map_err(|_| lost)?;
        let handle = queue.handle();
        let compositor: WlCompositor = bind(&globals, &handle, COMPOSITOR_VERSION)
            .ok_or("the desktop doesn't offer wl_compositor version 4")?;
        let subcompositor: WlSubcompositor =
            bind(&globals, &handle, 1).ok_or("the desktop doesn't offer subsurfaces (wl_subcompositor)")?;
        let viewporter: WpViewporter =
            bind(&globals, &handle, 1).ok_or("the desktop doesn't offer wp_viewporter")?;
        let dmabuf: ZwpLinuxDmabufV1 = globals
            .bind(&handle, DMABUF_VERSION..=DMABUF_FEEDBACK_VERSION, ())
            .map_err(|_| "the desktop doesn't offer linux-dmabuf version 3 or later")?;
        let presentation: Option<WpPresentation> = bind(&globals, &handle, 1);
        let manager: Option<WpColorManagerV1> = bind(&globals, &handle, COLOUR_VERSION);

        let surface = compositor.create_surface(&handle, ());
        // Clicks and keys go through to GTK's surface below.
        let nothing = compositor.create_region(&handle, ());
        surface.set_input_region(Some(&nothing));
        nothing.destroy();
        let subsurface = subcompositor.get_subsurface(&surface, &parent, &handle, ());
        subsurface.set_desync();
        // Under the window, which leaves the picture's place clear while this shows frames. What GTK
        // draws over the screen, like the overlay, toasts and the captured cursor, stays on top.
        if above {
            subsurface.place_above(&parent);
        } else {
            subsurface.place_below(&parent);
        }
        let viewport = viewporter.get_viewport(&surface, &handle, ());
        let dmabuf_feedback = (dmabuf.version() >= DMABUF_FEEDBACK_VERSION)
            .then(|| dmabuf.get_surface_feedback(&surface, &handle, ()));
        let mut events = Events::default();
        // The manager says what it offers before the description is asked for.
        queue.roundtrip(&mut events).map_err(|_| lost)?;
        events.notes.push(format!(
            "own surface: started. The compositor's colour management: {}, PQ {}, parametric {}, \
             luminances {}, mastering {}",
            if manager.is_some() { "offered" } else { "none" },
            events.pq_offered,
            events.parametric_offered,
            events.luminances_offered,
            events.mastering_offered,
        ));
        let manager = manager.filter(|_| events.pq_offered && events.parametric_offered);
        let colour = manager.as_ref().map(|manager| manager.get_surface(&surface, &handle, ()));
        let feedback = manager.as_ref().map(|manager| {
            let feedback = manager.get_surface_feedback(&surface, &handle, ());
            feedback.get_preferred(&handle, Described::Preferred);
            feedback
        });
        Ok(Self {
            queue,
            events,
            dmabuf,
            dmabuf_feedback,
            presentation,
            surface,
            parent,
            subsurface,
            viewport,
            manager,
            colour,
            feedback,
            descriptions: HashMap::new(),
            buffers: HashMap::new(),
            asked: HashSet::new(),
            cut_buffers: false,
            cut_refused: false,
            attached_key: None,
            generation: None,
            showing: None,
            last_reference: None,
            place: None,
            stall: StallWatch::default(),
            crop: None,
            source: None,
            whole_source: false,
            attached: None,
        })
    }

    /// Takes in what the compositor has said, which gives slots back. This doesn't read or wait.
    pub fn catch_up(&mut self) {
        let _ = self.queue.dispatch_pending(&mut self.events);
        self.take_made_buffers();
        if std::mem::take(&mut self.events.preferred_changed)
            && let Some(feedback) = &self.feedback
        {
            feedback.get_preferred(&self.queue.handle(), Described::Preferred);
        }
        let _ = self.queue.flush();
    }

    /// What has happened since the last call, in lines for the app's log.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.events.notes)
    }

    /// Whether HDR frames can be shown as HDR here.
    pub fn shows_hdr(&self) -> bool {
        self.colour.is_some()
    }

    pub fn shown(&self) -> Shown {
        Shown {
            presented: self.events.presented,
            zero_copy: self.events.zero_copy,
            discarded: self.events.discarded,
        }
    }

    /// Where the picture goes on the parent, in its logical pixels, and whether that moved it.
    pub fn place(&mut self, x: i32, y: i32, width: i32, height: i32) -> bool {
        if self.place == Some((x, y, width, height)) || width <= 0 || height <= 0 {
            return false;
        }
        self.place = Some((x, y, width, height));
        self.subsurface.set_position(x, y);
        self.viewport.set_destination(width, height);
        self.surface.commit();
        // A new place waits for the parent's next commit, and GTK makes none while it has nothing new
        // to draw: a guest whose screen changed shape under a still window stayed where the old shape
        // was. GTK sets and commits its own state in one go, so nothing of its is half done here.
        self.parent.commit();
        let _ = self.queue.flush();
        true
    }

    /// Cuts each frame to the crop by the buffer's own start and size (its plane's offset) instead of
    /// the viewport's source rectangle, for frames whose layout allows it (linear rows of one plane).
    /// No copy either way. A compositor that draws a viewport's source away from the buffer's corner
    /// wrongly (COSMIC 1.11 cuts the picture short by twice the source's offset, measured) shows the
    /// frame right this way; a compositor that refuses such a buffer gets the source rectangle again.
    pub fn set_cut_buffers(&mut self, on: bool) {
        self.cut_buffers = on;
    }

    /// Shows just this rectangle of each frame, or all of it with `None`. A frame already up
    /// is cut to it at once, as a still guest may send no other for a long time. Says whether the
    /// frame up has to be shown again to be cut to it (a buffer cut by its offset).
    pub fn set_crop(&mut self, crop: Option<Crop>) -> bool {
        self.crop = crop;
        if let Some(((_, Some(window)), frame_size)) = self.attached_key {
            let now = crop.and_then(|crop| crop.within(frame_size)).map(|c| (c.x, c.y, c.width, c.height));
            return now != Some(window);
        }
        if let Some(size) = self.attached
            && self.cut_to(size)
        {
            self.surface.commit();
            let _ = self.queue.flush();
        }
        false
    }

    /// The part of this frame its buffer is to be cut to by its offset, if it is to be.
    fn window_for(&self, frame: &SlotFrame) -> Window {
        let descriptor = frame.descriptor;
        if !self.cut_buffers || self.cut_refused || descriptor.format.modifier != LINEAR {
            return None;
        }
        let bytes = pixel_bytes(descriptor.format.fourcc)?;
        let crop = self.crop?.within((descriptor.width as i32, descriptor.height as i32))?;
        // GPUs take a buffer's start on a boundary of 256 bytes (measured: Mesa's radeonsi refused
        // every other one under mutter and COSMIC); a cell that does not start on one is cut by the
        // viewport's source instead.
        let offset = u64::from(descriptor.offset as u32)
            + crop.y as u64 * u64::from(descriptor.stride)
            + crop.x as u64 * u64::from(bytes);
        // The compositor takes the plane to run stride x height from its start, which must stay
        // inside the frame's own (a cell at the bottom right would reach past it by its x).
        let stride = u64::from(descriptor.stride);
        let end = offset + stride * crop.height as u64;
        let frame_end = u64::from(descriptor.offset as u32) + stride * u64::from(descriptor.height);
        (offset.is_multiple_of(CUT_ALIGNMENT) && end <= frame_end).then_some((
            crop.x,
            crop.y,
            crop.width,
            crop.height,
        ))
    }

    /// Sets the viewport's source for a buffer of this size, and says if it changed. It has to lie
    /// inside the buffer, or the compositor ends the connection, and that is GTK's as well.
    fn cut_to(&mut self, (width, height): (i32, i32)) -> bool {
        let source = self.crop.and_then(|crop| crop.within((width, height)));
        if source == self.source {
            return false;
        }
        match source {
            Some(Crop { x, y, width, height }) => {
                self.viewport.set_source(f64::from(x), f64::from(y), f64::from(width), f64::from(height))
            }
            None => self.viewport.set_source(WHOLE_BUFFER, WHOLE_BUFFER, WHOLE_BUFFER, WHOLE_BUFFER),
        }
        self.source = source;
        true
    }

    /// The PQ description for this reference white on this screen, once the compositor has made it.
    /// The first call asks for it.
    fn pq_description(&mut self, key: PqKey) -> Option<&WpImageDescriptionV1> {
        let manager = self.manager.as_ref()?;
        if !self.descriptions.contains_key(&key) {
            let (reference, peak) = key;
            let handle = self.queue.handle();
            let creator = manager.create_parametric_creator(&handle, ());
            creator.set_tf_named(TransferFunction::St2084Pq);
            creator.set_primaries_named(Primaries::Bt2020);
            // Without this the compositor takes PQ's reference white of 203 cd/m², and a guest
            // whose SDR white is elsewhere comes out too dark or too bright.
            if self.events.luminances_offered {
                creator.set_luminances(PQ_FLOOR, PQ_PEAK_NITS, reference);
            }
            // Without this the compositor has to allow for 10,000 cd/m² and squeezes the whole
            // picture down to fit the screen. The guest's desktop stays within the screen's peak.
            if self.events.mastering_offered {
                creator.set_mastering_luminance(PQ_FLOOR, peak);
            }
            self.descriptions.insert(key, creator.create(&handle, Described::Pq(key)));
            let asked = format!("own surface: HDR as PQ, SDR white at {reference} and peak {peak} cd/m²");
            self.events.notes.push(asked);
            let _ = self.queue.flush();
        }
        let usable = self.events.ready.contains(&key) && !self.events.failed.contains(&key);
        usable.then(|| &self.descriptions[&key])
    }

    /// Shows `frame`, or leaves it for GTK to show if the compositor can't take its buffer, or
    /// can't yet be told what it is. A frame that isn't shown gives its slot back here. `sdr_white_nits` is where the guest puts its SDR white in an HDR
    /// frame, if it has said.
    pub fn show(&mut self, frame: SlotFrame, sdr_white_nits: Option<u32>) -> Result<(), NotShown> {
        let slot_key = (frame.generation, frame.slot);
        if self.events.refused.iter().any(|(slot, window)| window.is_some() && *slot == slot_key) {
            if !self.cut_refused {
                self.events.notes.push(
                    "own surface: a buffer cut by its offset was refused, so the viewport cuts".to_owned(),
                );
            }
            self.cut_refused = true;
        }
        if self.events.refused.contains(&(slot_key, None)) {
            return Err(NotShown::BufferRefused);
        }
        if self.place.is_none() {
            return Err(NotShown::NotPlaced);
        }
        let colour = frame.descriptor.colour;
        let pq = colour.transfer == PQ_TRANSFER;
        if pq && !(self.shows_hdr() && colour.primaries == BT2020_PRIMARIES) {
            return Err(NotShown::HdrNotOffered);
        }
        // A frame that comes without its light keeps the last one, so the picture doesn't jump.
        let reference =
            sdr_white_nits.filter(|nits| *nits > 0).or(self.last_reference).unwrap_or(PQ_REFERENCE_NITS);
        self.last_reference = Some(reference);
        // Until the compositor has said how bright the screen goes, GTK shows HDR frames.
        let wanted = match (pq, self.events.peak) {
            (false, _) => None,
            (true, Some(peak)) => Some((reference, peak)),
            (true, None) => return Err(NotShown::PeakNotKnown),
        };
        if let Some(key) = wanted
            && self.pq_description(key).is_none()
        {
            let failed = self.events.failed.contains(&key);
            return Err(if failed { NotShown::DescriptionRefused } else { NotShown::DescriptionNotMade });
        }
        if self.generation != Some(frame.generation) {
            // The old ring's buffers the compositor may still show go when it lets go of them.
            let old: Vec<WlBuffer> = self.buffers.drain().map(|(_, buffer)| buffer).collect();
            for buffer in old {
                self.retire(buffer);
            }
            self.asked.clear();
            self.events.refused.clear();
            self.generation = Some(frame.generation);
        }
        let descriptor = frame.descriptor;
        let format = (descriptor.format.fourcc, descriptor.format.modifier);
        if self.events.showing_format != Some(format) {
            self.events.showing_format = Some(format);
            let layout = format!("{} {:#x}", dmabuf_feedback::fourcc_name(format.0), format.1);
            self.events.notes.push(format!("own surface: frames are {layout}"));
        }
        self.take_made_buffers();
        let window = self.window_for(&frame);
        let key = (slot_key, window);
        if window.is_some() {
            self.drop_other_windows(window);
        }
        let Some(buffer) = self.buffers.get(&key) else {
            if self.asked.insert(key) {
                self.ask_for_buffer(&frame, key);
            }
            // GTK shows the frame until the compositor has made its buffer, or said it can't.
            return Err(NotShown::BufferNotMade);
        };
        let handle = self.queue.handle();
        if wanted != self.showing {
            if let Some(colour) = &self.colour {
                match wanted.and_then(|key| self.descriptions.get(&key)) {
                    Some(description) => colour.set_image_description(description, RenderIntent::Perceptual),
                    None => colour.unset_image_description(),
                }
            }
            self.showing = wanted;
        }
        let frame_size = (descriptor.width as i32, descriptor.height as i32);
        let buffer_id = buffer.id();
        self.surface.attach(Some(buffer), 0, 0);
        self.attached_key = Some((key, frame_size));
        match window {
            Some((_, _, width, height)) => {
                // The buffer is the cut: all of it is shown.
                self.attached = Some((width, height));
                if self.source.take().is_some() || !self.whole_source {
                    self.viewport.set_source(WHOLE_BUFFER, WHOLE_BUFFER, WHOLE_BUFFER, WHOLE_BUFFER);
                    self.whole_source = true;
                }
            }
            None => {
                self.attached = Some(frame_size);
                self.whole_source = false;
                // In the same commit as the buffer, as a rectangle that fits it may not fit the last one.
                self.cut_to(frame_size);
            }
        }
        self.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        if let Some(presentation) = &self.presentation {
            presentation.feedback(&self.surface, &handle, ());
        }
        // The slot is the compositor's until it lets go of this buffer.
        self.events.holds.hold(buffer_id, frame.release);
        self.surface.commit();
        let _ = self.queue.flush();
        // Only a compositor that says what it showed can be watched.
        if self.presentation.is_some() {
            self.stall.sent(Instant::now());
        }
        Ok(())
    }

    /// Whether the frames sent reach the screen, as far as the compositor has said by now. Call
    /// [`Self::catch_up`] first. A window nobody can see is shown no frames, and that isn't looked
    /// into.
    pub fn watch(&mut self, now: Instant, seen: bool) -> Option<Stall> {
        if !seen {
            self.stall.nothing_awaited();
            return None;
        }
        self.stall.check(now, self.events.presented)
    }

    /// Asks the compositor for a buffer of this frame's slot. The answer comes back on the queue.
    fn ask_for_buffer(&mut self, frame: &SlotFrame, key: BufferKey) {
        let descriptor = frame.descriptor;
        let modifier = descriptor.format.modifier;
        let bytes = pixel_bytes(descriptor.format.fourcc).unwrap_or(4);
        let (offset, width, height) = match key.1 {
            Some((x, y, width, height)) => {
                (descriptor.offset as u32 + y as u32 * descriptor.stride + x as u32 * bytes, width, height)
            }
            None => (descriptor.offset as u32, descriptor.width as i32, descriptor.height as i32),
        };
        let params = self.dmabuf.create_params(&self.queue.handle(), key);
        params.add(
            frame.file.as_fd(),
            SINGLE_PLANE,
            offset,
            descriptor.stride,
            (modifier >> 32) as u32,
            modifier as u32,
        );
        params.create(width, height, descriptor.format.fourcc, zwp_linux_buffer_params_v1::Flags::empty());
        let _ = self.queue.flush();
    }

    /// Lets go of the buffers cut to another part of the frame than `window`, but the one on the
    /// surface: a cell that moved or changed size has no use for them.
    fn drop_other_windows(&mut self, window: Window) {
        let up = self.attached_key.map(|(key, _)| key);
        let stale: Vec<BufferKey> = self
            .buffers
            .keys()
            .copied()
            .filter(|key| key.1.is_some() && key.1 != window && Some(*key) != up)
            .collect();
        for key in stale {
            if let Some(buffer) = self.buffers.remove(&key) {
                self.retire(buffer);
            }
            self.asked.remove(&key);
        }
    }

    /// Destroys a buffer no longer wanted, or, while it still holds a slot, leaves it to go on its
    /// release: a destroyed buffer hears no release, and its slot would never go back.
    fn retire(&mut self, buffer: WlBuffer) {
        if let Some(buffer) = self.events.holds.retire(buffer.id(), buffer) {
            buffer.destroy();
        }
    }

    /// Keeps the buffers the compositor has made for this ring, and lets go of any for an older one.
    fn take_made_buffers(&mut self) {
        for (key, buffer) in self.events.made.drain(..) {
            if Some(key.0.0) == self.generation {
                self.buffers.insert(key, buffer);
            } else {
                buffer.destroy();
            }
        }
    }

    /// Takes the picture off, and gives back every slot. The surface stays for the next frame.
    pub fn hide(&mut self) {
        self.surface.attach(None, 0, 0);
        self.attached = None;
        self.attached_key = None;
        self.surface.commit();
        let _ = self.queue.flush();
        self.stall.nothing_awaited();
        // Nothing is on the surface now, so nothing it shows can be given back.
        for buffer in self.events.holds.release_all() {
            buffer.destroy();
        }
    }
}

impl Drop for OwnSurface {
    fn drop(&mut self) {
        self.hide();
        let made = self.events.made.drain(..).map(|(_, buffer)| buffer);
        for buffer in self.buffers.drain().map(|(_, buffer)| buffer).chain(made) {
            buffer.destroy();
        }
        if let Some(colour) = self.colour.take() {
            colour.destroy();
        }
        if let Some(feedback) = self.feedback.take() {
            feedback.destroy();
        }
        if let Some(feedback) = self.dmabuf_feedback.take() {
            feedback.destroy();
        }
        for (_, description) in self.descriptions.drain() {
            description.destroy();
        }
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
        let _ = self.queue.flush();
    }
}
