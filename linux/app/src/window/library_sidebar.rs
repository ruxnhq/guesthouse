//! The library as a sidebar of machines beside the chosen one, for the looks that are the app's own.
//! It fits a window half the screen wide, and at narrower widths the sidebar folds away behind a
//! button and comes over the content when called.
//!
//! When the window has no bar of its own, as in Tide on a tiling compositor, the app's name, its
//! menu and New Machine go at the top of the sidebar instead.

use crate::rows::name_for_readers;
use adw::prelude::*;
use gtk::gio;

/// Below this width the sidebar folds away.
const FOLD_BELOW: f64 = 560.0;
const SIDEBAR_WIDTH: f64 = 236.0;
/// The share of the window the sidebar takes when the window is wide.
const SIDEBAR_FRACTION: f64 = 0.28;
/// The smallest the library can be drawn at, which a breakpoint needs to know.
const SMALLEST: (i32, i32) = (300, 240);
const SPACING: i32 = 8;
const CONTENT_MARGIN: i32 = 14;
const CONTENT_SPACING: i32 = 12;

pub struct Sidebar {
    /// What goes in the page: the split view, in what lets it fold.
    pub root: adw::BreakpointBin,
    pub list: gtk::ListBox,
    /// The app's name and menu, and New Machine, for a window without a bar.
    top: gtk::Box,
    pub add: gtk::Button,
    /// Where the hero and what goes under it are put.
    pub column: gtk::Box,
}

impl Sidebar {
    pub fn new(brand: &gtk::Box, menu: &gio::Menu) -> Self {
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
        list.add_css_class("machine-list");
        name_for_readers(&list, "Machines");
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .menu_model(menu)
            .halign(gtk::Align::End)
            .hexpand(true)
            .build();
        menu_button.add_css_class("flat");
        name_for_readers(&menu_button, "Main Menu");
        let heading = gtk::Box::builder().spacing(SPACING).build();
        heading.append(brand);
        heading.append(&menu_button);
        let add = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label("New Machine").build())
            .build();
        let top = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(SPACING).build();
        top.append(&heading);
        top.append(&add);
        let machines = gtk::Label::builder().label("Machines").xalign(0.0).margin_top(SPACING).build();
        machines.add_css_class("section-label");
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let sidebar = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(SPACING).build();
        sidebar.add_css_class("library-sidebar");
        sidebar.append(&top);
        sidebar.append(&machines);
        sidebar.append(&scroller);
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(CONTENT_SPACING)
            .margin_top(CONTENT_MARGIN)
            .margin_bottom(CONTENT_MARGIN)
            .margin_start(CONTENT_MARGIN)
            .margin_end(CONTENT_MARGIN)
            .build();
        let show_machines = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Machines")
            .halign(gtk::Align::Start)
            .margin_top(CONTENT_MARGIN)
            .margin_start(CONTENT_MARGIN)
            .visible(false)
            .build();
        show_machines.add_css_class("flat");
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        content.add_css_class("library-content");
        content.append(&show_machines);
        content.append(&gtk::ScrolledWindow::builder().child(&column).vexpand(true).build());
        let split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar)
            .content(&content)
            .min_sidebar_width(SIDEBAR_WIDTH)
            .max_sidebar_width(SIDEBAR_WIDTH)
            .sidebar_width_fraction(SIDEBAR_FRACTION)
            .build();
        split.bind_property("show-sidebar", &show_machines, "active").bidirectional().sync_create().build();
        split.bind_property("collapsed", &show_machines, "visible").sync_create().build();
        let root = adw::BreakpointBin::builder()
            .child(&split)
            .width_request(SMALLEST.0)
            .height_request(SMALLEST.1)
            .build();
        let fold = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            FOLD_BELOW,
            adw::LengthUnit::Sp,
        ));
        fold.add_setter(&split, "collapsed", Some(&true.to_value()));
        fold.add_setter(&content, "css-classes", Some(&["library-content", "alone"].to_value()));
        root.add_breakpoint(fold);
        let folded = split.clone();
        // Folded, choosing a machine shows it.
        list.connect_row_activated(move |_, _| {
            if folded.is_collapsed() {
                folded.set_show_sidebar(false);
            }
        });
        Self { root, list, top, add, column }
    }

    /// Whether the sidebar carries the app's name, menu and New Machine, as when the window has no
    /// bar to put them in.
    pub fn set_top_shown(&self, shown: bool) {
        self.top.set_visible(shown);
    }

    /// Selects the card's row, as the hero shows its machine.
    pub fn mark_chosen(&self, chosen: Option<&gtk::Box>) {
        let row = chosen.and_then(|card| card.parent()).and_downcast::<gtk::ListBoxRow>();
        self.list.select_row(row.as_ref());
    }
}
