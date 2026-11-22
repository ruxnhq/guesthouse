//! What the window shows: the machines in the navigation on the left, and on the right the chosen
//! machine's page or the new machine's form. It's drawn from `Main` each time something changes, and
//! holds nothing of its own.
//!
//! It's laid out as Windows 11's own apps are: Mica under everything, the title bar drawn by the app,
//! and each page a column of sections, each a heading over a card of rows. Every colour is one of
//! WinUI's theme brushes, so light and dark both follow Windows.

use std::rc::Rc;

use guesthouse_control::client::Unreached;
use guesthouse_control::{Card, CardInfo, Machine, State, System};
use windows_reactor::*;

use crate::app::{self, Account, ConsoleState, DELETE_KEY, Draft, Kind, Line, Main, Message, NEW_TAG, Page};

/// The app's icon, the same as the GTK app's, for where it's shown large.
const LOGO_SVG: &[u8] =
    include_bytes!("../../app/data/icons/hicolor/scalable/apps/io.github.xnixy.Guesthouse.svg");
/// The window's first size, and the least it can be made, in WinUI's units.
const WIDTH: f64 = 1180.0;
const HEIGHT: f64 = 820.0;
const LEAST_WIDTH: f64 = 760.0;
const LEAST_HEIGHT: f64 = 520.0;
/// The navigation on the left.
const PANE_WIDTH: f64 = 280.0;
/// A page's widest. Wider, its rows get hard to read across.
const PAGE_WIDTH: f64 = 1000.0;
/// Between a page's sections, and between a section's heading and its card.
const SECTION_GAP: f64 = 24.0;
const HEADING_GAP: f64 = 8.0;
/// How tall a log is before it scrolls.
const LOG_HEIGHT: f64 = 320.0;
/// A log's font: Windows Terminal's, and Consolas where that isn't installed.
const MONO: &str = "Cascadia Mono, Consolas";
/// The size of the app's icon on a page that says one thing.
const LOGO_SIZE: f64 = 96.0;
/// The size of the exe's icon the title bar shows, in pixels: the largest of the plainer drawings
/// for small sizes, which WinUI shrinks to the title bar's 16 units at most scales.
const TITLE_ICON_FRAME: u8 = 24;

/// Glyphs of Segoe Fluent Icons.
mod glyph {
    pub const WINDOWS: &str = "\u{E737}";
    pub const LINUX: &str = "\u{E83B}";
    pub const TERMINAL: &str = "\u{E756}";
    pub const START: &str = "\u{E768}";
    pub const STOP: &str = "\u{E71A}";
    pub const MORE: &str = "\u{E712}";
    pub const DELETE: &str = "\u{E74D}";
    pub const ADD: &str = "\u{E710}";
    pub const COPY: &str = "\u{E8C8}";
    pub const KEY: &str = "\u{E8D7}";
    pub const REMOTE: &str = "\u{E703}";
    pub const FOLDER: &str = "\u{E8B7}";
    pub const RENAME: &str = "\u{E8AC}";
    pub const MEMORY: &str = "\u{E964}";
    pub const PROCESSOR: &str = "\u{E950}";
    pub const DISK: &str = "\u{EDA2}";
    pub const CARD: &str = "\u{F211}";
    pub const POWER: &str = "\u{E7E8}";
    pub const SETUP: &str = "\u{E9F5}";
    pub const PROBLEM: &str = "\u{EA39}";
    pub const WARNING: &str = "\u{E7BA}";
    pub const REFRESH: &str = "\u{E72C}";
    pub const SCREEN: &str = "\u{E7F4}";
    pub const CONSOLE: &str = "\u{E7F8}";
    pub const CLOSE: &str = "\u{E711}";
    pub const SAVE: &str = "\u{E74E}";
    pub const UNDO: &str = "\u{E7A7}";
    pub const GAME: &str = "\u{E7FC}";
}

pub fn view(main: &Main, context: &mut ViewContext<Main>) -> View {
    context.window_title("Guesthouse");
    let least = WindowConstraints {
        min_width: Some(LEAST_WIDTH),
        min_height: Some(LEAST_HEIGHT),
        ..Default::default()
    };
    let mut visuals = WindowVisuals::new()
        .client_size(WIDTH, HEIGHT)
        .constraints(least)
        .backdrop(WindowBackdrop::Mica)
        .theme(main.theme);
    if let Some(icon) = &main.icon {
        visuals = visuals.icon(icon.clone());
    }
    context.window_visuals(visuals);

    let small = match crate::system::icon_frame(TITLE_ICON_FRAME) {
        Some(png) => Icon::image_data(EncodedImage::from_static(png)),
        None => Icon::image_svg(logo()),
    };
    let mut title_bar = TitleBar::new().title("Guesthouse").icon(small);
    if let Ok(status) = &main.seen {
        title_bar = title_bar.subtitle(format!("service {}", status.version));
    }
    let body: View = match &main.seen {
        Err(words) => away(words),
        Ok(_) => navigation(main, context),
    };
    let news = InfoBar::new()
        .is_open(main.news.is_some())
        .is_closable(true)
        .severity(match &main.news {
            Some(news) if news.bad => InfoBarSeverity::Error,
            _ => InfoBarSeverity::Informational,
        })
        .message(main.news.as_ref().map(|news| news.text.as_str()).unwrap_or_default())
        .vertical_alignment(VerticalAlignment::Bottom)
        .horizontal_alignment(HorizontalAlignment::Center)
        .margin(16.0)
        .max_width(560.0)
        .on_closed(context.message(Message::NewsClosed));
    let content = Grid::new().rows([GridLength::Auto, GridLength::STAR]).children((
        title_bar.grid_row(0),
        Border::new().grid_row(1).content(body),
        news.grid_row(1),
    ));
    match &main.deleting {
        Some(name) => content.content_dialog(delete_dialog(name, context)),
        None => content.into(),
    }
}

