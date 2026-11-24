//! Ubuntu's own settings: its GPU, which is a preview.

use super::Shared;
use super::gpu_choice::add_gpu_choice;
use crate::rows::explain;
use adw::prelude::*;
use guesthouse_app_core::machine::Kind;

const SHORT: &str = "A preview";
const NOTE: &str = "This is a preview, and it has only run on my PC. If it can't start, or with it off, Ubuntu boots \
                    with a plain screen and says why.";

pub struct GpuPreview;

impl GpuPreview {
    pub fn new(shared: &Shared, group: &adw::PreferencesGroup) -> Self {
        let machine = shared.machine.borrow().clone();
        if machine.kind != Kind::Linux {
            return Self;
        }
        let gpu = adw::SwitchRow::builder().title("GPU").subtitle(SHORT).active(machine.gpu_preview).build();
        explain(&gpu, NOTE);
        let saving = shared.clone();
        gpu.connect_active_notify(move |row| {
            saving.machine.borrow_mut().gpu_preview = row.is_active();
            saving.save();
        });
        group.add(&gpu);
        add_gpu_choice(shared, group);
        Self
    }
}
