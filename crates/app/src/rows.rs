//! Small widgets the pages have in common.

use adw::prelude::*;
use guesthouse_app_core::setup::checks::CheckState;

/// A row saying how something went, and what to do about it. Commands in it can be copied.
pub fn state_row(title: &str, state: CheckState, text: &str, fix: &str) -> adw::ActionRow {
    let lines: Vec<&str> = [text, fix].into_iter().filter(|line| !line.is_empty()).collect();
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(lines.join("\n"))
        .use_markup(false)
        .subtitle_selectable(true)
        .build();
    let (name, style) = match state {
        CheckState::Ok => ("object-select-symbolic", "success"),
        CheckState::Warning => ("dialog-warning-symbolic", "warning"),
        CheckState::Failed => ("dialog-error-symbolic", "error"),
    };
    let icon = gtk::Image::from_icon_name(name);
    icon.add_css_class(style);
    row.add_prefix(&icon);
    row
}

/// A button that sits in a row or a group's header.
pub fn row_button(label: &str) -> gtk::Button {
    gtk::Button::builder().label(label).valign(gtk::Align::Center).build()
}

/// Gives a widget with no text of its own a name for screen readers.
pub fn name_for_readers(widget: &impl IsA<gtk::Widget>, name: &str) {
    widget.upcast_ref::<gtk::Widget>().update_property(&[gtk::accessible::Property::Label(name)]);
}

pub fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button =
        gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).valign(gtk::Align::Center).build();
    button.add_css_class("flat");
    name_for_readers(&button, tooltip);
    button
}

pub fn number_row(title: &str, value: f64, lowest: f64, highest: f64) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(lowest, highest, 1.0);
    row.set_title(title);
    row.set_value(value);
    // The row's field has the focus, so that's what a screen reader lands on.
    name_for_readers(&row, title);
    if let Ok(field) = row.delegate().and_dynamic_cast::<gtk::Widget>() {
        name_for_readers(&field, title);
    }
    row
}

pub fn choice_row(title: &str, names: &[&str]) -> adw::ComboRow {
    let row = adw::ComboRow::builder().title(title).build();
    row.set_model(Some(&gtk::StringList::new(names)));
    row
}

/// Rows to put inside a dialog.
pub fn boxed_rows(rows: &[&gtk::Widget]) -> gtk::ListBox {
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list.add_css_class("boxed-list");
    for row in rows {
        list.append(*row);
    }
    list
}

/// A question with a Cancel button, which calls `answered` only for a yes.
pub fn question(
    heading: &str,
    body: &str,
    yes: &str,
    appearance: adw::ResponseAppearance,
) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("yes", yes);
    dialog.set_response_appearance("yes", appearance);
    dialog.set_close_response("cancel");
    dialog
}

pub fn on_yes(dialog: &adw::AlertDialog, answered: impl Fn() + 'static) {
    dialog.connect_response(None, move |_, response| {
        if response == "yes" {
            answered();
        }
    });
}

/// Asks before something that can't be undone.
pub fn confirm(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    yes: &str,
    answered: impl Fn() + 'static,
) {
    let dialog = question(heading, body, yes, adw::ResponseAppearance::Destructive);
    on_yes(&dialog, answered);
    dialog.present(Some(parent));
}

/// As wide as a page of preferences, so what's put over or under one lines up with its rows.
const PAGE_WIDTH: i32 = 600;
const PAGE_MARGIN: i32 = 12;
const HEADING_SPACING: i32 = 4;
const MESSAGE_SPACING: i32 = 18;
const MESSAGE_ICON_SIZE: i32 = 112;
const MESSAGE_MARGIN: i32 = 24;
/// Wide enough that a sentence reads as a line or two, not a column.
const MESSAGE_CHARS: i32 = 66;

fn page_wide(child: &impl IsA<gtk::Widget>) -> adw::Clamp {
    adw::Clamp::builder()
        .child(child)
        .maximum_size(PAGE_WIDTH)
        .margin_start(PAGE_MARGIN)
        .margin_end(PAGE_MARGIN)
        .build()
}

/// A page's name in large letters, with a small line over it for where the page is: its step.
pub fn page_heading(over: &str, title: &str) -> adw::Clamp {
    let heading = gtk::Box::new(gtk::Orientation::Vertical, HEADING_SPACING);
    if !over.is_empty() {
        let over = gtk::Label::builder().label(over).xalign(0.0).build();
        over.add_css_class("section-label");
        heading.append(&over);
    }
    let title = gtk::Label::builder().label(title).xalign(0.0).wrap(true).build();
    title.add_css_class("page-title");
    heading.append(&title);
    let clamp = page_wide(&heading);
    clamp.set_margin_top(PAGE_MARGIN);
    clamp
}

/// What a page ends in: its buttons, at the right, under its rows.
pub fn bottom_bar(buttons: &[&gtk::Button]) -> adw::Clamp {
    let bar = gtk::Box::builder().spacing(PAGE_MARGIN).halign(gtk::Align::End).build();
    for button in buttons {
        bar.append(*button);
    }
    let clamp = page_wide(&bar);
    clamp.add_css_class("bottom-bar");
    clamp
}

/// A page that says one thing: an icon, a large title, a sentence, and what to do next. It's laid
/// out here and not left to libadwaita's status page, which squeezes its words into a narrow
/// column on the libadwaita that Ubuntu 24.04 has.
pub fn message_page(icon: &str, title: &str, words: &str, next: &gtk::Button) -> gtk::Box {
    let title = gtk::Label::builder().label(title).wrap(true).justify(gtk::Justification::Center).build();
    title.add_css_class("empty-title");
    let words = gtk::Label::builder()
        .label(words)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(MESSAGE_CHARS)
        .build();
    words.add_css_class("empty-words");
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(MESSAGE_SPACING)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .margin_top(MESSAGE_MARGIN)
        .margin_bottom(MESSAGE_MARGIN)
        .margin_start(MESSAGE_MARGIN)
        .margin_end(MESSAGE_MARGIN)
        .build();
    page.append(&gtk::Image::builder().icon_name(icon).pixel_size(MESSAGE_ICON_SIZE).build());
    page.append(&title);
    page.append(&words);
    page.append(next);
    page
}