fn logo() -> EncodedImage {
    EncodedImage::from_static(LOGO_SVG)
}

// ---- Where the service can't be asked ----

/// In place of everything else when the service can't be asked.
fn away(words: &str) -> View {
    let mut parts: Vec<View> = vec![
        icon(glyph::WARNING, 48.0).foreground(ThemeBrush::SystemCaution).into(),
        subtitle(words).text_alignment(TextAlignment::Center).text_wrapping(TextWrapping::Wrap).into(),
        centred(
            "The Guesthouse service makes and runs the machines, as the system, and this app only shows \
             what it says and asks it for what you choose. The machines go on as they are without the app.",
        ),
    ];
    if words == Unreached::NoService.words() {
        parts.push(centred(
            "Once it's installed, it starts with Windows, and if it was stopped, Services starts it again: \
             it's called Guesthouse there. Until then, run guesthouse-service.exe run in an elevated prompt.",
        ));
    }
    parts.push(
        StackPanel::new()
            .orientation(Orientation::Horizontal)
            .spacing(8.0)
            .horizontal_alignment(HorizontalAlignment::Center)
            .margin(Thickness::new(0.0, 8.0, 0.0, 0.0))
            .children((
                ProgressRing::new().is_active(true).width(16.0).height(16.0),
                caption(
                    "The app asks again every two seconds, and shows the machines as soon as it answers.",
                ),
            ))
            .into(),
    );
    one_thing(parts)
}

// ---- The navigation ----

fn navigation(main: &Main, context: &ViewContext<Main>) -> View {
    let machines = main.status().map(|status| status.machines.as_slice()).unwrap_or_default();
    let selected = match main.page {
        Page::Machine => main.selected.as_deref(),
        Page::New => None,
    };
    let items = machines.iter().map(|machine| {
        let item = NavigationViewItem::new()
            .tag(machine.name.as_str())
            .icon(Icon::font(system_glyph(machine.system)))
            .content(nav_item(machine))
            .is_selected(selected == Some(machine.name.as_str()))
            .automation_name(format!(
                "{}, {}, {}",
                machine.name,
                system_words(machine.system),
                app::state_words(machine.state)
            ));
        keyed(machine.name.as_str(), item)
    });
    let new = NavigationViewItem::new()
        .tag(NEW_TAG)
        .icon(Icon::font(glyph::ADD))
        .content("New machine")
        .is_selected(main.page == Page::New);
    let page: View = match (main.page, main.machine()) {
        (Page::New, _) => new_page(main, context),
        (Page::Machine, Some(machine)) => machine_page(main, machine, context),
        (Page::Machine, None) => empty(context),
    };
    NavigationView::new()
        .pane_display_mode(NavigationViewPaneDisplayMode::Left)
        .is_back_button_visible(NavigationViewBackButtonVisible::Collapsed)
        .is_settings_visible(false)
        .is_pane_toggle_button_visible(false)
        .open_pane_length(PANE_WIDTH)
        .pane_header(
            TextBlock::new()
                .text("Machines")
                .font_weight(FontWeight::SEMI_BOLD)
                .foreground(ThemeBrush::SecondaryText)
                .margin(Thickness::new(16.0, 8.0, 0.0, 8.0))
                .vertical_alignment(VerticalAlignment::Center),
        )
        .keyed_menu_items(items)
        .footer_menu_items((new,))
        .on_selected_tag_changed(
            context.callback(|tag: Option<Rc<str>>| Message::Navigate(tag.map(|tag| tag.to_string()))),
        )
        .content(page)
        .into()
}

/// A machine in the navigation: its name, and where it stands beside a coloured dot.
fn nav_item(machine: &Machine) -> View {
    StackPanel::new()
        .margin(Thickness::new(0.0, 6.0, 0.0, 6.0))
        .children((
            TextBlock::new().text(machine.name.as_str()).text_trimming(TextTrimming::CharacterEllipsis),
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(6.0)
                .children((dot(machine.state), caption(app::state_words(machine.state)))),
        ))
        .into()
}

