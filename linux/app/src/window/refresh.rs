//! Keeping the window true to what is running. The looking happens on a thread, as nothing here may
//! hold up the screen, and it runs each second.

use super::Window;
use crate::common::in_background;
use crate::notify;
use gtk::glib;
use guesthouse_app_core::actions::rebooted;
use guesthouse_app_core::app_windows::open_windows::{OpenProgram, open_in};
use guesthouse_app_core::card_share::look::{self as share, CardShare, Looks, Running};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::ending;
use guesthouse_app_core::machine::{Kind, MACHINE_FILE, Machine, load_all};
use guesthouse_app_core::process_stat::Seen;
use guesthouse_app_core::processes::all_leftovers;
use guesthouse_app_core::slots;
use guesthouse_app_core::stopped::note_if_stopped_by_itself;
use guesthouse_app_core::tasks::{State, Status, free_slot, is_paused, status_since};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_secs(1);
/// Seconds between looks at QEMU's run state and at what a VM left behind.
const CHECK_TICKS: u32 = 3;

#[derive(Default)]
pub struct RefreshState {
    reading: bool,
    ticks: u32,
    paused_before: BTreeSet<String>,
    /// The machines that were up at the last look, their slots and their QEMUs.
    running_before: BTreeMap<String, (u8, Seen)>,
    leftovers_before: BTreeMap<u8, u64>,
    problem: Option<String>,
    /// Whether the machines have been looked at once. What a machine already said before the app
    /// started is old news: its card shows it, and nothing announces it again.
    looked: bool,
    /// Each running machine's card clients at the last look, for the share panel's engine figures.
    share_looks: Looks,
    /// What the share panel's times count from.
    started: Option<Instant>,
    /// The window's count of its own machine saves when the look began.
    writes_at_look: u64,
}

struct Report {
    statuses: BTreeMap<String, Status>,
    paused: Vec<Machine>,
    left: BTreeMap<u8, u64>,
    rebooted: Vec<Machine>,
    checked: bool,
    saved: Vec<Machine>,
    vm_running: bool,
    /// Throwaway copies that had run and stopped, and were deleted as they were made to be.
    deleted: Vec<String>,
    share: Vec<CardShare>,
    share_looks: Looks,
    /// The programs with app windows open, of each running Windows machine that has any.
    open_windows: BTreeMap<String, Vec<OpenProgram>>,
}

/// The machines up now, with the processes they run in, for the share panel.
fn running_now(machines: &[Machine], statuses: &BTreeMap<String, Status>) -> Vec<Running> {
    let up = |machine: &Machine| {
        let found = statuses.get(&machine.id).filter(|found| found.state == State::Running)?;
        let qemu = found.qemu.map(|seen| seen.pid);
        Some(Running { machine: machine.clone(), slot: found.slot?, qemu })
    };
    machines.iter().filter_map(up).collect()
}

fn look(
    context: &Arc<Context>,
    machines: &[Machine],
    running_before: &BTreeMap<String, (u8, Seen)>,
    checked: bool,
    share_before: &Looks,
    started: Instant,
) -> Report {
    let status = |machine: &Machine| status_since(context, machine, running_before.get(&machine.id).copied());
    let statuses: BTreeMap<String, Status> =
        machines.iter().map(|machine| (machine.id.clone(), status(machine))).collect();
    let slot_of = |machine: &Machine| statuses[&machine.id].slot.map(slots::number);
    let is_idle_gpu = |machine: &&Machine| {
        let found = &statuses[&machine.id];
        found.state == State::Running && !found.busy && machine.has_gpu()
    };
    let running: Vec<&Machine> = machines.iter().filter(is_idle_gpu).collect();
    let paused = |machine: &&&Machine| {
        checked && context.run_state(slot_of(machine)).is_some_and(|state| is_paused(&state))
    };
    let restarted = |machine: &&&Machine| checked && rebooted(context, slot_of(machine));
    let idle = !statuses.values().any(|found| found.busy);
    // Taken here, just before the clients are read, as the looks above can take a while.
    let at = started.elapsed();
    let (share, share_looks) = share::look(context, &running_now(machines, &statuses), share_before, at);
    let open_windows = machines
        .iter()
        .filter(|machine| machine.kind == Kind::Windows)
        .filter_map(|machine| {
            let found = statuses.get(&machine.id).filter(|found| found.state == State::Running)?;
            let open = open_in(context, slots::number(found.slot?));
            (!open.is_empty()).then(|| (machine.id.clone(), open))
        })
        .collect();
    Report {
        open_windows,
        paused: running.iter().filter(paused).map(|machine| (**machine).clone()).collect(),
        rebooted: running.iter().filter(restarted).map(|machine| (**machine).clone()).collect(),
        left: if checked && idle && !machines.is_empty() { all_leftovers(context) } else { BTreeMap::new() },
        deleted: guesthouse_app_core::throwaway::delete_finished(context),
        saved: load_all(context).0,
        vm_running: free_slot(context).is_none(),
        checked,
        statuses,
        share,
        share_looks,
    }
}

