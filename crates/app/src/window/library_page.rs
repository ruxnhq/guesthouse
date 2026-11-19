//! The first page: the machines, or a way to make the first one.

use super::Window;
use super::hero::Hero;
use crate::application::APP_ID;
use crate::rows::name_for_readers;
use adw::prelude::*;
use gtk::gio;
use guesthouse_app_core::updates::VERSION;

const COLUMN_SPACING: i32 = 20;
const COLUMN_MARGIN: i32 = 24;
const SIDE_MARGIN: i32 = 20;
const COLUMN_WIDTH: i32 = 1160;
const CARD_SPACING: u32 = 16;
/// As many cards side by side as a wide window has room for.
const CARDS_ACROSS: u32 = 3;
const BRAND_SPACING: i32 = 8;

pub struct LibraryPage {
    pub page: adw::NavigationPage,
    hero: Hero,
    rail: gtk::FlowBox,
    problems: gtk::Label,
    stack: gtk::Stack,
    add: gtk::Button,
    start: gtk::Button,
    update: adw::Banner,
}

/// The app's name and version, where a title would be.
fn brand() -> gtk::Box {
    let name = gtk::Label::new(Some("Guesthouse"));
    name.add_css_class("brand");
    let version = gtk::Label::new(Some(VERSION));
    version.add_css_class("dim-label");
    version.add_css_class("caption");
    let brand = gtk::Box::builder().spacing(BRAND_SPACING).margin_start(BRAND_SPACING).build();
    brand.append(&gtk::Image::from_icon_name(&format!("{APP_ID}-symbolic")));
    brand.append(&name);
    brand.append(&version);
    brand
}

fn main_menu() -> gio::Menu {
    let setup_items = gio::Menu::new();
    setup_items.append(Some("Run Setup Again"), Some("win.setup"));
    setup_items.append(Some("Show Log"), Some("win.log"));
    setup_items.append(Some("Check for Updates"), Some("win.check-updates"));
    let about_items = gio::Menu::new();
    about_items.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
    about_items.append(Some("About Guesthouse"), Some("win.about"));
    let menu = gio::Menu::new();
    menu.append_section(None, &setup_items);
    menu.append_section(None, &about_items);
    menu
}

impl LibraryPage {
    pub fn new(description: &str) -> Self {
        let hero = Hero::new();
        let rail = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .column_spacing(CARD_SPACING)
            .row_spacing(CARD_SPACING)
            .max_children_per_line(CARDS_ACROSS)
            .valign(gtk::Align::Start)
            .build();
        rail.add_css_class("rail");
        let machines = gtk::Label::builder().label("Machines").xalign(0.0).build();
        machines.add_css_class("section-label");
        let problems = gtk::Label::builder().wrap(true).xalign(0.0).visible(false).build();
        problems.add_css_class("error");
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(COLUMN_SPACING)
            .margin_top(COLUMN_MARGIN)
            .margin_bottom(COLUMN_MARGIN)
            .margin_start(SIDE_MARGIN)
            .margin_end(SIDE_MARGIN)
            .build();
        column.append(hero.widget());
        column.append(&machines);
        column.append(&rail);
        column.append(&problems);
        let clamp = adw::Clamp::builder().child(&column).maximum_size(COLUMN_WIDTH).build();
        let scroller = gtk::ScrolledWindow::builder().child(&clamp).build();
        let start = gtk::Button::builder().label("New Machine").halign(gtk::Align::Center).build();
        start.add_css_class("pill");
        start.add_css_class("suggested-action");
        let empty = adw::StatusPage::builder()
            .icon_name(APP_ID)
            .title("No Machines")
            .description(description)
            .child(&start)
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&scroller, Some("machines"));
        stack.add_named(&empty, Some("empty"));
        let add = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label("New Machine").build())
            .build();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
        header.pack_start(&brand());
        let menu = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .primary(true)
            .tooltip_text("Main Menu")
            .menu_model(&main_menu())
            .build();
        name_for_readers(&menu, "Main Menu");
        header.pack_end(&menu);
        header.pack_end(&add);
        let toolbar = adw::ToolbarView::new();
        toolbar.set_content(Some(&stack));
        toolbar.add_top_bar(&header);
        let update = adw::Banner::builder().button_label("Update").action_name("win.update").build();
        toolbar.add_top_bar(&update);
        let page = adw::NavigationPage::builder().title("Guesthouse").tag("library").child(&toolbar).build();
        Self { page, hero, rail, problems, stack, add, start, update }
    }

    pub fn connect_new_machine(&self, window: &Window) {
        for button in [&self.add, &self.start] {
            let window = window.clone();
            button.connect_clicked(move |_| window.show_new_machine());
        }
    }

    pub fn offer_update(&self, version: &str) {
        self.update.set_title(&format!("Guesthouse {version} is out. You have {VERSION}."));
        self.update.set_revealed(true);
    }

    pub fn hero(&self) -> &Hero {
        &self.hero
    }

    pub fn add(&self, card: &gtk::Box) {
        self.rail.append(card);
    }

    pub fn remove(&self, card: &gtk::Box) {
        self.rail.remove(card);
    }

    /// Calls `chosen` with the card that was clicked, or that Enter was pressed on.
    pub fn connect_chosen(&self, chosen: impl Fn(&gtk::Widget) + 'static) {
        self.rail.connect_child_activated(move |_, holder| {
            if let Some(card) = holder.child() {
                chosen(&card);
            }
        });
    }

    /// Rings the card of the machine the hero shows.
    pub fn mark_chosen(&self, chosen: Option<&gtk::Box>) {
        let mut holder = self.rail.first_child();
        while let Some(found) = holder {
            let is_chosen = chosen.is_some_and(|card| card.parent().as_ref() == Some(&found));
            if is_chosen {
                found.add_css_class("chosen");
            } else {
                found.remove_css_class("chosen");
            }
            holder = found.next_sibling();
        }
    }

    pub fn show(&self, has_machines: bool, errors: &[String]) {
        self.problems.set_label(&errors.join("\n"));
        self.problems.set_visible(!errors.is_empty());
        let filled = has_machines || !errors.is_empty();
        self.stack.set_visible_child_name(if filled { "machines" } else { "empty" });
    }
}