/// What the right side shows when there's no machine.
fn empty(context: &ViewContext<Main>) -> View {
    one_thing(vec![
        Image::new()
            .source_svg(logo())
            .width(LOGO_SIZE)
            .height(LOGO_SIZE)
            .horizontal_alignment(HorizontalAlignment::Center)
            .into(),
        subtitle("No machines yet").horizontal_alignment(HorizontalAlignment::Center).into(),
        centred(
            "Make one from a Windows 11 or Ubuntu Server image. Its first boot sets it up without the card, \
             and it has its card from the next boot on.",
        ),
        button("New machine", glyph::ADD, context.message(Message::NewMachine), true)
            .horizontal_alignment(HorizontalAlignment::Center)
            .margin(Thickness::new(0.0, 8.0, 0.0, 0.0))
            .into(),
    ])
}

// ---- A machine's page ----

fn machine_page(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let mut sections = vec![header(machine, context)];
    sections.push(group("Status", status_rows(machine)));
    sections.push(group("Connect", connect_rows(main, machine, context)));
    if let Some(draft) = main.draft.as_ref().filter(|draft| draft.machine == machine.name) {
        sections.push(settings_section(main, machine, draft, context));
    }
    sections.push(log_section(main, context));
    page(sections)
}

/// The machine's name, where it stands, and what can be done to it.
fn header(machine: &Machine, context: &ViewContext<Main>) -> View {
    let state = machine.state;
    let tile = Border::new()
        .width(64.0)
        .height(64.0)
        .corner_radius(8.0)
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .vertical_alignment(VerticalAlignment::Center)
        .content(
            icon(system_glyph(machine.system), 32.0)
                .horizontal_alignment(HorizontalAlignment::Center)
                .vertical_alignment(VerticalAlignment::Center),
        );
    let about = format!("{} · {}", system_words(machine.system), memory_words(machine.memory_mb));
    let words =
        StackPanel::new().spacing(6.0).vertical_alignment(VerticalAlignment::Center).grid_column(1).children(
            (
                title(&machine.name),
                StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(12.0)
                    .children((badge(state), caption(&about).vertical_alignment(VerticalAlignment::Center))),
            ),
        );

    let start = button("Start", glyph::START, context.message(Message::Start), true)
        .is_enabled(state == State::Stopped);
    // Asked again while it shuts down, the service stops it at once.
    let stop_words = if state == State::Stopping { "Stop now" } else { "Stop" };
    let stop = button(stop_words, glyph::STOP, context.message(Message::Stop), false)
        .is_enabled(matches!(state, State::Starting | State::Running | State::Stopping));
    let delete = MenuItem::item(DELETE_KEY, "Delete…")
        .glyph(glyph::DELETE)
        .enabled(state == State::Stopped)
        .tooltip("Only a stopped machine can be deleted. Its disks go with it.");
    let more = square(glyph::MORE, &format!("More for {}", machine.name)).tooltip("More").menu(Menu::new(
        [delete],
        context.callback(|key: Key| Message::More(key.as_str().map(str::to_owned))),
    ));
    let actions = StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(8.0)
        .vertical_alignment(VerticalAlignment::Center)
        .grid_column(2)
        .children((start, stop, more));
    Grid::new()
        .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
        .column_spacing(16.0)
        .children((tile, words, actions))
        .into()
}

fn status_rows(machine: &Machine) -> Vec<View> {
    let state = machine.state;
    let line = match state {
        State::Stopping => {
            "Its guest is shutting down. Stop now turns it off at once, as pulling the plug would."
        }
        State::Stopped => "Start boots it. Delete, under More, takes its disks with it.",
        State::Preparing => "Its disks are being made.",
        State::Starting => "Its guest is booting. Stop asks it to shut down.",
        State::Running => "Stop asks its guest to shut down.",
    };
    // A ring while it's on its way somewhere, so it's plain that it's working.
    let working = matches!(state, State::Preparing | State::Starting | State::Stopping);
    let ring: Option<View> =
        working.then(|| ProgressRing::new().is_active(true).width(20.0).height(20.0).into());
    vec![
        row(glyph::POWER, app::state_words(state), &[line], ring),
        row(glyph::SETUP, "Setup", &[&setup_words(machine)], None),
    ]
}

/// What the first boot does, and where this machine is in that.
fn setup_words(machine: &Machine) -> String {
    let first =
        "A machine's first boot sets it up without the card, and it has its card from the next boot on.";
    match (machine.set_up, machine.state, &machine.card) {
        (false, State::Starting | State::Running, _) => {
            format!("Its first boot is setting it up, which takes a few minutes. {first}")
        }
        (false, _, _) => format!("Not set up yet: its first boot does that. {first}"),
        (true, _, Some(card)) => format!("Set up. It boots with {card}."),
        (true, _, None) => "Set up. It has no card.".to_owned(),
    }
}

