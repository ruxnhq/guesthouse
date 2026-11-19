//! The viewer's bar: a pill that floats over the top of the guest's screen, with the way back, what
//! the keyboard and mouse are doing, and a menu of everything else. It's also what the window is
//! dragged by and closed from, as the page has no header bar.

use crate::rows::{icon_button, name_for_readers};
use adw::prelude::*;
use gtk::gio;

const SPACING: i32 = 4;
/// Room round the pill for its shadow, and to keep it off the window's edge.
const MARGIN: i32 = 10;
const HINT_CHARS: i32 = 44;
/// The name the pill's actions go by in its menu.
const GROUP: &str = "screen";

pub struct Pill {
    bar: gtk::WindowHandle,
    hint: gtk::Label,
    pub capture: gtk::Button,
    pub fullscreen: gtk::Button,
    more: gtk::MenuButton,
    controls: gtk::WindowControls,
    /// The monitors fullscreen can go to, which the viewer lists.
    pub monitors: gio::Menu,
    actions: gio::SimpleActionGroup,
    overlay: gio::SimpleAction,
    hides: gio::SimpleAction,
    shut_down: gio::SimpleAction,
}

/// A button on the pill. One that took the focus from the screen would end the capture it starts.
fn pill_button(icon: &str, name: &str) -> gtk::Button {
    let button = icon_button(icon, name);
    button.set_focus_on_click(false);
    button
}

fn switch(name: &str, on: bool) -> gio::SimpleAction {
    gio::SimpleAction::new_stateful(name, None, &on.to_variant())
}

fn menu(monitors: &gio::Menu) -> gio::Menu {
    let switches = gio::Menu::new();
    switches.append(Some("Performance Overlay"), Some("screen.overlay"));
    switches.append(Some("Hide the Bar"), Some("screen.hides"));
    let machine = gio::Menu::new();
    machine.append(Some("Settings"), Some("screen.settings"));
    machine.append(Some("Send Report…"), Some("screen.report"));
    let power = gio::Menu::new();
    power.append(Some("Shut Down"), Some("screen.shut-down"));
    power.append(Some("Force Off"), Some("screen.force-off"));
    let menu = gio::Menu::new();
    menu.append_section(Some("Fullscreen On"), monitors);
    menu.append_section(None, &switches);
    menu.append_section(None, &machine);
    menu.append_section(None, &power);
    menu
}

impl Pill {
    pub fn new(machine_name: &str, overlay: bool, hides: bool) -> Self {
        let back = pill_button("go-previous-symbolic", "Back to the Library");
        back.set_action_name(Some("navigation.pop"));
        let name = gtk::Label::new(Some(machine_name));
        name.add_css_class("pill-name");
        let hint = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(HINT_CHARS)
            .build();
        hint.add_css_class("pill-hint");
        let capture = pill_button("input-mouse-symbolic", "Capture Mouse");
        let fullscreen = pill_button("view-fullscreen-symbolic", "Fullscreen");
        let monitors = gio::Menu::new();
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("More")
            .menu_model(&menu(&monitors))
            .focus_on_click(false)
            .build();
        more.add_css_class("flat");
        name_for_readers(&more, "More");
        let controls = gtk::WindowControls::new(gtk::PackType::End);
        let pill = gtk::Box::builder()
            .spacing(SPACING)
            .halign(gtk::Align::Center)
            .margin_top(MARGIN)
            .margin_bottom(MARGIN)
            .margin_start(MARGIN)
            .margin_end(MARGIN)
            .build();
        pill.add_css_class("viewer-pill");
        pill.append(&back);
        pill.append(&name);
        pill.append(&hint);
        pill.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        pill.append(&capture);
        pill.append(&fullscreen);
        pill.append(&more);
        pill.append(&controls);
        let bar = gtk::WindowHandle::builder().child(&pill).build();
        let actions = gio::SimpleActionGroup::new();
        let overlay = switch("overlay", overlay);
        let hides = switch("hides", hides);
        let shut_down = gio::SimpleAction::new("shut-down", None);
        for action in [&overlay, &hides, &shut_down] {
            actions.add_action(action);
        }
        bar.insert_action_group(GROUP, Some(&actions));
        Self { bar, hint, capture, fullscreen, more, controls, monitors, actions, overlay, hides, shut_down }
    }

    pub fn widget(&self) -> &gtk::WindowHandle {
        &self.bar
    }

    /// Runs this when the menu's item of this name is chosen.
    pub fn connect(&self, name: &str, run: impl Fn() + 'static) {
        let action = match self.actions.lookup_action(name).and_downcast::<gio::SimpleAction>() {
            Some(action) => action,
            None => {
                let action = gio::SimpleAction::new(name, None);
                self.actions.add_action(&action);
                action
            }
        };
        action.connect_activate(move |_, _| run());
    }

    pub fn hint(&self) -> String {
        self.hint.label().into()
    }

    pub fn set_hint(&self, hint: &str) {
        self.hint.set_label(hint);
    }

    pub fn set_overlay(&self, on: bool) {
        self.overlay.set_state(&on.to_variant());
    }

    pub fn set_hides(&self, hides: bool) {
        self.hides.set_state(&hides.to_variant());
    }

    pub fn allow_shut_down(&self, allowed: bool) {
        self.shut_down.set_enabled(allowed);
    }

    /// The window's own buttons have no place in fullscreen, and the fullscreen button leads out.
    pub fn set_fullscreen(&self, fullscreen: bool, tooltip: &str) {
        let icon = if fullscreen { "view-restore-symbolic" } else { "view-fullscreen-symbolic" };
        self.fullscreen.set_icon_name(icon);
        self.fullscreen.set_tooltip_text(Some(tooltip));
        self.controls.set_visible(!fullscreen);
    }

    /// Whether the menu is open, when the pointer is in it and not gone.
    pub fn menu_open(&self) -> bool {
        self.more.popover().is_some_and(|menu| menu.is_visible())
    }
}
