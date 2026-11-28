//! The viewer's bar, with the way back, what the keyboard and mouse are doing, and a menu of
//! everything else. While it stays, in a window, it's a header bar in the desktop's look, and the
//! guest's screen starts below it. While it hides, as in fullscreen, it's a pill that floats over the
//! top of the screen when called. The same buttons move from one to the other. Either is what the
//! window is dragged by and closed from, as the page has no other bar.

use crate::rows::{icon_button, name_for_readers};
use adw::prelude::*;
use gtk::gio;
use std::cell::Cell;

const SPACING: i32 = 4;
/// Room round the pill for its shadow, and to keep it off the window's edge.
const MARGIN: i32 = 10;
const HINT_CHARS: i32 = 44;
/// The name the bar's actions go by in its menu.
const GROUP: &str = "screen";

pub struct Pill {
    /// What the page's toolbar holds: the header bar and the pill, one of them shown.
    top: gtk::Box,
    header: adw::HeaderBar,
    title: adw::WindowTitle,
    bar: gtk::WindowHandle,
    pill: gtk::Box,
    back: gtk::Button,
    /// Where the pill's buttons start, after what it says.
    separator: gtk::Separator,
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
    /// Whether the buttons are on the header bar.
    docked: Cell<bool>,
}

/// A button on the pill. One that took the focus from the screen would end the capture it starts.
fn pill_button(icon: &str, name: &str) -> gtk::Button {
    let button = icon_button(icon, name);
    button.set_focus_on_click(false);
    button
}

/// The window's buttons as the desktop lays them out, like `icon:minimize,maximize,close`.
fn desktop_layout() -> String {
    gtk::Settings::default()
        .and_then(|settings| settings.gtk_decoration_layout())
        .map(String::from)
        .unwrap_or_default()
}

/// A layout of the window's buttons with the one that maximises left out, on either side.
fn without_maximize(layout: &str) -> String {
    let side = |buttons: &str| {
        buttons.split(',').filter(|button| button.trim() != "maximize").collect::<Vec<_>>().join(",")
    };
    layout.split(':').map(side).collect::<Vec<_>>().join(":")
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
            // The same width whatever it says, so the buttons after it don't move under the pointer.
            .width_chars(HINT_CHARS)
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
        let separator = gtk::Separator::new(gtk::Orientation::Vertical);
        pill.append(&separator);
        pill.append(&capture);
        pill.append(&fullscreen);
        pill.append(&more);
        pill.append(&controls);
        let bar = gtk::WindowHandle::builder().child(&pill).build();
        let title = adw::WindowTitle::new(machine_name, "");
        // Its own way back is left out, as the pill's moves onto it.
        let header =
            adw::HeaderBar::builder().title_widget(&title).show_back_button(false).visible(false).build();
        let top = gtk::Box::new(gtk::Orientation::Vertical, 0);
        top.append(&header);
        top.append(&bar);
        let actions = gio::SimpleActionGroup::new();
        let overlay = switch("overlay", overlay);
        let hides = switch("hides", hides);
        let shut_down = gio::SimpleAction::new("shut-down", None);
        for action in [&overlay, &hides, &shut_down] {
            actions.add_action(action);
        }
        // On what holds both, as the menu moves between them.
        top.insert_action_group(GROUP, Some(&actions));
        Self {
            top,
            header,
            title,
            bar,
            pill,
            back,
            separator,
            hint,
            capture,
            fullscreen,
            more,
            controls,
            monitors,
            actions,
            overlay,
            hides,
            shut_down,
            docked: Cell::new(false),
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.top
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
        self.title.set_subtitle(hint);
    }

    pub fn set_overlay(&self, on: bool) {
        self.overlay.set_state(&on.to_variant());
    }

    /// The header bar while the bar stays, and the pill while it hides. The way back goes at the
    /// header bar's start, and the rest at its end, the menu by the window's buttons as on the pill.
    pub fn set_docked(&self, docked: bool) {
        if self.docked.replace(docked) == docked {
            return;
        }
        let (back, capture, fullscreen, more) = (&self.back, &self.capture, &self.fullscreen, &self.more);
        if docked {
            self.pill.remove(back);
            self.pill.remove(capture);
            self.pill.remove(fullscreen);
            self.pill.remove(more);
            self.header.pack_start(back);
            self.header.pack_end(more);
            self.header.pack_end(fullscreen);
            self.header.pack_end(capture);
        } else {
            self.header.remove(back);
            self.header.remove(capture);
            self.header.remove(fullscreen);
            self.header.remove(more);
            self.pill.prepend(back);
            self.pill.insert_child_after(capture, Some(&self.separator));
            self.pill.insert_child_after(fullscreen, Some(capture));
            self.pill.insert_child_after(more, Some(fullscreen));
        }
        self.header.set_visible(docked);
        self.bar.set_visible(!docked);
    }

    pub fn set_hides(&self, hides: bool) {
        self.hides.set_state(&hides.to_variant());
    }

    pub fn allow_shut_down(&self, allowed: bool) {
        self.shut_down.set_enabled(allowed);
    }

    /// The fullscreen button leads out. The window's own buttons stay, so the window can be
    /// minimised without leaving fullscreen, as it could before the pill, all but the one that
    /// maximises, which a window in fullscreen can't be.
    ///
    /// That button coming back also makes the bar wider, and KWin needs that. It works out whether
    /// a window can be maximised only when the window's smallest size changes, says no while it's
    /// in fullscreen, and doesn't ask again when it leaves, so the button stayed greyed out.
    pub fn set_fullscreen(&self, fullscreen: bool, tooltip: &str) {
        let icon = if fullscreen { "view-restore-symbolic" } else { "view-fullscreen-symbolic" };
        self.fullscreen.set_icon_name(icon);
        self.fullscreen.set_tooltip_text(Some(tooltip));
        // With no layout of the desktop's to go by, GTK's own is left as it is.
        let layout = desktop_layout();
        let fullscreen_layout = (fullscreen && !layout.is_empty()).then(|| without_maximize(&layout));
        self.controls.set_decoration_layout(fullscreen_layout.as_deref());
    }

    /// Whether the menu is open, when the pointer is in it and not gone.
    pub fn menu_open(&self) -> bool {
        self.more.popover().is_some_and(|menu| menu.is_visible())
    }
}

#[cfg(test)]
mod tests {
    use super::without_maximize;

    #[test]
    fn fullscreen_leaves_out_only_the_button_that_maximises() {
        assert_eq!(without_maximize("icon:minimize,maximize,close"), "icon:minimize,close");
        assert_eq!(without_maximize("close,maximize,minimize:menu"), "close,minimize:menu");
        assert_eq!(without_maximize("appmenu:close"), "appmenu:close");
        assert_eq!(without_maximize(":maximize"), ":");
    }
}
