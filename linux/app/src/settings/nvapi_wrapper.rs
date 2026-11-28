//! An NVIDIA Windows machine's NVAPI: Guesthouse's wrapper, which DLSS needs, or NVIDIA's own, which
//! games whose anti-cheat refuses the wrapper need. It's on the Advanced page.

use super::Shared;
use crate::rows::explain;
use adw::prelude::*;
use guesthouse_app_core::gpu_maker::GpuMaker;

const TITLE: &str = "DLSS";
const SHORT: &str = "Off for games whose anti-cheat refuses it";
const NOTE: &str = "NVIDIA's NVAPI won't tell DLSS which card it's on in a virtual machine, so Guesthouse puts its \
                    own NVAPI in Windows' place for it. It isn't signed, and some games' anti-cheat refuses to start \
                    with it. Turn this off for those games, and NVIDIA's goes back. Takes effect when the machine \
                    starts.";

pub struct NvapiWrapper;

impl NvapiWrapper {
    pub fn new(shared: &Shared, group: &adw::PreferencesGroup) -> Self {
        let machine = shared.machine.borrow().clone();
        if machine.windows_maker() != Some(GpuMaker::Nvidia) {
            return Self;
        }
        let row =
            adw::SwitchRow::builder().title(TITLE).subtitle(SHORT).active(machine.nvapi_wrapper()).build();
        explain(&row, NOTE);
        let saving = shared.clone();
        row.connect_active_notify(move |row| {
            saving.machine.borrow_mut().nvapi_wrapper_off = !row.is_active();
            saving.save();
        });
        group.add(&row);
        Self
    }
}