pub fn start_ticking(window: &Window) {
    let window = window.clone();
    glib::timeout_add_local(TICK, move || {
        window.refresh();
        glib::ControlFlow::Continue
    });
}

impl Window {
    /// Looks at the VMs, unless a look is already under way.
    pub fn refresh(&self) {
        let mut state = self.0.refresh.borrow_mut();
        if state.reading {
            return;
        }
        state.reading = true;
        state.ticks += 1;
        let checked = state.ticks.is_multiple_of(CHECK_TICKS);
        let running_before = state.running_before.clone();
        let share_before = state.share_looks.clone();
        let started = *state.started.get_or_insert_with(Instant::now);
        state.writes_at_look = self.machine_writes();
        drop(state);
        let machines: Vec<Machine> =
            self.0.cards.borrow().values().map(|row| row.machine().borrow().clone()).collect();
        let context = self.context();
        let looking = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || Ok(look(&looking, &machines, &running_before, checked, &share_before, started)),
            move |result| match result {
                Ok(report) => this.show_status(report),
                Err(problem) => this.on_refresh_failed(problem),
            },
        );
    }

    /// This runs every second, so one failure must not become a toast a second.
    fn on_refresh_failed(&self, text: String) {
        let mut state = self.0.refresh.borrow_mut();
        state.reading = false;
        if state.problem.as_ref() != Some(&text) {
            state.problem = Some(text.clone());
            drop(state);
            self.problem(&text);
        }
    }

    /// What a task or `guesthouse-vm` saved, into the handles, so they show it. A look that began
    /// before one of the app's own saves read the files before it, and is left for the next.
    fn take_saved(&self, saved: &[Machine]) {
        let stale = self.0.refresh.borrow().writes_at_look != self.machine_writes();
        for machine in saved {
            // A task made it, as a copy is: it's only seen once it's whole, and it's still there.
            let made_elsewhere = !self.0.cards.borrow().contains_key(&machine.id)
                && machine.folder(&self.context()).join(MACHINE_FILE).exists();
            if made_elsewhere {
                self.add_machine(machine.clone());
            }
            if let Some(row) = self.0.cards.borrow().get(&machine.id).filter(|_| !stale) {
                let handle = row.machine();
                let caught_up = handle.borrow().caught_up(&row.known(), machine);
                if let Ok(caught_up) = caught_up {
                    *handle.borrow_mut() = caught_up;
                    row.set_known(machine.clone());
                }
            }
        }
    }

    /// What is only looked at now and then. Twice in a row counts, as something can stop the VM for a
    /// moment, or be about to tidy up.
    fn act_on_checks(&self, report: &Report) {
        let (paused_before, leftovers_before) = {
            let state = self.0.refresh.borrow();
            (state.paused_before.clone(), state.leftovers_before.clone())
        };
        for machine in report.paused.iter().filter(|machine| paused_before.contains(&machine.id)) {
            self.toast("The VM was left paused. Resuming it.", adw::ToastPriority::Normal);
            self.launch("resume", machine, &[], None);
        }
        let any_machine = self.0.cards.borrow().values().next().map(|row| row.machine());
        for (slot, file) in &report.left {
            if let Some(machine) = any_machine.as_ref().filter(|_| leftovers_before.get(slot) == Some(file)) {
                let machine = machine.borrow().clone();
                self.launch("clean_up", &machine, &[], Some(slots::number(*slot)));
            }
        }
        for machine in &report.rebooted {
            self.launch("after_reboot", machine, &[], None);
        }
        let mut state = self.0.refresh.borrow_mut();
        state.paused_before = report.paused.iter().map(|machine| machine.id.clone()).collect();
        state.leftovers_before.clone_from(&report.left);
    }

    fn show_machine_status(&self, id: &str, status: &Status, vm_running: bool, open: &[OpenProgram]) {
        let rows = self.0.cards.borrow();
        // A row can be gone if it was deleted while this was being read.
        let Some(row) = rows.get(id) else { return };
        let looked = self.0.refresh.borrow().looked;
        let before = row.status();
        if looked && !status.error.is_empty() && status.error != before.error {
            match &status.restore_point {
                Some(point) => self.start_failed(&status.error, &row.machine().borrow(), point),
                None => self.problem(&status.error),
            }
        }
        if looked
            && let Some((title, body)) = notify::of_change(&row.machine().borrow().name, &before, status)
        {
            notify::send(self, id, &title, &body);
        }
        row.set_app_windows(open);
        row.set_status(status, vm_running);
        if let Some(settings) = self.0.settings.borrow().as_ref().filter(|dialog| dialog.machine_id() == id) {
            settings.set_status(status);
        }
        let page = self.0.screen_page.borrow().clone();
        if let Some(page) = page.filter(|page| page.machine_id() == id) {
            if status.state == State::Off {
                self.0.pages.pop_to_tag("library");
            } else {
                page.show_hint(Some(status));
            }
        }
    }

    /// A VM can go at any moment, and the row showing Off doesn't say that it wasn't meant to. A
    /// machine seen running reads as off only once its QEMU has ended.
    fn say_what_stopped(&self, report: &Report) {
        let up = |(id, status): (&String, &Status)| {
            Some((id.clone(), status.slot.zip(status.qemu).filter(|_| status.state == State::Running)?))
        };
        let running: BTreeMap<String, (u8, Seen)> = report.statuses.iter().filter_map(up).collect();
        let before = std::mem::replace(&mut self.0.refresh.borrow_mut().running_before, running.clone());
        // QEMU says why it's shutting down only as it goes, so something listens from the start.
        for (id, (slot, _)) in &running {
            if !before.contains_key(id) {
                ending::watch(&self.0.context, *slot);
            }
        }
        for (id, (slot, _)) in before {
            let gone = report.statuses.get(&id).is_some_and(|found| found.state == State::Off && !found.busy);
            let machine = self.0.cards.borrow().get(&id).map(|row| row.machine().borrow().clone());
            if let Some(machine) = machine.filter(|_| gone) {
                note_if_stopped_by_itself(&self.0.context, &machine, slot);
            }
        }
    }

    /// The icon in the tray names the machines that are running.
    fn tell_the_tray(&self, report: &Report) {
        let tray = self.0.tray.borrow();
        let Some(tray) = tray.as_ref() else { return };
        let cards = self.0.cards.borrow();
        let is_running = |(id, status): (&String, &Status)| {
            (status.state == State::Running)
                .then(|| cards.get(id).map(|card| card.machine().borrow().name.clone()))?
        };
        tray.set_running(report.statuses.iter().filter_map(is_running).collect());
    }

    fn show_status(&self, mut report: Report) {
        {
            let mut state = self.0.refresh.borrow_mut();
            state.reading = false;
            state.problem = None;
            state.share_looks = std::mem::take(&mut report.share_looks);
        }
        self.0.library.share_panel().show(&report.share);
        self.0.vm_running.set(report.vm_running);
        if let Some(dialog) = self.0.new_machine.borrow().as_ref() {
            dialog.set_vm_running(report.vm_running);
        }
        for id in &report.deleted {
            self.on_removed(id);
        }
        self.take_saved(&report.saved);
        if report.checked {
            self.act_on_checks(&report);
        }
        self.say_what_stopped(&report);
        for (id, status) in &report.statuses {
            let open = report.open_windows.get(id).map_or(&[][..], Vec::as_slice);
            self.show_machine_status(id, status, report.vm_running, open);
        }
        self.0.refresh.borrow_mut().looked = true;
        self.tell_the_tray(&report);
    }
}
