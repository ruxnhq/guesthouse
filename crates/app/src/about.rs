//! What the app is, its version, and where to find the people behind it.

use crate::application::APP_ID;
use crate::rows::boxed_rows;
use crate::window::Window;
use adw::prelude::*;
use gtk::gio;
use guesthouse_app_core::updates::VERSION;

const WEBSITE: &str = "https://github.com/xnixy/guesthouse";
const ISSUES: &str = "https://github.com/xnixy/guesthouse/issues";
const DISCORD: &str = "https://discord.gg/rN3VARayZV";
const WHAT_IT_IS: &str = "Runs VMs on this PC. With its GPU part, Windows gets NVIDIA's own driver on the card the host keeps using.";
const LICENCE: &str = "Free software, under the GNU General Public License, version 2 or later.";
const WIDTH: i32 = 440;
const ICON_SIZE: i32 = 72;
const SPACING: i32 = 12;
const MARGIN: i32 = 20;

/// A row that opens a page in the browser.
fn link_row(window: &Window, title: &str, address: &'static str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle(address).activatable(true).build();
    row.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
    let parent = window.widget().clone();
    row.connect_activated(move |_| {
        gtk::UriLauncher::new(address).launch(Some(&parent), gio::Cancellable::NONE, |_| {});
    });
    row
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
    column.append(&centred(LICENCE, "caption"));
    let links = [
        link_row(window, "Website", WEBSITE),
        link_row(window, "Report a Problem", ISSUES),
        link_row(window, "Discord", DISCORD),
    ];
    let links: Vec<&gtk::Widget> = links.iter().map(|row| row.upcast_ref()).collect();
    column.append(&boxed_rows(&links));
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    view.set_content(Some(
        &gtk::ScrolledWindow::builder().child(&column).propagate_natural_height(true).build(),
    ));
    let dialog = adw::Dialog::builder().title("About Guesthouse").child(&view).content_width(WIDTH).build();
    dialog.present(Some(window.widget()));
}
