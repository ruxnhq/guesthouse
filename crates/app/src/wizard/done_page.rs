//! The end of setup, with the way to the first machine.

use super::Wizard;
use crate::application::APP_ID;
use crate::rows::message_page;
use adw::prelude::*;
use std::rc::Rc;

pub fn done_page(wizard: &Rc<Wizard>) -> adw::NavigationPage {
    let button = gtk::Button::builder().label("New Machine").halign(gtk::Align::Center).build();
    button.add_css_class("pill");
    button.add_css_class("suggested-action");
    let closing = wizard.clone();
    button.connect_clicked(move |_| {
        closing.dialog.close();
        closing.window.show_new_machine();
    });
    let status = message_page(
        APP_ID,
        "Guesthouse Is Ready",
        "Make your first machine, with Windows, Ubuntu, Android or another system.",
        &button,
    );
    let toolbar = adw::ToolbarView::new();
    toolbar.set_content(Some(&status));
    toolbar.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    adw::NavigationPage::builder().title("Done").tag("done").child(&toolbar).build()
}