fn connect_rows(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> Vec<View> {
    let address = app::address(machine);
    let account = main.account.as_ref().filter(|account| account.machine == machine.name);
    let reachable = address.is_some() && machine.state == State::Running;
    let where_words = match address {
        Some(address) => format!("Its guest last said its address is {address}."),
        None => "No address yet. The guest says its address once it's running and on the network.".to_owned(),
    };
    let mut rows = Vec::new();
    match machine.system {
        System::Windows => {
            // remoteman's host is put in the guest at its boots after the first, which set it up.
            let streams = reachable && machine.set_up;
            let screen = button("Open screen", glyph::SCREEN, context.message(Message::Screen), true)
                .is_enabled(streams);
            rows.push(row(
                glyph::SCREEN,
                "Screen",
                &["The guest's screen at full speed, drawn by its card and streamed by remoteman."],
                Some(screen.into()),
            ));
            let open =
                button("Remote Desktop", glyph::REMOTE, context.message(Message::RemoteDesktop), false)
                    .is_enabled(reachable);
            rows.push(row(
                glyph::REMOTE,
                "Remote Desktop",
                &[&where_words, "Remote Desktop shows the screen of a Pro guest, and Home has none."],
                Some(open.into()),
            ));
        }
        System::Linux => match (account, address) {
            (Some(account), Some(address)) => rows.push(copy_row(
                glyph::TERMINAL,
                "SSH",
                &format!("ssh {}@{address}", account.user),
                context,
            )),
            _ => rows.push(row(glyph::TERMINAL, "SSH", &[&where_words], None)),
        },
    }
    rows.push(console_row(main, machine, context));
    match account {
        Some(Account { user, password, .. }) => {
            rows.push(copy_row(glyph::KEY, "User", user, context));
            rows.push(copy_row(glyph::KEY, "Password", password, context));
        }
        None => {
            let show = Button::new().content("Show").on_click(context.message(Message::SignIn));
            rows.push(row(
                glyph::KEY,
                "User and password",
                &["The guest's administrator, made for this machine."],
                Some(show.into()),
            ));
        }
    }
    rows
}

/// The machine's console: its own screen, which shows from the firmware on, needing nothing of
/// the guest, so it's there for setup and a boot that's stuck.
fn console_row(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let about = "Its own screen from the moment it's on: the firmware, setup and the sign-in screen. \
                 The keyboard and mouse go to the guest while its window has them.";
    let open = main.console.as_ref().filter(|open| open.machine == machine.name);
    let Some(open) = open else {
        let on = matches!(machine.state, State::Starting | State::Running | State::Stopping);
        let button =
            button("Open console", glyph::CONSOLE, context.message(Message::Console), false).is_enabled(on);
        return row(glyph::CONSOLE, "Console", &[about], Some(button.into()));
    };
    let (words, ended) = match &open.state {
        ConsoleState::Waiting => ("Waiting for the machine's screen to be served.", false),
        ConsoleState::Connecting => ("Connecting.", false),
        ConsoleState::Connected => ("Open in its own window. Closing it leaves the machine running.", false),
        ConsoleState::Ended(why) => (why.as_str(), true),
    };
    let mut buttons: Vec<View> = Vec::new();
    if ended {
        buttons.push(button("Reconnect", glyph::REFRESH, context.message(Message::Reconnect), true).into());
    } else {
        let show = button("Show", glyph::CONSOLE, context.message(Message::Console), false);
        buttons.push(show.automation_name("Show the console").into());
    }
    buttons.push(
        button("Close", glyph::CLOSE, context.message(Message::CloseConsole), false)
            .automation_name("Close the console")
            .into(),
    );
    let right = StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children(buttons);
    let state =
        caption(words).foreground(if ended { ThemeBrush::SystemCritical } else { ThemeBrush::SecondaryText });
    row_with(
        glyph::CONSOLE,
        "Console",
        StackPanel::new().children((caption(about), state)),
        Some(right.into()),
    )
}

/// What the machine is given at each boot, which can be changed while it's off: the same controls
/// as the new machine's form, then Save and Undo.
fn settings_section(main: &Main, machine: &Machine, draft: &Draft, context: &ViewContext<Main>) -> View {
    let stopped = machine.state == State::Stopped;
    let open = stopped && !draft.saving;
    let mut rows = vec![
        row(
            glyph::MEMORY,
            "Memory (MB)",
            &["Taken from this PC while the machine runs."],
            Some(number_box(
                "Memory (MB)",
                draft.memory_mb,
                1024.0,
                open,
                context.callback(Message::SetMemory),
            )),
        ),
        row(
            glyph::PROCESSOR,
            "Processors",
            &[],
            Some(number_box(
                "Processors",
                draft.processors,
                1.0,
                open,
                context.callback(Message::SetProcessors),
            )),
        ),
        row(
            glyph::CARD,
            "Card",
            &[CARD_NOTE],
            Some(card_box(
                main,
                &main.settings_cards(),
                &draft.card,
                open,
                context.callback(Message::SetCard),
            )),
        ),
    ];
    if app::offers_dlss(machine.system, &draft.card, main.cards()) {
        let switch = ToggleSwitch::new()
            .is_on(draft.nvapi_wrapper)
            .is_enabled(open)
            .automation_name("DLSS")
            .on_toggled(context.callback(Message::SetDlss));
        rows.push(row(glyph::GAME, "DLSS", &[DLSS_NOTE], Some(switch.into())));
    }

    let changed = draft.changed(machine);
    let save = button("Save", glyph::SAVE, context.message(Message::SaveSettings), true)
        .is_enabled(open && changed && draft.settings().is_some());
    let undo = button("Undo", glyph::UNDO, context.message(Message::UndoSettings), false)
        .is_enabled(open && (changed || draft.refused.is_some()));
    let mut foot: Vec<View> = vec![save.into(), undo.into()];
    let (words, bad) = match (&draft.refused, stopped) {
        (Some(reason), _) => (Some(reason.as_str()), true),
        (None, false) => (Some("Its settings change while it's off. Stop it first."), false),
        (None, true) if draft.saving => (Some("Saving."), false),
        (None, true) if changed && draft.settings().is_none() => {
            (Some("Give it memory and processors first."), false)
        }
        (None, true) if changed => (Some("They take effect when it next starts."), false),
        (None, true) => (None, false),
    };
    if let Some(words) = words {
        let line = caption(words).vertical_alignment(VerticalAlignment::Center);
        foot.push(if bad { line.foreground(ThemeBrush::SystemCritical).into() } else { line.into() });
    }
    let foot = StackPanel::new().orientation(Orientation::Horizontal).spacing(12.0).children(foot);
    StackPanel::new().spacing(HEADING_GAP).children((group("Settings", rows), foot)).into()
}

/// What a card is to the guest, under the card's list.
const CARD_NOTE: &str = "The card the guest shares with this PC. Default is whichever has the most memory of its own \
                         when it boots.";

/// The DLSS switch's note, in the Linux app's words.
const DLSS_NOTE: &str = "NVIDIA's NVAPI won't tell DLSS which card it's on in a virtual machine, so Guesthouse puts \
                         its own NVAPI in Windows' place for it. It isn't signed, and some games' anti-cheat refuses \
                         to start with it. Turn this off for those games, and NVIDIA's goes back. Takes effect when \
                         the machine starts.";

/// A box for a whole number, as memory and processors are given.
fn number_box(
    name: &str,
    value: Option<f64>,
    step: f64,
    enabled: bool,
    changed: Callback<Option<f64>>,
) -> View {
    NumberBox::new()
        .value(value)
        .minimum(1.0)
        .small_change(step)
        .spin_button_placement_mode(NumberBoxSpinButtonPlacementMode::Inline)
        .is_enabled(enabled)
        .width(180.0)
        .automation_name(name)
        .on_value_changed(changed)
        .into()
}

/// The list of cards, with `chosen` selected.
fn card_box(
    main: &Main,
    choices: &[Card],
    chosen: &Card,
    enabled: bool,
    changed: Callback<Option<usize>>,
) -> View {
    let names: Vec<String> = choices.iter().map(|card| card_words(card, main.cards())).collect();
    ComboBox::new()
        .items_source(names)
        .selected_index(choices.iter().position(|card| card == chosen))
        .is_enabled(enabled)
        .width(360.0)
        .automation_name("Card")
        .on_selection_changed(changed)
        .into()
}

fn log_section(main: &Main, context: &ViewContext<Main>) -> View {
    let problems = main.log.iter().filter(|line| line.kind == Kind::Problem).count();
    let mut right: Vec<View> = Vec::new();
    if problems > 0 {
        let words = if problems == 1 { "1 problem".to_owned() } else { format!("{problems} problems") };
        right.push(
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(6.0)
                .vertical_alignment(VerticalAlignment::Center)
                .children((
                    icon(glyph::PROBLEM, 12.0).foreground(ThemeBrush::SystemCritical),
                    caption(&words).foreground(ThemeBrush::SystemCritical),
                ))
                .into(),
        );
    }
    if !main.watching() {
        right.push(button("Watch again", glyph::REFRESH, context.message(Message::WatchAgain), false).into());
    }
    let heading = Grid::new().columns([GridLength::STAR, GridLength::Auto]).children((
        strong("Log").vertical_alignment(VerticalAlignment::Bottom),
        StackPanel::new().orientation(Orientation::Horizontal).spacing(12.0).grid_column(1).children(right),
    ));
    StackPanel::new().spacing(HEADING_GAP).children((heading, log(&main.log, "Nothing yet."))).into()
}

// ---- The new machine's form ----

fn new_page(main: &Main, context: &ViewContext<Main>) -> View {
    let form = &main.form;
    let busy = main.busy();
    let mut sections: Vec<View> = vec![
        StackPanel::new()
            .spacing(4.0)
            .children((
                title("New machine"),
                caption(
                    "The service makes its disk from the image. Its first boot sets it up without the card, and \
                     it has its card from the next boot on.",
                ),
            ))
            .into(),
    ];

    let name_ok = form.name.trim().is_empty() || guesthouse_control::valid_name(form.name.trim());
    let name_hint = if name_ok {
        "Letters, digits, - and _, up to 40."
    } else {
        "Only letters, digits, - and _, up to 40."
    };
    let name = TextBox::new()
        .placeholder_text("win11")
        .text(form.name.as_str())
        .width(280.0)
        .is_enabled(!busy)
        .automation_name("Name")
        .on_text_changed(context.callback(|text: std::rc::Rc<str>| Message::Name(text.to_string())));
    let name_row = row_with(
        glyph::RENAME,
        "Name",
        caption(name_hint).foreground(if name_ok {
            ThemeBrush::SecondaryText
        } else {
            ThemeBrush::SystemCritical
        }),
        Some(name.into()),
    );
    let image = Grid::new().columns([GridLength::STAR, GridLength::Auto]).column_spacing(8.0).children((
        TextBox::new()
            .placeholder_text("A path to an ISO, install.wim or install.esd")
            .text(form.image.as_str())
            .is_enabled(!busy)
            .automation_name("Image")
            .on_text_changed(context.callback(|text: std::rc::Rc<str>| Message::Image(text.to_string()))),
        button("Browse…", glyph::FOLDER, context.message(Message::Browse), false)
            .is_enabled(!busy)
            .grid_column(1),
    ));
    let image_row = row_under(
        glyph::FOLDER,
        "Image",
        "A Windows 11 ISO, or its install.wim or install.esd, or an Ubuntu Server ISO.",
        image.into(),
    );
    sections.push(group("Machine", vec![name_row, image_row]));

    let number = |name: &str, value: Option<f64>, step: f64, changed: Callback<Option<f64>>| -> View {
        number_box(name, value, step, !busy, changed)
    };
    sections.push(group(
        "Size",
        vec![
            row(
                glyph::MEMORY,
                "Memory (MB)",
                &["Taken from this PC while the machine runs."],
                Some(number("Memory (MB)", form.memory_mb, 1024.0, context.callback(Message::Memory))),
            ),
            row(
                glyph::PROCESSOR,
                "Processors",
                &[],
                Some(number("Processors", form.processors, 1.0, context.callback(Message::Processors))),
            ),
            row(
                glyph::DISK,
                "Disk (GB)",
                &["The disk's largest size. It only takes what the guest writes."],
                Some(number("Disk (GB)", form.size_gb, 16.0, context.callback(Message::Size))),
            ),
        ],
    ));

    let cards = card_box(main, &main.card_choices(), &form.card, !busy, context.callback(Message::Card));
    sections.push(group("Graphics", vec![row(glyph::CARD, "Card", &[CARD_NOTE], Some(cards))]));

    let ready = form.machine().is_some() && !busy;
    let mut foot: Vec<View> =
        vec![button("Make it", glyph::ADD, context.message(Message::Create), true).is_enabled(ready).into()];
    if !ready && !busy {
        foot.push(
            caption("Give it a name and an image first.")
                .vertical_alignment(VerticalAlignment::Center)
                .into(),
        );
    }
    sections.push(StackPanel::new().orientation(Orientation::Horizontal).spacing(12.0).children(foot).into());

    if let Some(making) = &main.making {
        let mut parts: Vec<View> = vec![strong(&format!("Making {}", making.name)).into()];
        match &making.ended {
            None => parts.push(ProgressBar::new().is_indeterminate(true).into()),
            Some(ended) => parts.push(
                InfoBar::new()
                    .is_open(true)
                    .is_closable(false)
                    .severity(if ended.is_none() { InfoBarSeverity::Success } else { InfoBarSeverity::Error })
                    .title(if ended.is_none() { "Made" } else { "Not made" })
                    .message(match ended {
                        None => "It's in the list. Start it there, and its first boot sets it up.",
                        Some(why) => why.as_str(),
                    })
                    .into(),
            ),
        }
        parts.push(log(&making.lines, "Waiting for the service."));
        // Under the title, where it's seen at once, rather than under the form that started it.
        sections.insert(1, StackPanel::new().spacing(HEADING_GAP).children(parts).into());
    }
    page(sections)
}

/// A card in the form's list: its name and its memory, so cards of one maker can be told apart.
fn card_words(card: &Card, cards: &[CardInfo]) -> String {
    match card {
        Card::Default => match cards.iter().find(|card| card.default) {
            Some(card) => format!("Default, now {}", card.name),
            None => "Default".to_owned(),
        },
        Card::Named(name) => {
            let info = cards.iter().find(|card| card.name == *name);
            let mut words = name.clone();
            if let Some(mb) = info.and_then(|card| card.memory_mb) {
                words.push_str(&format!(" · {}", card_memory_words(mb)));
            }
            if info.is_some_and(|card| card.default) {
                words.push_str(" · default");
            }
            words
        }
        Card::None => "None".to_owned(),
    }
}

fn delete_dialog(name: &str, context: &ViewContext<Main>) -> ContentDialog {
    ContentDialog::new()
        .title(format!("Delete {name}?"))
        .content(
            TextBlock::new()
                .text("Its disks are deleted with it, and that can't be undone.")
                .text_wrapping(TextWrapping::Wrap),
        )
        .primary_button_text("Delete")
        .close_button_text("Cancel")
        .default_button(ContentDialogButton::Close)
        .is_open(true)
        .on_closed(context.callback(Message::DeleteAnswered))
}

// ---- The pieces the pages are made of ----

fn system_words(system: System) -> &'static str {
    match system {
        System::Windows => "Windows",
        System::Linux => "Linux",
    }
}

