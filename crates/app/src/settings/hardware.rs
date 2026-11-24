//! Cores, memory and sound on the Overview, and V-Cache pinning and the video memory cap on the Advanced
//! page.

use super::{Shared, is_off};
use crate::rows::{explain, number_row};
use adw::prelude::*;
use guesthouse_app_core::host::{cpu_list, physical_cores, vcache_cpus};
use guesthouse_app_core::limits::{cores_range, ram_field};
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::settings::max_vram_gib_for;
use guesthouse_app_core::tasks::Status;

const VM_MUST_BE_OFF: &str = "The VM must be off to change these.";
const SOUND_SHORT: &str = "Through PipeWire";
const SOUND_NOTE: &str = "Off, the guest's sound card plays nothing, and QEMU gets no way to your audio server or any \
                          other socket of your desktop's.";
const VRAM_TITLE: &str = "Video Memory Cap";
const VRAM_NOTE: &str = "The most video memory one program in Windows can have. A program that goes past it loses \
                         its GPU device, and others carry on. 0 is no cap.";
const PIN_NOTE: &str =
    "Keeps the VM on the cores that sit beside the CPU's 3D V-Cache, which games run faster on.";

pub struct Hardware {
    group: adw::PreferencesGroup,
    advanced: adw::PreferencesGroup,
    cores: adw::SpinRow,
    ram: adw::SpinRow,
    vram: adw::SpinRow,
    pin: adw::SwitchRow,
    pin_possible: bool,
    sound: adw::SwitchRow,
}

impl Hardware {
    /// The video memory cap's row, whose top follows the machine's card.
    pub fn cap_row(&self) -> adw::SpinRow {
        self.vram.clone()
    }

    /// The sound row, which the Overview shows after the disk.
    pub fn sound_row(&self) -> &adw::SwitchRow {
        &self.sound
    }

    /// Cores and RAM go in `group`, and the expert rows in `advanced`.
    pub fn new(shared: &Shared, group: &adw::PreferencesGroup, advanced: &adw::PreferencesGroup) -> Self {
        let context = shared.window.context();
        let machine = shared.machine.borrow().clone();
        let host_cores = physical_cores(&context.cpus);
        let (least_cores, most_cores) = cores_range(&context, &machine);
        let cores = number_row("Cores", machine.cores.into(), least_cores.into(), most_cores.into());
        cores.set_subtitle(&format!("This PC has {host_cores}"));
        group.add(&cores);
        let wanted = machine.ram_gib.into();
        let field = ram_field(&context, &machine);
        // What it has stays, so a setting that no longer fits isn't changed behind the user's back.
        let ram =
            number_row("RAM", machine.ram_gib.into(), field.lower as f64, field.upper.max(wanted) as f64);
        // The first sentence is the figure, and the reasons behind it go behind the info button.
        match field.subtitle.split_once(". ") {
            Some((figure, reasons)) => {
                ram.set_subtitle(figure);
                explain(&ram, reasons);
            }
            None => ram.set_subtitle(field.subtitle.trim_end_matches('.')),
        }
        group.add(&ram);
        let cpus = vcache_cpus(&context.cpus);
        let pin = adw::SwitchRow::builder().title("V-Cache Pinning").active(machine.vcache_pin).build();
        pin.set_subtitle(&if cpus.is_empty() {
            "This CPU has no V-Cache cores".to_owned()
        } else {
            format!("On CPUs {}", cpu_list(&cpus))
        });
        explain(&pin, PIN_NOTE);
        advanced.add(&pin);
        let vram =
            number_row(VRAM_TITLE, machine.vram_gib.into(), 0.0, max_vram_gib_for(&context, &machine).into());
        vram.set_subtitle("In GiB, 0 for none");
        explain(&vram, VRAM_NOTE);
        // Only Windows' GPU-PV has a cap. The row is still made, as the card's row sets its top.
        if machine.kind == Kind::Windows {
            advanced.add(&vram);
        }
        // Only a GPU machine has ever had sound, and macOS and Android guests count as one. The Overview
        // adds it after the disk.
        let sound = adw::SwitchRow::builder()
            .title("Sound")
            .subtitle(SOUND_SHORT)
            .active(machine.sound)
            .visible(machine.kind == Kind::Windows || machine.macos() || machine.android())
            .build();
        explain(&sound, SOUND_NOTE);
        let this = Self {
            group: group.clone(),
            advanced: advanced.clone(),
            cores,
            ram,
            vram,
            pin,
            pin_possible: !cpus.is_empty(),
            sound,
        };
        this.connect(shared);
        this
    }

    fn connect(&self, shared: &Shared) {
        let saving = shared.clone();
        self.cores.connect_value_notify(move |row| {
            saving.machine.borrow_mut().cores = row.value() as u32;
            saving.save();
        });
        let saving = shared.clone();
        self.ram.connect_value_notify(move |row| {
            saving.machine.borrow_mut().ram_gib = row.value() as u32;
            saving.save();
        });
        let saving = shared.clone();
        self.vram.connect_value_notify(move |row| {
            saving.machine.borrow_mut().vram_gib = row.value() as u32;
            saving.save();
        });
        let saving = shared.clone();
        self.pin.connect_active_notify(move |row| {
            saving.machine.borrow_mut().vcache_pin = row.is_active();
            saving.save();
        });
        let saving = shared.clone();
        self.sound.connect_active_notify(move |row| {
            saving.machine.borrow_mut().sound = row.is_active();
            saving.save();
        });
    }

    pub fn set_status(&self, status: &Status) {
        let off = is_off(status);
        self.cores.set_sensitive(off);
        self.ram.set_sensitive(off);
        self.vram.set_sensitive(off);
        self.pin.set_sensitive(off && self.pin_possible);
        self.sound.set_sensitive(off);
        self.group.set_description((!off).then_some(VM_MUST_BE_OFF));
        self.advanced.set_description((!off).then_some(VM_MUST_BE_OFF));
    }
}
