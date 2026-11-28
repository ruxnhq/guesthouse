//! What the compositor says each monitor expects, through the Wayland colour management protocol. A
//! monitor in HDR either expects PQ, the transfer function HDR10 is sent in, or has room above SDR
//! white: KWin 6.8 describes an HDR monitor as gamma 2.2 in BT.2020 with its peak above its reference
//! white. KWin, Mutter 48 and others offer the protocol, so this works on any desktop that has it,
//! where Mutter's D-Bus only works on GNOME.
//!
//! Like the pointer lock, this lives on GTK's own connection with a queue of its own. GTK goes on
//! reading the socket for everyone, and `catch_up` takes in what landed on this queue.

use crate::viewer::own_surface_takes_hdr;
use gdk_wayland::prelude::*;
use gdk_wayland::{WaylandDisplay, WaylandMonitor};
use gtk::gdk;
use guesthouse_app_core::display::ScreenHdr;
use std::cell::RefCell;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::color_management::v1::client::wp_color_management_output_v1::{
    self, WpColorManagementOutputV1,
};
use wayland_protocols::wp::color_management::v1::client::wp_color_manager_v1::{
    self, Feature, Primaries, RenderIntent, TransferFunction, WpColorManagerV1,
};
use wayland_protocols::wp::color_management::v1::client::wp_image_description_info_v1::{
    self, WpImageDescriptionInfoV1,
};
use wayland_protocols::wp::color_management::v1::client::wp_image_description_v1::{
    self, WpImageDescriptionV1,
};

/// The version GTK 4.24 binds, so that this is told what GTK is told. A compositor answers each
/// version in its own terms: Mutter 51 names sRGB's transfer function to version 1 and the compound
/// power curve in its place to version 2, and GTK looks for the first.
const GTK_VERSION: u32 = 1;
/// An output's description takes two round trips: one for it to be ready, one for what it says.
const ROUND_TRIPS_TO_SETTLE: usize = 3;

/// What an output's description said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    Hdr,
    Sdr,
    /// The compositor couldn't describe it, as when the output has just gone.
    Failed,
}

/// What a description has said so far.
#[derive(Debug, Clone, Copy, Default)]
struct Reading {
    transfer: Option<WEnum<TransferFunction>>,
    /// The peak and the reference white, in cd/m².
    luminances: Option<(u32, u32)>,
}

/// What a whole description says. One that names no transfer function and no luminances is an ICC
/// profile's, and those are for SDR screens. A peak at the reference white leaves HDR no room, which is
/// how a monitor in HDR looks with its SDR brightness all the way up.
fn seen_in(reading: Reading) -> Seen {
    let headroom = reading.luminances.is_some_and(|(peak, reference)| peak > reference);
    match reading.transfer {
        Some(WEnum::Value(TransferFunction::St2084Pq)) => Seen::Hdr,
        _ if headroom => Seen::Hdr,
        _ => Seen::Sdr,
    }
}

/// One monitor in HDR is enough. Monitors that haven't said, or couldn't, don't count either way.
fn answer_from(seen: impl IntoIterator<Item = Option<Seen>>) -> ScreenHdr {
    let mut any_said = false;
    for said in seen.into_iter().flatten() {
        match said {
            Seen::Hdr => return ScreenHdr::Shows,
            Seen::Sdr => any_said = true,
            Seen::Failed => {}
        }
    }
    if any_said { ScreenHdr::Cannot } else { ScreenHdr::CantTell }
}

/// What the compositor's colour manager says it can do, as far as GTK's choice depends on it.
#[derive(Debug, Clone, Copy, Default)]
struct Offered {
    parametric: bool,
    set_primaries: bool,
    srgb_transfer: bool,
    pq_transfer: bool,
    srgb_primaries: bool,
    perceptual: bool,
}