fn system_glyph(system: System) -> &'static str {
    match system {
        System::Windows => glyph::WINDOWS,
        System::Linux => glyph::LINUX,
    }
}

const MB_PER_GB: u64 = 1024;

/// A machine's memory, as it was given: in GB when it's a whole number of them, and in MB otherwise.
fn memory_words(mb: u64) -> String {
    if mb >= MB_PER_GB && mb.is_multiple_of(MB_PER_GB) {
        format!("{} GB", mb / MB_PER_GB)
    } else {
        format!("{mb} MB")
    }
}

/// A card's memory, to the nearest GB, as a card's driver gives a little under its size.
fn card_memory_words(mb: u64) -> String {
    if mb >= MB_PER_GB { format!("{} GB", (mb + MB_PER_GB / 2) / MB_PER_GB) } else { format!("{mb} MB") }
}

/// The colour of a machine's dot: green when running, amber on its way somewhere, grey when stopped.
fn state_brush(state: State) -> ThemeBrush {
    match state {
        State::Running => ThemeBrush::SystemSuccess,
        State::Starting | State::Stopping | State::Preparing => ThemeBrush::SystemCaution,
        State::Stopped => ThemeBrush::TertiaryText,
    }
}

fn dot(state: State) -> Ellipse {
    Ellipse::new()
        .width(8.0)
        .height(8.0)
        .fill(state_brush(state))
        .vertical_alignment(VerticalAlignment::Center)
}

