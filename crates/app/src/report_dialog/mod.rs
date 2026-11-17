//! Send Report: the tester picks the machine and the kind of problem, says what went wrong, sees what
//! the report holds, and sends it or saves it. Nothing is sent until they press Send.

mod after;

use crate::common::in_background;
use crate::rows::choice_row;
use crate::window::Window;
use adw::prelude::*;
use guesthouse_app_core::machine::{Machine, load_all};
use guesthouse_app_core::report::endpoint::KEPT_DAYS;
use guesthouse_app_core::report::send::send;
use guesthouse_app_core::report::{
    Collection, ProblemKind, Request, Words, collect, latest_machine, new_issue_link, write_in_reports_folder,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const WIDTH: i32 = 560;
const HEIGHT: i32 = 760;
const MARGIN: i32 = 12;
const DESCRIPTION_HEIGHT: i32 = 120;
const NO_MACHINE: &str = "None, it's about the app";
const GATHERING: &str = "Gathering what goes in…";

fn what_is_sent() -> String {
    format!(
        "A report holds the app's log, the chosen machine's logs from its last runs and its settings, and \
         facts about this PC: its system, GPU, driver and the checks Guesthouse makes. It goes to \
         Guesthouse's developer, is read by the developer and their automation tools, and is deleted after \
         {KEPT_DAYS} days. Your user name, host name and home folder are taken out where they're found, but \
         logs are free text, so something personal can still be in them. Nothing is sent until you press \
         Send."
    )
}

struct Parts {
    window: Window,
    dialog: adw::Dialog,
    machines: Vec<Machine>,
    machine: adw::ComboRow,
    kind: adw::ComboRow,
    guest_serial: adw::SwitchRow,
    description: gtk::TextView,
    contact: adw::EntryRow,
    contents: gtk::Label,
    send: gtk::Button,
    save: gtk::Button,
    collected: RefCell<Option<Collection>>,
    /// Each new look gets a number, so an older one that ends late is ignored.
    looks: Cell<u32>,
}

#[derive(Clone)]
struct ReportDialog(Rc<Parts>);

pub fn show(window: &Window) {
    let dialog = ReportDialog::new(window);
    dialog.0.dialog.present(Some(window.widget()));
    dialog.look();
}

impl ReportDialog {
    fn new(window: &Window) -> Self {
        let context = window.context();
        let (machines, _) = load_all(&context);
        let latest = latest_machine(&context).map(|machine| machine.id);
        let mut names: Vec<&str> = machines.iter().map(|machine| machine.name.as_str()).collect();
        names.push(NO_MACHINE);
        let machine = choice_row("Machine", &names);
        let chosen = machines.iter().position(|machine| Some(&machine.id) == latest.as_ref());
        machine.set_selected(chosen.unwrap_or(machines.len()) as u32);
        let kinds: Vec<&str> = ProblemKind::ALL.iter().map(|kind| kind.label()).collect();
        let kind = choice_row("Kind of problem", &kinds);
        kind.set_selected(ProblemKind::ALL.len() as u32 - 1);
        let guest_serial = adw::SwitchRow::builder()
            .title("The guest's serial log")
            .subtitle("What Windows or Linux wrote to its serial port. The app's log can still hold a few of the guest's answers to its own scripts.")
            .active(true)
            .build();
        let choices = adw::PreferencesGroup::new();
        choices.add(&machine);
        choices.add(&kind);
        choices.add(&guest_serial);

        let description = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(MARGIN)
            .bottom_margin(MARGIN)
            .left_margin(MARGIN)
            .right_margin(MARGIN)
            .height_request(DESCRIPTION_HEIGHT)
            .accepts_tab(false)
            .build();
        description.update_property(&[gtk::accessible::Property::Label("What went wrong")]);
        let frame = gtk::Frame::builder().child(&description).build();
        let words = adw::PreferencesGroup::builder()
            .title("What went wrong")
            .description("What you did, what you expected, and what happened instead.")
            .build();
        words.add(&frame);
        let contact = adw::EntryRow::builder().title("How to reach you, if you'd like (optional)").build();
        let contact_group = adw::PreferencesGroup::new();
        contact_group.add(&contact);

        let contents = gtk::Label::builder().label(GATHERING).wrap(true).xalign(0.0).selectable(true).build();
        contents.add_css_class("monospace");
        let held =
            adw::PreferencesGroup::builder().title("What it holds").description(what_is_sent()).build();
        held.add(&contents);

        let page = adw::PreferencesPage::new();
        for group in [&choices, &words, &contact_group, &held] {
            page.add(group);
        }
        let send = gtk::Button::builder().label("Send").sensitive(false).build();
        send.add_css_class("suggested-action");
        let save = gtk::Button::builder().label("Save to File…").sensitive(false).build();
        let cancel = gtk::Button::with_label("Cancel");
        let header =
            adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
        header.pack_start(&cancel);
        header.pack_end(&send);
        header.pack_end(&save);
        let toolbar = adw::ToolbarView::new();
        toolbar.set_content(Some(&page));
        toolbar.add_top_bar(&header);
        let dialog = adw::Dialog::builder()
            .title("Send Report")
            .content_width(WIDTH)
            .content_height(HEIGHT)
            .child(&toolbar)
            .build();
        let this = Self(Rc::new(Parts {
            window: window.clone(),
            dialog,
            machines,
            machine,
            kind,
            guest_serial,
            description,
            contact,
            contents,
            send,
            save,
            collected: RefCell::default(),
            looks: Cell::new(0),
        }));
        this.connect(&cancel);
        this
    }

    fn connect(&self, cancel: &gtk::Button) {
        let parts = &self.0;
        let dialog = parts.dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
        let this = self.clone();
        parts.machine.connect_selected_notify(move |_| this.look());
        let this = self.clone();
        parts.kind.connect_selected_notify(move |_| this.look());
        let this = self.clone();
        parts.guest_serial.connect_active_notify(move |_| this.look());
        let this = self.clone();
        parts.send.connect_clicked(move |_| this.on_send());
        let this = self.clone();
        parts.save.connect_clicked(move |_| this.on_save());
    }

    fn request(&self) -> Request {
        let parts = &self.0;
        let kind = ProblemKind::ALL.get(parts.kind.selected() as usize).copied().unwrap_or_default();
        let app_facts = vec![
            (
                "GTK in the app".to_owned(),
                format!("{}.{}.{}", gtk::major_version(), gtk::minor_version(), gtk::micro_version()),
            ),
            (
                "libadwaita in the app".to_owned(),
                format!("{}.{}.{}", adw::major_version(), adw::minor_version(), adw::micro_version()),
            ),
        ];
        Request {
            machine: parts.machines.get(parts.machine.selected() as usize).cloned(),
            kind,
            guest_serial: parts.guest_serial.is_active(),
            app_facts,
        }
    }

    fn words(&self) -> Words {
        let buffer = self.0.description.buffer();
        let description = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        Words { description, contact: self.0.contact.text().to_string() }
    }

    fn set_busy(&self, busy: bool) {
        let ready = !busy && self.0.collected.borrow().is_some();
        self.0.send.set_sensitive(ready);
        self.0.save.set_sensitive(ready);
    }

    /// Gathers again for what's chosen now. It's only reading, so it can run as often as a choice changes.
    fn look(&self) {
        let number = self.0.looks.get() + 1;
        self.0.looks.set(number);
        *self.0.collected.borrow_mut() = None;
        self.0.contents.set_label(GATHERING);
        self.set_busy(true);
        let context = self.0.window.context();
        let reading = context.clone();
        let request = self.request();
        let this = self.clone();
        in_background(
            &context,
            move || Ok(collect(&reading, &request)),
            move |found| {
                if this.0.looks.get() != number {
                    return;
                }
                match found {
                    Ok(collection) => {
                        this.0.contents.set_label(&contents_text(&collection));
                        *this.0.collected.borrow_mut() = Some(collection);
                    }
                    Err(problem) => this.0.contents.set_label(&problem),
                }
                this.set_busy(false);
            },
        );
    }

    fn on_send(&self) {
        let Some(collection) = self.0.collected.borrow().clone() else { return };
        self.set_busy(true);
        self.0.send.set_label("Sending…");
        let context = self.0.window.context();
        let working = context.clone();
        let words = self.words();
        let this = self.clone();
        in_background(
            &context,
            move || {
                let bundle = write_in_reports_folder(&working, &collection, &words)?;
                let sent = send(&bundle).map_err(|failure| failure.0);
                let link =
                    sent.as_ref().ok().map(|id| new_issue_link(&working, id, &bundle, &words.description));
                Ok((bundle, sent, link))
            },
            move |outcome| {
                this.0.send.set_label("Send");
                this.set_busy(false);
                let window = &this.0.window;
                match outcome {
                    Ok((_, Ok(id), link)) => {
                        this.0.dialog.close();
                        after::sent(window, &id, &link.unwrap_or_default());
                    }
                    Ok((bundle, Err(reason), _)) => after::not_sent(window, &this.0.dialog, &reason, bundle),
                    Err(problem) => window.problem(&format!("The report couldn't be written: {problem}")),
                }
            },
        );
    }

    fn on_save(&self) {
        let Some(collection) = self.0.collected.borrow().clone() else { return };
        self.set_busy(true);
        let context = self.0.window.context();
        let working = context.clone();
        let words = self.words();
        let this = self.clone();
        in_background(
            &context,
            move || write_in_reports_folder(&working, &collection, &words),
            move |written| {
                this.set_busy(false);
                match written {
                    Ok(bundle) => after::save_as(&this.0.window, &this.0.dialog, bundle),
                    Err(problem) => {
                        this.0.window.problem(&format!("The report couldn't be written: {problem}"))
                    }
                }
            },
        );
    }
}

fn contents_text(collection: &Collection) -> String {
    let mut lines: Vec<String> = collection.names().map(str::to_owned).collect();
    lines.push("description.txt, summary.json, manifest.txt".to_owned());
    if !collection.missing.is_empty() {
        lines.push(String::new());
        lines.push("Not in it:".to_owned());
        for missing in &collection.missing {
            lines.push(format!("{}: {}", missing.name, missing.reason));
            if let Some(how) = &missing.how {
                lines.push(format!("  to get it yourself: {how}"));
            }
        }
    }
    if !collection.notes.is_empty() {
        lines.push(String::new());
        lines.extend(collection.notes.iter().cloned());
    }
    lines.join("\n")
}
