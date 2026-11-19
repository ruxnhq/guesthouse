//! First-time setup, a page at a time. All the work is in app-core, and runs on threads.

mod check_page;
mod done_page;
mod driver_page;

use crate::rows::{bottom_bar, page_heading};
use crate::window::Window;
use adw::prelude::*;
use std::rc::Rc;

/// The last page isn't a step.
const STEPS: u32 = 2;
const SIZE: i32 = 640;

/// What every step has: its number and its name in large letters, rows to fill, and Next under them.
pub struct Step {
    page: adw::NavigationPage,
    next: gtk::Button,
    content: adw::PreferencesPage,
}

impl Step {
    fn new(title: &str, tag: &str, number: u32) -> Self {
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
        toolbar.add_bottom_bar(&bottom_bar(&[&next]));
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