/// Where a machine stands, as a dot and its word in a pill.
fn badge(state: State) -> Border {
    Border::new()
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .corner_radius(12.0)
        .padding(Thickness::new(10.0, 3.0, 12.0, 4.0))
        .vertical_alignment(VerticalAlignment::Center)
        .content(
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(8.0)
                .children((dot(state), TextBlock::new().text(app::state_words(state)).font_size(12.0))),
        )
}

/// A page: its sections in a column, scrolled when they don't fit.
fn page(sections: Vec<View>) -> View {
    ScrollViewer::new()
        .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
        .content(
            StackPanel::new()
                .spacing(SECTION_GAP)
                .max_width(PAGE_WIDTH)
                .margin(Thickness::new(32.0, 24.0, 32.0, 32.0))
                .children(sections),
        )
        .into()
}

/// A page that says one thing, in the middle: a picture, a line, and what to do.
fn one_thing(parts: Vec<View>) -> View {
    StackPanel::new()
        .spacing(12.0)
        .max_width(480.0)
        .margin(32.0)
        .horizontal_alignment(HorizontalAlignment::Center)
        .vertical_alignment(VerticalAlignment::Center)
        .children(parts)
        .into()
}

/// WinUI's Title style.
fn title(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_family("Segoe UI Variable Display")
        .font_size(28.0)
        .font_weight(FontWeight::SEMI_BOLD)
        .text_trimming(TextTrimming::CharacterEllipsis)
}

