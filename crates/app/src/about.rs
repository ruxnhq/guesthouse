//! What the app is, its version, and where to find the people behind it.

use crate::application::APP_ID;
use crate::window::Window;
use adw::prelude::*;
use gtk::gio;
use guesthouse_app_core::updates::VERSION;

const WEBSITE: &str = "https://github.com/ruxnhq/guesthouse";
const ISSUES: &str = "https://github.com/ruxnhq/guesthouse/issues";
const DISCORD: &str = "https://discord.gg/rN3VARayZV";
const WHAT_IT_IS: &str = "Runs VMs that use this PC's GPU while the PC keeps using it too: Windows, Linux, Android and \
                          macOS, on NVIDIA, AMD and Intel cards. Some of these are still previews.";
const LICENCE: &str = "Free software, under the GNU General Public License, version 2 or later.";
/// The names the app says, as the README and THIRD-PARTY.md give them in full.
const MARKS: &str = "Windows, macOS, Android and the cards' names are trademarks of their owners. Guesthouse isn't \
                     made or endorsed by Microsoft, Apple, Google, NVIDIA, AMD or Intel.";
const LINK_SPACING: i32 = 8;
const WIDTH: i32 = 500;
const ICON_SIZE: i32 = 56;
const SPACING: i32 = 12;
const MARGIN: i32 = 20;

/// A button that opens a page in the browser, with the address as its tooltip.
fn link_button(window: &Window, title: &str, icon: &str, address: &'static str) -> gtk::Button {
    let content = adw::ButtonContent::builder().label(title).icon_name(icon).build();
    let button = gtk::Button::builder().child(&content).tooltip_text(address).build();
    button.add_css_class("pill");
    let parent = window.widget().clone();
    button.connect_clicked(move |_| {
        gtk::UriLauncher::new(address).launch(Some(&parent), gio::Cancellable::NONE, |_| {});
    });
    button
}

fn centred(text: &str, style: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).wrap(true).justify(gtk::Justification::Center).build();
    label.add_css_class(style);
    label
}

pub fn show(window: &Window) {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(SPACING)
        .margin_top(MARGIN)
        .margin_bottom(MARGIN)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .build();
    column.append(&gtk::Image::builder().icon_name(APP_ID).pixel_size(ICON_SIZE).build());
    column.append(&centred("Guesthouse", "title-1"));
    column.append(&centred(VERSION, "dim-label"));
    column.append(&centred(WHAT_IT_IS, "body"));
    let links = gtk::Box::builder().spacing(LINK_SPACING).halign(gtk::Align::Center).build();
    links.append(&link_button(window, "Website", "web-browser-symbolic", WEBSITE));
    links.append(&link_button(window, "Report a Problem", "dialog-warning-symbolic", ISSUES));
    links.append(&link_button(window, "Discord", "user-available-symbolic", DISCORD));
    column.append(&links);
    let licence = centred(LICENCE, "caption");
    licence.add_css_class("dim-label");
    column.append(&licence);
    let marks = centred(MARKS, "caption");
    marks.add_css_class("dim-label");
    column.append(&marks);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    view.set_content(Some(&column));
    let dialog = adw::Dialog::builder().title("About Guesthouse").child(&view).content_width(WIDTH).build();
    dialog.present(Some(window.widget()));
}
