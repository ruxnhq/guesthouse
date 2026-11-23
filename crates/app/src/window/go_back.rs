//! A start that failed after Guesthouse took a snapshot before it: the failure is told with a way back
//! to that snapshot. Going back is only ever the user's choice, and it asks first.

use super::Window;
use crate::rows::confirm;
use gtk::glib;
use guesthouse_app_core::auto_snapshot::RestorePoint;
use guesthouse_app_core::log::write_log;
use guesthouse_app_core::machine::Machine;

const GO_BACK_SECONDS: u32 = 15;

impl Window {
    /// The same as [`Window::problem`], with a button for going back to the snapshot taken before the
    /// start. The toast has room for one button, so the log stays in the app's menu, as Show Log.
    pub fn start_failed(&self, text: &str, machine: &Machine, point: &RestorePoint) {
        let _ = write_log(&self.0.context, &format!("app: {text}"));
        let toast = adw::Toast::builder()
            .title(glib::markup_escape_text(text).as_str())
            .timeout(GO_BACK_SECONDS)
            .button_label("Go Back…")
            .build();
        let (this, machine, point) = (self.clone(), machine.clone(), point.clone());
        toast.connect_button_clicked(move |_| this.ask_to_go_back(&machine, &point));
        self.0.toasts.add_toast(toast);
    }

    fn ask_to_go_back(&self, machine: &Machine, point: &RestorePoint) {
        let taken = if point.automatic {
            "Guesthouse took this snapshot just before it."
        } else {
            "It started from this snapshot, which it had just gone back to."
        };
        let body = format!(
            "{} did not start after a change. {taken} Going back undoes everything done on the machine \
             since: the change, and anything else. Other snapshots stay. This cannot be undone.",
            machine.name
        );
        let (this, machine, tag) = (self.clone(), machine.clone(), point.tag.clone());
        confirm(self.widget(), &format!("Go Back to {}?", point.name), &body, "Go Back", move || {
            this.launch("go_back", &machine, std::slice::from_ref(&tag), None);
        });
    }
}
