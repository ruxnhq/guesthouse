//! What a recipe will run, shown in full in a dialog of its own, with the script as it is and room to
//! scroll. It is the one place the user reads a recipe before agreeing to it.

use adw::prelude::*;

const TEXT_WIDTH: i32 = 480;
const TEXT_LEAST_HEIGHT: i32 = 160;
const TEXT_MOST_HEIGHT: i32 = 360;

fn dialog(heading: &str, text: &str) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::new(Some(heading), None);
    let label = gtk::Label::builder().label(text).xalign(0.0).selectable(true).wrap(true).build();
    label.add_css_class("monospace");
    let scroll = gtk::ScrolledWindow::builder()
        .child(&label)
        .min_content_width(TEXT_WIDTH)
        .min_content_height(TEXT_LEAST_HEIGHT)
        .max_content_height(TEXT_MOST_HEIGHT)
        .propagate_natural_height(true)
        .build();
    scroll.add_css_class("card");
    dialog.set_extra_child(Some(&scroll));
    dialog
}

/// Shows the recipe, and nothing else is asked.
pub fn show(parent: &impl IsA<gtk::Widget>, heading: &str, text: &str) {
    let dialog = dialog(heading, text);
    dialog.add_response("close", "Close");
    dialog.set_close_response("close");
    dialog.present(Some(parent));
}

/// Shows the recipe and asks whether to go on with it. `answered` is called only for a yes.
pub fn ask(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    text: &str,
    yes: &str,
    answered: impl Fn() + 'static,
) {
    let dialog = dialog(heading, text);
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("yes", yes);
    dialog.set_response_appearance("yes", adw::ResponseAppearance::Suggested);
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        if response == "yes" {
            answered();
        }
    });
    dialog.present(Some(parent));
}