impl Offered {
    /// GTK uses the compositor's colour management only with all of these, and without it, it turns
    /// HDR frames into SDR itself (`gdk_wayland_color_set_color_manager`'s checks in GTK 4.24's
    /// gdkwaylandcolor.c). KWin 6.8 has no named sRGB transfer function, so GTK goes without there.
    fn gtk_takes_it(self) -> bool {
        self.parametric
            && self.srgb_transfer
            && (self.srgb_primaries || self.set_primaries)
            && self.perceptual
    }

    /// The viewer's own surface describes HDR frames itself, as PQ in parametric terms.
    fn own_surface_could_take_it(self) -> bool {
        self.parametric && self.pq_transfer && self.perceptual
    }

    /// What becomes of a screen's answer on the way through the toolkit. `own_surface` is whether
    /// the viewer's own surface is there to pass HDR on, as far as anyone has seen.
    fn passed_on(self, answer: ScreenHdr, own_surface: bool) -> ScreenHdr {
        match answer {
            ScreenHdr::Shows if self.gtk_takes_it() => ScreenHdr::Shows,
            ScreenHdr::Shows if own_surface && self.own_surface_could_take_it() => ScreenHdr::ShowsDirect,
            ScreenHdr::Shows => ScreenHdr::ToolkitCant,
            answer => answer,
        }
    }
}

/// Tells one output's objects from another's. A plain number, as the protocol's objects carry it.
type OutputKey = u32;

struct Output {
    key: OutputKey,
    monitor: gdk::Monitor,
    colour: WpColorManagementOutputV1,
    /// The description being read now.
    reading: Reading,
    seen: Option<Seen>,
}

#[derive(Default)]
struct Outputs {
    list: Vec<Output>,
    next_key: OutputKey,
    offered: Offered,
}