/// WinUI's Subtitle style.
fn subtitle(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_family("Segoe UI Variable Display")
        .font_size(20.0)
        .font_weight(FontWeight::SEMI_BOLD)
}

/// WinUI's BodyStrong style.
fn strong(text: &str) -> TextBlock {
    TextBlock::new().text(text).font_weight(FontWeight::SEMI_BOLD)
}

/// WinUI's Caption style, in the secondary text colour.
fn caption(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_size(12.0)
        .foreground(ThemeBrush::SecondaryText)
        .text_wrapping(TextWrapping::Wrap)
}

/// Words in the middle of a page that says one thing.
fn centred(text: &str) -> View {
    TextBlock::new()
        .text(text)
        .foreground(ThemeBrush::SecondaryText)
        .text_wrapping(TextWrapping::Wrap)
        .text_alignment(TextAlignment::Center)
        .into()
}

fn icon(glyph: &str, size: f64) -> FontIcon {
    FontIcon::new().glyph(glyph).font_size(size)
}

/// A button with a glyph before its words.
fn button(text: &str, glyph: &str, clicked: Callback<()>, accent: bool) -> Button {
    let content = StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(8.0)
        .children((icon(glyph, 14.0), TextBlock::new().text(text)));
    let button = Button::new().content(content).automation_name(text).on_click(clicked);
    if accent { button.style(ButtonStyle::Accent) } else { button }
}

/// A square button with only a glyph, named for those who can't see it.
fn square(glyph: &str, name: &str) -> Button {
    Button::new().content(icon(glyph, 16.0)).automation_name(name)
}

/// A card: WinUI's card fill and stroke, rounded.
fn card(child: impl Into<View>) -> Border {
    Border::new()
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .corner_radius(8.0)
        .content(child)
}

/// A section: its heading, and a card of its rows with a hairline between each.
fn group(heading: &str, rows: Vec<View>) -> View {
    let mut inside: Vec<View> = Vec::new();
    for (n, row) in rows.into_iter().enumerate() {
        if n > 0 {
            inside.push(Border::new().height(1.0).background(ThemeBrush::Divider).into());
        }
        inside.push(row);
    }
    StackPanel::new()
        .spacing(HEADING_GAP)
        .children((strong(heading), card(StackPanel::new().children(inside))))
        .into()
}

