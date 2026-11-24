//! First-time setup, a page at a time. All the work is in app-core, and runs on threads.

mod check_page;
mod done_page;
mod driver_page;

use crate::rows::{bottom_bar, info_button, page_heading};
use crate::window::Window;
use adw::prelude::*;
use guesthouse_app_core::setup::checks::CheckState;
use std::rc::Rc;

/// The last page isn't a step.
const STEPS: u32 = 2;
const SIZE: i32 = 640;

/// Words kept small in a title, as GNOME's header capitalization has them.
const SMALL_WORDS: [&str; 12] = ["a", "an", "and", "as", "at", "for", "in", "of", "on", "or", "the", "to"];

/// What every step has: its number and its name in large letters, rows to fill, and Next under them.
pub struct Step {
    page: adw::NavigationPage,
    next: gtk::Button,
    content: adw::PreferencesPage,
}

impl Step {
    /// `also` goes in the bottom bar before Next.
    fn new(title: &str, tag: &str, number: u32, also: Option<&gtk::Button>) -> Self {
        let next = gtk::Button::builder().label("Next").sensitive(false).build();
        next.add_css_class("suggested-action");
        let content = adw::PreferencesPage::new();
        content.set_vexpand(true);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&page_heading(&format!("Step {number} of {STEPS}"), title));
        column.append(&content);
        let toolbar = adw::ToolbarView::new();
        toolbar.set_content(Some(&column));
        toolbar.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
        let buttons: Vec<&gtk::Button> = also.into_iter().chain([&next]).collect();
        toolbar.add_bottom_bar(&bottom_bar(&buttons));
        let page = adw::NavigationPage::builder().title(title).tag(tag).child(&toolbar).build();
        Self { page, next, content }
    }
}

pub struct Wizard {
    window: Window,
    dialog: adw::Dialog,
    pages: adw::NavigationView,
}

pub fn show(window: &Window) {
    let pages = adw::NavigationView::new();
    let dialog = adw::Dialog::builder()
        .title("Set Up Guesthouse")
        .content_width(SIZE)
        .content_height(SIZE)
        .child(&pages)
        .build();
    let wizard = Rc::new(Wizard { window: window.clone(), dialog, pages });
    let driver = driver_page::DriverPage::new(&wizard);
    let check = check_page::CheckPage::new(&wizard, &driver);
    wizard.pages.add(check.page());
    let stopping = driver.clone();
    wizard.dialog.connect_closed(move |_| stopping.stop());
    wizard.dialog.present(Some(window.widget()));
}

/// A title as GNOME writes one, from the sentence-case titles the checks have: "Host NVIDIA driver"
/// becomes "Host NVIDIA Driver".
fn header_case(title: &str) -> String {
    let words = title.split(' ').enumerate().map(|(at, word)| {
        if at > 0 && SMALL_WORDS.contains(&word) {
            return word.to_owned();
        }
        let mut letters = word.chars();
        letters.next().map_or_else(String::new, |first| first.to_uppercase().chain(letters).collect())
    });
    words.collect::<Vec<_>>().join(" ")
}

/// A row saying how a check went, with what to do about it behind an info button. The words in there
/// can be selected, as a fix can be a command to copy.
pub fn check_row(title: &str, state: CheckState, text: &str, fix: &str) -> adw::ActionRow {
    let said = if text.is_empty() { fix } else { text };
    let row = adw::ActionRow::builder().title(header_case(title)).subtitle(said).use_markup(false).build();
    if !text.is_empty() && !fix.is_empty() {
        let button = info_button(fix);
        if let Some(label) = button.popover().and_then(|popover| popover.child()).and_downcast::<gtk::Label>()
        {
            label.set_selectable(true);
            // A selectable label starts with all of it selected.
            label.connect_map(|label| label.select_region(0, 0));
        }
        row.add_suffix(&button);
    }
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