impl Outputs {
    fn get(&mut self, key: OutputKey) -> Option<&mut Output> {
        self.list.iter_mut().find(|output| output.key == key)
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Outputs {
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

/// The manager lists what the compositor supports, which says whether GTK will use it.
impl Dispatch<WpColorManagerV1, ()> for Outputs {
    fn event(
        outputs: &mut Self,
        _: &WpColorManagerV1,
        event: wp_color_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let offered = &mut outputs.offered;
        match event {
            wp_color_manager_v1::Event::SupportedFeature { feature: WEnum::Value(feature) } => {
                match feature {
                    Feature::Parametric => offered.parametric = true,
                    Feature::SetPrimaries => offered.set_primaries = true,
                    _ => {}
                }
            }
            wp_color_manager_v1::Event::SupportedTfNamed { tf: WEnum::Value(TransferFunction::Srgb) } => {
                offered.srgb_transfer = true;
            }
            wp_color_manager_v1::Event::SupportedTfNamed { tf: WEnum::Value(TransferFunction::St2084Pq) } => {
                offered.pq_transfer = true;
            }
            wp_color_manager_v1::Event::SupportedPrimariesNamed {
                primaries: WEnum::Value(Primaries::Srgb),
            } => {
                offered.srgb_primaries = true;
            }
            wp_color_manager_v1::Event::SupportedIntent {
                render_intent: WEnum::Value(RenderIntent::Perceptual),
            } => {
                offered.perceptual = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<WpColorManagementOutputV1, OutputKey> for Outputs {
    fn event(
        _: &mut Self,
        colour: &WpColorManagementOutputV1,
        event: wp_color_management_output_v1::Event,
        key: &OutputKey,
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        // Descriptions never change, so a new one has to be asked for. The old answer stands until then.
        if let wp_color_management_output_v1::Event::ImageDescriptionChanged = event {
            colour.get_image_description(handle, *key);
        }
    }
}

impl Dispatch<WpImageDescriptionV1, OutputKey> for Outputs {
    fn event(
        outputs: &mut Self,
        description: &WpImageDescriptionV1,
        event: wp_image_description_v1::Event,
        key: &OutputKey,
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        match event {
            wp_image_description_v1::Event::Ready { .. } | wp_image_description_v1::Event::Ready2 { .. } => {
                if let Some(output) = outputs.get(*key) {
                    output.reading = Reading::default();
                }
                description.get_information(handle, *key);
            }
            wp_image_description_v1::Event::Failed { .. } => {
                if let Some(output) = outputs.get(*key) {
                    output.seen = Some(Seen::Failed);
                }
            }
            _ => return,
        }
        // The information object stands on its own, so the description isn't needed any more.
        description.destroy();
    }
}

impl Dispatch<WpImageDescriptionInfoV1, OutputKey> for Outputs {
    fn event(
        outputs: &mut Self,
        _: &WpImageDescriptionInfoV1,
        event: wp_image_description_info_v1::Event,
        key: &OutputKey,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = outputs.get(*key) else { return };
        match event {
            wp_image_description_info_v1::Event::TfNamed { tf } => output.reading.transfer = Some(tf),
            wp_image_description_info_v1::Event::Luminances { max_lum, reference_lum, .. } => {
                output.reading.luminances = Some((max_lum, reference_lum));
            }
            wp_image_description_info_v1::Event::Done => {
                output.seen = Some(seen_in(std::mem::take(&mut output.reading)));
            }
            _ => {}
        }
    }
}

pub struct OutputColours {
    manager: WpColorManagerV1,
    queue: RefCell<EventQueue<Outputs>>,
    outputs: RefCell<Outputs>,
}

impl OutputColours {
    /// `None` if this isn't Wayland, or the compositor doesn't offer the protocol.
    pub fn new(display: &gdk::Display) -> Option<Self> {
        let display = display.downcast_ref::<WaylandDisplay>()?;
        let backend = display.wl_display()?.backend().upgrade()?;
        let connection = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<Outputs>(&connection).ok()?;
        let manager = globals.bind(&queue.handle(), GTK_VERSION..=GTK_VERSION, ()).ok()?;
        Some(Self { manager, queue: RefCell::new(queue), outputs: RefCell::default() })
    }

    /// Watches each of these monitors, and stops watching any other.
    pub fn follow(&self, monitors: &[gdk::Monitor]) {
        let queue = self.queue.borrow();
        let handle = queue.handle();
        let mut outputs = self.outputs.borrow_mut();
        outputs.list.retain(|output| {
            let keep = monitors.contains(&output.monitor);
            if !keep {
                output.colour.destroy();
            }
            keep
        });
        for monitor in monitors {
            if outputs.list.iter().any(|output| output.monitor == *monitor) {
                continue;
            }
            let wayland = monitor.downcast_ref::<WaylandMonitor>().and_then(WaylandMonitor::wl_output);
            let Some(wayland) = wayland else { continue };
            let key = outputs.next_key;
            outputs.next_key += 1;
            let colour = self.manager.get_output(&wayland, &handle, key);
            colour.get_image_description(&handle, key);
            outputs.list.push(Output {
                key,
                monitor: monitor.clone(),
                colour,
                reading: Reading::default(),
                seen: None,
            });
        }
        let _ = queue.flush();
    }

    /// Waits for the first descriptions, so there's an answer from the start.
    pub fn settle(&self) {
        let mut queue = self.queue.borrow_mut();
        let mut outputs = self.outputs.borrow_mut();
        for _ in 0..ROUND_TRIPS_TO_SETTLE {
            let all_said = outputs.list.iter().all(|output| output.seen.is_some());
            if all_said || queue.roundtrip(&mut outputs).is_err() {
                break;
            }
        }
    }

    /// Takes in what the compositor has said since the last look. This doesn't read or wait.
    pub fn catch_up(&self) {
        let mut queue = self.queue.borrow_mut();
        let _ = queue.dispatch_pending(&mut self.outputs.borrow_mut());
        let _ = queue.flush();
    }

    /// The answer for the monitors the window is on, or for all of them while it's on none.
    pub fn answer(&self, on: &[gdk::Monitor]) -> ScreenHdr {
        let outputs = self.outputs.borrow();
        let shown = outputs.list.iter().filter(|output| on.is_empty() || on.contains(&output.monitor));
        outputs.offered.passed_on(answer_from(shown.map(|output| output.seen)), own_surface_takes_hdr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(transfer: TransferFunction, luminances: Option<(u32, u32)>) -> Reading {
        Reading { transfer: Some(WEnum::Value(transfer)), luminances }
    }

    #[test]
    fn pq_is_hdr_and_anything_else_without_headroom_is_not() {
        assert_eq!(seen_in(named(TransferFunction::St2084Pq, None)), Seen::Hdr);
        for other in [TransferFunction::Gamma22, TransferFunction::Srgb, TransferFunction::Hlg] {
            assert_eq!(seen_in(named(other, None)), Seen::Sdr, "{other:?}");
            assert_eq!(seen_in(named(other, Some((80, 80)))), Seen::Sdr, "{other:?}");
        }
        assert_eq!(seen_in(Reading { transfer: Some(WEnum::Unknown(99)), luminances: None }), Seen::Sdr);
        assert_eq!(seen_in(Reading::default()), Seen::Sdr);
    }

    #[test]
    fn kwins_hdr_monitor_is_gamma_22_with_room_above_sdr_white() {
        // As KWin 6.8 describes a monitor with HDR on: BT.2020, gamma 2.2, peak 508, reference 265.
        assert_eq!(seen_in(named(TransferFunction::Gamma22, Some((508, 265)))), Seen::Hdr);
        // The same monitor with its SDR brightness at the peak has no room left.
        assert_eq!(seen_in(named(TransferFunction::Gamma22, Some((400, 400)))), Seen::Sdr);
    }

    #[test]
    fn hdr_gtk_cant_pass_on_needs_the_own_surface() {
        let kwin = Offered {
            parametric: true,
            set_primaries: true,
            pq_transfer: true,
            perceptual: true,
            ..Offered::default()
        };
        assert_eq!(kwin.passed_on(ScreenHdr::Shows, true), ScreenHdr::ShowsDirect);
        assert_eq!(kwin.passed_on(ScreenHdr::Shows, false), ScreenHdr::ToolkitCant, "refused, or kept off");
        let no_pq = Offered { pq_transfer: false, ..kwin };
        assert_eq!(no_pq.passed_on(ScreenHdr::Shows, true), ScreenHdr::ToolkitCant);
        let mutter = Offered { srgb_transfer: true, ..kwin };
        assert_eq!(mutter.passed_on(ScreenHdr::Shows, false), ScreenHdr::Shows, "GTK passes it on itself");
        assert_eq!(kwin.passed_on(ScreenHdr::Cannot, true), ScreenHdr::Cannot);
    }

    #[test]
    fn gtk_goes_without_on_kwin_68() {
        // What KWin 6.8 offers: parametric, set_primaries, perceptual, but no named sRGB transfer.
        let kwin = Offered {
            parametric: true,
            set_primaries: true,
            srgb_primaries: true,
            perceptual: true,
            ..Default::default()
        };
        assert!(!kwin.gtk_takes_it());
        assert!(Offered { srgb_transfer: true, ..kwin }.gtk_takes_it());
        assert!(!Offered { srgb_transfer: true, perceptual: false, ..kwin }.gtk_takes_it());
    }

    #[test]
    fn one_monitor_in_hdr_is_enough() {
        assert_eq!(answer_from([Some(Seen::Sdr), Some(Seen::Hdr)]), ScreenHdr::Shows);
        assert_eq!(answer_from([None, Some(Seen::Failed), Some(Seen::Hdr)]), ScreenHdr::Shows);
    }

    #[test]
    fn monitors_that_all_say_no_mean_no() {
        assert_eq!(answer_from([Some(Seen::Sdr), Some(Seen::Sdr)]), ScreenHdr::Cannot);
        assert_eq!(answer_from([Some(Seen::Sdr), Some(Seen::Failed), None]), ScreenHdr::Cannot);
    }

    #[test]
    fn nothing_said_means_it_cant_tell() {
        assert_eq!(answer_from([]), ScreenHdr::CantTell);
        assert_eq!(answer_from([None, Some(Seen::Failed)]), ScreenHdr::CantTell);
    }
}
