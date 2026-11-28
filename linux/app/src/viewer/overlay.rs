//! The figures over the screen. It only exists while it's on, and reads the frame count and the
//! host's sensors once a second, so with it off there's no thread and no timer.

use super::screen::Screen;
use gtk::glib;
use gtk::prelude::*;
use guesthouse_app_core::gpu_sample::{SAMPLE_INTERVAL, Sampler};
use guesthouse_app_core::host_sensors::HostSensors;
use guesthouse_app_core::overlay;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

const UPDATE: Duration = Duration::from_secs(1);
const MARGIN: i32 = 12;

/// Whether the figures are shown when the screen opens, and the file the machine's daemon publishes
/// its card's readings in.
pub struct OverlayChoice {
    pub shown: bool,
    pub sensors: PathBuf,
}

pub struct PerformanceOverlay {
    screen: Screen,
    label: gtk::Label,
    source: Option<glib::SourceId>,
}

impl PerformanceOverlay {
    /// `sensors` is the file the machine's daemon publishes its card's readings in.
    pub fn new(screen: &Screen, sensors: PathBuf) -> Self {
        let sampler = Sampler::start(Box::new(move || HostSensors::open_at(&sensors)), SAMPLE_INTERVAL);
        let label = gtk::Label::builder()
            .xalign(0.0)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_start(MARGIN)
            .margin_top(MARGIN)
            .can_target(false)
            .build();
        label.add_css_class("performance-overlay");
        screen.widget().add_overlay(&label);
        label.set_label(&overlay::text(0, 0.0, None, sampler.now(), screen.sdr_white_nits()));
        let shown = Rc::new(Cell::new((screen.frames_shown(), Instant::now())));
        let (watched, updating) = (screen.clone(), label.clone());
        let source = glib::timeout_add_local(UPDATE, move || {
            let (frames, since) = shown.get();
            let now = watched.frames_shown();
            let sample = sampler.latest();
            let seconds = since.elapsed().as_secs_f64();
            let nits = watched.sdr_white_nits();
            let text = overlay::text((now - frames) as u32, seconds, sample.as_ref(), sampler.now(), nits);
            updating.set_label(&text);
            shown.set((now, Instant::now()));
            glib::ControlFlow::Continue
        });
        Self { screen: screen.clone(), label, source: Some(source) }
    }
}

impl Drop for PerformanceOverlay {
    fn drop(&mut self) {
        if let Some(source) = self.source.take() {
            source.remove();
        }
        self.screen.widget().remove_overlay(&self.label);
    }
}
