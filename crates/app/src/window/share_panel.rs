//! How the card is shared, on the library page between the chosen machine and the rest: each card
//! with a machine running on it, the whole card first and then each machine's part. It shows only
//! while such a machine runs. The rows are made again only when the cards or machines change, so a
//! row someone is reading doesn't go from under them each second.

use adw::prelude::*;
use guesthouse_app_core::card_share::look::CardShare;
use guesthouse_app_core::card_share::words::{card_note, machine_line, whole_line};

const SPACING: i32 = 12;
const TITLE: &str = "Sharing the Card";

/// One card's group and its rows, the whole card's first.
struct Shown {
    group: adw::PreferencesGroup,
    rows: Vec<adw::ActionRow>,
}

pub struct SharePanel {
    root: gtk::Box,
    groups: gtk::Box,
    shown: std::cell::RefCell<(Vec<String>, Vec<Shown>)>,
}

fn row(title: &str) -> adw::ActionRow {
    adw::ActionRow::builder().title(title).subtitle_selectable(true).build()
}

/// What decides the rows: each card's name, and the names of the machines on it.
fn layout(cards: &[CardShare]) -> Vec<String> {
    let names =
        |card: &CardShare| card.machines.iter().map(|machine| machine.name.clone()).collect::<Vec<_>>();
    cards.iter().map(|card| format!("{}\n{}", card.name, names(card).join("\n"))).collect()
}

fn group_of(card: &CardShare) -> Shown {
    let group = adw::PreferencesGroup::builder().title(&card.name).build();
    if let Some(note) = card_note(card.card.maker) {
        group.set_description(Some(note));
    }
    let mut rows = vec![row("Whole card")];
    rows.extend(card.machines.iter().map(|machine| row(&machine.name)));
    for found in &rows {
        group.add(found);
    }
    Shown { group, rows }
}

impl SharePanel {
    pub fn new() -> Self {
        let label = gtk::Label::builder().label(TITLE).xalign(0.0).build();
        label.add_css_class("section-label");
        let groups = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(SPACING).build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(SPACING)
            .visible(false)
            .build();
        root.append(&label);
        root.append(&groups);
        Self { root, groups, shown: Default::default() }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows the newest look. No card means no machine with a GPU is running, and the panel goes.
    pub fn show(&self, cards: &[CardShare]) {
        let mut shown = self.shown.borrow_mut();
        let wanted = layout(cards);
        if shown.0 != wanted {
            for old in shown.1.drain(..) {
                self.groups.remove(&old.group);
            }
            shown.1 = cards.iter().map(group_of).collect();
            for new in &shown.1 {
                self.groups.append(&new.group);
            }
            shown.0 = wanted;
        }
        for (card, found) in cards.iter().zip(&shown.1) {
            let lines =
                std::iter::once(whole_line(&card.whole)).chain(card.machines.iter().map(machine_line));
            for (line, row) in lines.zip(&found.rows) {
                row.set_subtitle(&line);
            }
        }
        self.root.set_visible(!cards.is_empty());
    }
}