/// A row of a section: a glyph, a title with lines under it, and something on the right.
fn row(glyph: &str, heading: &str, lines: &[&str], right: Option<View>) -> View {
    let mut words: Vec<View> = Vec::new();
    for line in lines.iter().filter(|line| !line.is_empty()) {
        words.push(caption(line).into());
    }
    row_with(glyph, heading, StackPanel::new().children(words), right)
}

/// A row whose words under the title are given as they are.
fn row_with(glyph: &str, heading: &str, under: impl Into<View>, right: Option<View>) -> View {
    let words = StackPanel::new()
        .vertical_alignment(VerticalAlignment::Center)
        .grid_column(1)
        .children((TextBlock::new().text(heading).text_wrapping(TextWrapping::Wrap), under.into()));
    let mut cells: Vec<View> = vec![row_icon(glyph).into(), words.into()];
    if let Some(right) = right {
        cells.push(
            Border::new().grid_column(2).vertical_alignment(VerticalAlignment::Center).content(right).into(),
        );
    }
    Border::new()
        .padding(Thickness::new(16.0, 12.0, 16.0, 12.0))
        .min_height(64.0)
        .content(
            Grid::new()
                .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
                .column_spacing(16.0)
                .children(cells),
        )
        .into()
}

/// A row with a control across its width, under its title, as a long path needs.
fn row_under(glyph: &str, heading: &str, line: &str, control: View) -> View {
    let words = StackPanel::new()
        .spacing(8.0)
        .grid_column(1)
        .children((StackPanel::new().children((TextBlock::new().text(heading), caption(line))), control));
    Border::new()
        .padding(Thickness::new(16.0, 12.0, 16.0, 16.0))
        .content(
            Grid::new()
                .columns([GridLength::Auto, GridLength::STAR])
                .column_spacing(16.0)
                .children((row_icon(glyph).vertical_alignment(VerticalAlignment::Top), words)),
        )
        .into()
}

fn row_icon(glyph: &str) -> FontIcon {
    icon(glyph, 20.0).width(20.0).vertical_alignment(VerticalAlignment::Center)
}

/// A row whose text can be copied, as a guest's sign-in is.
fn copy_row(glyph: &str, heading: &str, text: &str, context: &ViewContext<Main>) -> View {
    let copy = button("Copy", glyph::COPY, context.message(Message::Copy(text.to_owned())), false)
        .automation_name(format!("Copy {heading}"));
    let text = TextBlock::new()
        .text(text)
        .font_family(MONO)
        .foreground(ThemeBrush::SecondaryText)
        .is_text_selection_enabled(true)
        .text_wrapping(TextWrapping::Wrap);
    row_with(glyph, heading, text, Some(copy.into()))
}

/// Lines of a log, newest at the foot, which the view keeps in sight as lines come.
fn log(lines: &[Line], empty: &str) -> View {
    let shown: Vec<View> =
        if lines.is_empty() { vec![caption(empty).into()] } else { lines.iter().map(log_line).collect() };
    card(
        ScrollView::new()
            .height(LOG_HEIGHT)
            // Anchored at the foot, so a new line keeps the newest in view while the person
            // hasn't scrolled up.
            .vertical_anchor_ratio(1.0)
            .content(
                StackPanel::new().spacing(4.0).margin(Thickness::new(16.0, 12.0, 16.0, 12.0)).children(shown),
            ),
    )
    .into()
}

/// A line of a log, in a fixed-width font. A problem has a mark before it and is in red; what the
/// guest wrote is greyed, as it's the guest's and not the service's.
fn log_line(line: &Line) -> View {
    let (prefix, brush) = match line.kind {
        Kind::Note => ("", ThemeBrush::PrimaryText),
        Kind::Problem => ("", ThemeBrush::SystemCritical),
        Kind::Guest => ("guest: ", ThemeBrush::SecondaryText),
        Kind::Report => ("report: ", ThemeBrush::SecondaryText),
    };
    let text = TextBlock::new()
        .text(format!("{prefix}{}", line.text))
        .font_family(MONO)
        .font_size(12.0)
        .foreground(brush)
        .is_text_selection_enabled(true)
        .text_wrapping(TextWrapping::Wrap)
        .grid_column(1);
    let mark: View = if line.kind == Kind::Problem {
        icon(glyph::PROBLEM, 12.0)
            .foreground(ThemeBrush::SystemCritical)
            .vertical_alignment(VerticalAlignment::Top)
            .margin(Thickness::new(0.0, 2.0, 0.0, 0.0))
            .automation_name("Problem")
            .into()
    } else {
        Border::new().into()
    };
    let text: View =
        if line.kind == Kind::Problem { text.font_weight(FontWeight::SEMI_BOLD).into() } else { text.into() };
    Grid::new()
        .columns([GridLength::Pixel(16.0), GridLength::STAR])
        .column_spacing(6.0)
        .children((mark, text))
        .into()
}
