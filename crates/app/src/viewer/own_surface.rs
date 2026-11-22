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

use super::dmabuf_feedback::{self, Reading};
use super::frames::SlotFrame;
pub use super::stall::Stall;
use super::stall::StallWatch;
use gdk_wayland::prelude::*;
use gdk_wayland::{WaylandDisplay, WaylandSurface};
use gtk::gdk;
use std::collections::{HashMap, HashSet};
use std::os::fd::AsFd;
use std::sync::OnceLock;
use std::time::Instant;
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

/// Why the surface didn't show a frame, which goes to GTK instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// A buffer's slot. The compositor makes the buffer before it's known whose it is, so the slot is
/// filled in when the buffer arrives.
type BufferSlot = OnceLock<SlotKey>;

/// What the compositor has said, gathered while GTK reads.
#[derive(Default)]
struct Events {
    /// The slots each buffer still holds for the compositor, given back on `release`.
    held: HashMap<SlotKey, Box<dyn FnOnce() + Send>>,
    /// Slots whose buffer the compositor refused, so their frames go to GTK instead.
    refused: Vec<SlotKey>,
    /// Buffers the compositor has made, not yet taken in.
    made: Vec<(SlotKey, WlBuffer)>,
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

impl Dispatch<WlBuffer, BufferSlot> for Events {
    fn event(
        events: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        slot: &BufferSlot,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event
            && let Some(key) = slot.get()
            && let Some(release) = events.held.remove(key)
        {
            release();
        }
    }
}

/// The compositor's answer to a buffer asked for with `create`. A refusal only sends that slot's
/// frames to GTK. `create_immed` would make a refusal a protocol error, which ends the connection
/// GTK shares with this: GNOME refuses NVIDIA's 10-bit frames in rows that way.
impl Dispatch<ZwpLinuxBufferParamsV1, SlotKey> for Events {
    fn event(
        events: &mut Self,
        params: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        key: &SlotKey,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => {
                if let Some(slot) = buffer.data::<BufferSlot>() {
                    let _ = slot.set(*key);
                }
                events
                    .notes
                    .push(format!("own surface: the compositor made the buffer of {}", slot_name(*key)));
                events.made.push((*key, buffer));
            }
            zwp_linux_buffer_params_v1::Event::Failed => {
                let refused = slot_name(*key);
                events.notes.push(format!("own surface: the compositor refused the buffer of {refused}"));
                events.refused.push(*key);
            }
            _ => return,
        }
        params.destroy();
    }

    wayland_client::event_created_child!(Events, ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (WlBuffer, BufferSlot::new()),
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
    buffers: HashMap<SlotKey, WlBuffer>,
    /// Slots whose buffer has been asked for, made or not.
    asked: HashSet<SlotKey>,
    /// Which ring the buffers are of. A new one makes them all stale.
    generation: Option<u32>,
    /// The PQ description on the surface, or `None` for plain sRGB.
    showing: Option<PqKey>,
    /// The guest's reference white last used, for a frame that comes without one.
    last_reference: Option<u32>,
    place: Option<(i32, i32, i32, i32)>,
    /// Whether the frames sent reach the screen.
    stall: StallWatch,
}

impl OwnSurface {
    /// Under `parent`, which must be a Wayland toplevel. `None` if this isn't Wayland or the
    /// compositor lacks something this needs.
    pub fn new(display: &gdk::Display, parent: &gdk::Surface) -> Option<Self> {
        let display = display.downcast_ref::<WaylandDisplay>()?;
        let parent = parent.downcast_ref::<WaylandSurface>()?.wl_surface()?;
        let backend = display.wl_display()?.backend().upgrade()?;
        let connection = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<Events>(&connection).ok()?;
        let handle = queue.handle();
        let compositor: WlCompositor = bind(&globals, &handle, COMPOSITOR_VERSION)?;
        let subcompositor: WlSubcompositor = bind(&globals, &handle, 1)?;
        let viewporter: WpViewporter = bind(&globals, &handle, 1)?;
        let dmabuf: ZwpLinuxDmabufV1 =
            globals.bind(&handle, DMABUF_VERSION..=DMABUF_FEEDBACK_VERSION, ()).ok()?;
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
        subsurface.place_below(&parent);
        let viewport = viewporter.get_viewport(&surface, &handle, ());
        let dmabuf_feedback = (dmabuf.version() >= DMABUF_FEEDBACK_VERSION)
            .then(|| dmabuf.get_surface_feedback(&surface, &handle, ()));
        let mut events = Events::default();
        // The manager says what it offers before the description is asked for.
        queue.roundtrip(&mut events).ok()?;
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
        Some(Self {
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
            generation: None,
            showing: None,
            last_reference: None,
            place: None,
            stall: StallWatch::default(),
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
        let key = (frame.generation, frame.slot);
        if self.events.refused.contains(&key) {
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
            for (_, buffer) in self.buffers.drain() {
                buffer.destroy();
            }
            // A destroyed buffer hears no release, so its slot's file would stay open for good.
            let older: Vec<SlotKey> =
                self.events.held.keys().copied().filter(|key| key.0 != frame.generation).collect();
            for key in older {
                if let Some(release) = self.events.held.remove(&key) {
                    release();
                }
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
        self.surface.attach(Some(buffer), 0, 0);
        self.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        if let Some(presentation) = &self.presentation {
            presentation.feedback(&self.surface, &handle, ());
        }
        // The slot was shown before, and is still the compositor's until it says otherwise. That
        // can't happen, as the producer only sends a slot it has back, but a stale hold must not
        // keep a slot forever.
        if let Some(stale) = self.events.held.insert(key, frame.release) {
            stale();
        }
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
    fn ask_for_buffer(&mut self, frame: &SlotFrame, key: SlotKey) {
        let descriptor = frame.descriptor;
        let modifier = descriptor.format.modifier;
        let params = self.dmabuf.create_params(&self.queue.handle(), key);
        params.add(
            frame.file.as_fd(),
            SINGLE_PLANE,
            descriptor.offset as u32,
            descriptor.stride,
            (modifier >> 32) as u32,
            modifier as u32,
        );
        params.create(
            descriptor.width as i32,
            descriptor.height as i32,
            descriptor.format.fourcc,
            zwp_linux_buffer_params_v1::Flags::empty(),
        );
        let _ = self.queue.flush();
    }

    /// Keeps the buffers the compositor has made for this ring, and lets go of any for an older one.
    fn take_made_buffers(&mut self) {
        for (key, buffer) in self.events.made.drain(..) {
            if Some(key.0) == self.generation {
                self.buffers.insert(key, buffer);
            } else {
                buffer.destroy();
            }
        }
    }

    /// Takes the picture off, and gives back every slot. The surface stays for the next frame.
    pub fn hide(&mut self) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        let _ = self.queue.flush();
        self.stall.nothing_awaited();
        for (_, release) in self.events.held.drain() {
            release();
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
