//! What the window shows: the machines in the navigation on the left, and on the right the chosen
//! machine's page or the new machine's form. It's drawn from `Main` each time something changes, and
//! holds nothing of its own.
//!
//! It's laid out as Windows 11's own apps are: Mica under everything, the title bar drawn by the app,
//! and each page a column of sections, each a heading over a card of rows. Every colour is one of
//! WinUI's theme brushes, so light and dark both follow Windows.

use std::rc::Rc;

use guesthouse_control::client::Unreached;
use guesthouse_control::report::{KEPT_DAYS, ProblemKind, stage::size_words};
use guesthouse_control::{Card, CardInfo, Machine, NetworkMode, State, System};
use windows_reactor::*;

use crate::app::{
    self, Account, ConsoleState, DELETE_KEY, Draft, EDITIONS, FolderFor, Kind, Line, Main, Message, NEW_TAG,
    Page, REPORT_TAG, Ready, ReportStage,
};

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
    pub const DOWNLOAD: &str = "\u{E896}";
    pub const SHIELD: &str = "\u{EA18}";
    pub const NETWORK: &str = "\u{E968}";
    pub const SHARE: &str = "\u{E8F7}";
    pub const PERSON: &str = "\u{E77B}";
    pub const GROW: &str = "\u{E740}";
    pub const SNAPSHOT: &str = "\u{E722}";
    pub const BACK: &str = "\u{E7A7}";
    pub const FEEDBACK: &str = "\u{E939}";
    pub const DOCUMENT: &str = "\u{E8A5}";
    pub const EDIT: &str = "\u{E70F}";
    pub const MAIL: &str = "\u{E715}";
    pub const SEND: &str = "\u{E724}";
    pub const OPEN: &str = "\u{E8A7}";
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
        Err(_) if main.page == Page::Report => report_page(main, context, true),
        Err(words) => away(words, context),
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
    match (&main.deleting, &main.snapshot_ask) {
        (Some(name), _) => content.content_dialog(delete_dialog(name, context)),
        (None, Some(ask)) => content.content_dialog(snapshot_dialog(ask, context)),
        (None, None) if main.bridge_ask => content.content_dialog(bridge_dialog(main, context)),
        (None, None) => content.into(),
    }
}

fn logo() -> EncodedImage {
    EncodedImage::from_static(LOGO_SVG)
}

// ---- Where the service can't be asked ----

/// In place of everything else when the service can't be asked.
fn away(words: &str, context: &ViewContext<Main>) -> View {
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
        button("Report a problem", glyph::FEEDBACK, context.message(Message::OpenReport), false)
            .horizontal_alignment(HorizontalAlignment::Center)
            .margin(Thickness::new(0.0, 8.0, 0.0, 0.0))
            .into(),
    );
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
        Page::New | Page::Report => None,
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
    let report = NavigationViewItem::new()
        .tag(REPORT_TAG)
        .icon(Icon::font(glyph::FEEDBACK))
        .content("Report a problem")
        .is_selected(main.page == Page::Report);
    let page: View = match (main.page, main.machine()) {
        (Page::New, _) => new_page(main, context),
        (Page::Report, _) => report_page(main, context, false),
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
        .footer_menu_items((new, report))
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

/// A newer Windows host, at the top of a page, until it's put aside.
fn update_card(main: &Main, context: &ViewContext<Main>) -> Option<View> {
    let latest = main.update.as_ref()?;
    let words = format!(
        "You have {}. Download the installer and run it: it keeps your machines, and asks each running one to shut down first.",
        guesthouse_control::WINDOWS_VERSION
    );
    let download = button("Download", glyph::DOWNLOAD, context.message(Message::GetUpdate), true)
        .automation_name(format!("Download Guesthouse {}", latest.version));
    let later = Button::new().content("Not now").on_click(context.message(Message::LaterUpdate));
    let buttons =
        StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children((download, later));
    Some(
        card(row(
            glyph::DOWNLOAD,
            &format!("Guesthouse {} is out", latest.version),
            &[&words],
            Some(buttons.into()),
        ))
        .into(),
    )
}

fn machine_page(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let mut sections: Vec<View> = update_card(main, context).into_iter().collect();
    sections.push(header(machine, context));
    sections.push(group("Status", status_rows(machine)));
    sections.push(group("Connect", connect_rows(main, machine, context)));
    if let Some(draft) = main.draft.as_ref().filter(|draft| draft.machine == machine.name) {
        sections.push(settings_section(main, machine, draft, context));
    }
    sections.push(group("Snapshots", snapshot_rows(main, machine, context)));
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
        .is_enabled(state == State::Stopped && machine.working.is_none());
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
    let mut rows = vec![row(glyph::POWER, app::state_words(state), &[line], ring)];
    if let Some(doing) = &machine.working {
        let ring = ProgressRing::new().is_active(true).width(20.0).height(20.0);
        rows.push(row(
            glyph::SNAPSHOT,
            doing,
            &["Nothing else can be done to it until this is done."],
            Some(ring.into()),
        ));
    }
    rows.push(row(glyph::SETUP, "Setup", &[&setup_words(machine)], None));
    rows
}

/// The machine's snapshots, and taking another. Each is of a stopped machine that has been set up.
fn snapshot_rows(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> Vec<View> {
    let free = machine.state == State::Stopped && machine.working.is_none();
    let take_words = if !machine.set_up {
        "A machine has snapshots once its first boot has set it up."
    } else if machine.state != State::Stopped {
        "Stop it to take a snapshot. A snapshot keeps its disk, its firmware and its TPM as they are."
    } else {
        "Keeps its disk, its firmware and its TPM as they are now, to go back to. Its disk can't grow while it has snapshots."
    };
    let label = TextBox::new()
        .placeholder_text("A label, such as Before the update")
        .text(main.snapshot_label.as_str())
        .is_enabled(free && machine.set_up)
        .width(280.0)
        .automation_name("Snapshot label")
        .on_text_changed(context.callback(|text: Rc<str>| Message::SnapshotLabel(text.to_string())));
    let take = button("Take", glyph::SNAPSHOT, context.message(Message::TakeSnapshot), false)
        .is_enabled(free && machine.set_up && !main.snapshot_label.trim().is_empty())
        .automation_name("Take a snapshot");
    let right = StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children((label, take));
    let mut rows = vec![row(glyph::SNAPSHOT, "Take a snapshot", &[take_words], Some(right.into()))];
    for snapshot in machine.snapshots.iter().rev() {
        let mut lines = vec![crate::system::local_time(snapshot.taken)];
        if snapshot.current {
            lines.push("It runs on from here.".to_owned());
        }
        let back =
            button("Go back", glyph::BACK, context.message(Message::RevertSnapshot(snapshot.id)), false)
                .is_enabled(free)
                .automation_name(format!("Go back to {}", snapshot.label));
        let delete =
            button("Delete", glyph::DELETE, context.message(Message::DeleteSnapshot(snapshot.id)), false)
                .is_enabled(free)
                .automation_name(format!("Delete the snapshot {}", snapshot.label));
        let buttons =
            StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children((back, delete));
        let words: Vec<&str> = lines.iter().map(String::as_str).collect();
        rows.push(row(glyph::SNAPSHOT, &snapshot.label, &words, Some(buttons.into())));
    }
    rows
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
        System::Linux => match account {
            Some(account) => {
                let lines = app::ssh_lines(machine, &account.user);
                for (heading, line) in &lines {
                    rows.push(copy_row(glyph::TERMINAL, heading, line, context));
                }
                if lines.is_empty() {
                    rows.push(row(glyph::TERMINAL, "SSH", &[&where_words], None));
                }
            }
            None => rows.push(row(glyph::TERMINAL, "SSH", &[SSH_NOTE, &where_words], None)),
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

/// What an Ubuntu guest's two ways of SSH are, shown where its user isn't yet.
const SSH_NOTE: &str = "While it runs, its SSH is also on a port of this PC's own loopback, which needs no network. \
                        Show the user and password to see the lines. The port is open to any user of this PC, and \
                        the guest still asks for its own password.";

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
            // An Ubuntu guest's files disk was made for its card's driver, so the service keeps its card.
            &[if machine.system == System::Linux { CARD_KEPT_NOTE } else { CARD_NOTE }],
            Some(card_box(
                main,
                &main.settings_cards(),
                &draft.card,
                open && machine.system == System::Windows,
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
    rows.push(row(
        glyph::SHIELD,
        "TPM",
        &[tpm_words(Some(machine.system))],
        Some(toggle("TPM", draft.tpm, open, context.callback(Message::SetTpm))),
    ));
    rows.push(row(
        glyph::PROCESSOR,
        "Nested virtualization",
        &[nested_words(machine.system)],
        Some(toggle(
            "Nested virtualization",
            draft.nested_virtualization,
            open,
            context.callback(Message::SetNested),
        )),
    ));
    rows.push(row(
        glyph::NETWORK,
        "Network",
        &["Through Windows' own network for its machines. Without it the guest has no address to reach it on."],
        Some(toggle("Network", draft.network, open, context.callback(Message::SetNetwork))),
    ));
    rows.push(network_row(main, draft, open && draft.network, context));
    rows.push(forwards_row(draft, open && draft.network, context));
    rows.push(shares_row(machine.system, draft, open, context));
    rows.push(disk_row(machine, draft, stopped, context));

    let changed = draft.changed(machine);
    let save = button("Save", glyph::SAVE, context.message(Message::SaveSettings), true)
        .is_enabled(open && changed && draft.settings().is_some() && draft.problem().is_none());
    let undo = button("Undo", glyph::UNDO, context.message(Message::UndoSettings), false)
        .is_enabled(open && (changed || draft.refused.is_some()));
    let mut foot: Vec<View> = vec![save.into(), undo.into()];
    let problem = draft.problem();
    let (words, bad) = match (&draft.refused, stopped) {
        (Some(reason), _) => (Some(reason.as_str()), true),
        (None, false) => (Some("Its settings change while it's off. Stop it first."), false),
        (None, true) if draft.saving => (Some("Saving."), false),
        (None, true) if changed && draft.settings().is_none() => {
            (Some("Give it memory and processors, and both ports of each forward, first."), false)
        }
        (None, true) if changed && problem.is_some() => (problem.as_deref(), true),
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

/// Which network the machine's card is on, with the adapters it could be bridged onto.
fn network_row(main: &Main, draft: &Draft, enabled: bool, context: &ViewContext<Main>) -> View {
    let choices = main.network_choices();
    let names: Vec<String> = choices.iter().map(|mode| network_words(mode, main)).collect();
    let list = ComboBox::new()
        .items_source(names)
        .selected_index(choices.iter().position(|mode| *mode == draft.network_mode))
        .is_enabled(enabled)
        .width(360.0)
        .automation_name("Connects through")
        .on_selection_changed(context.callback(Message::SetNetworkMode));
    let mut lines = vec![SHARED_NOTE.to_owned(), BRIDGED_NOTE.to_owned()];
    match main.bridging() {
        Some(bridging) if !bridging.available => {
            lines.push(
                bridging.reason.clone().unwrap_or_else(|| "This PC can't bridge a machine.".to_owned()),
            );
        }
        Some(bridging) if bridging.adapters.is_empty() => {
            lines.push(
                "No Ethernet or Wi-Fi adapter of this PC's is up, so there is nothing to bridge onto."
                    .to_owned(),
            );
        }
        _ => {}
    }
    if let NetworkMode::Bridged { adapter } = &draft.network_mode {
        let wifi = main.bridging().is_some_and(|bridging| {
            bridging.adapters.iter().any(|known| known.name == *adapter && known.wifi)
        });
        if wifi {
            lines.push(
                "This is a Wi-Fi adapter. Many Wi-Fi adapters, and routers, refuse a second address on one \
                 connection, so the machine may not start, or get no address. Ethernet always can."
                    .to_owned(),
            );
        }
    }
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    row(glyph::NETWORK, "Connects through", &lines, Some(list.into()))
}

/// The shared network, which every edition has.
const SHARED_NOTE: &str = "Shared: Windows' own network for its machines. The guest has a private address that only \
                           this PC reaches; forward ports below to reach it from other devices.";

/// Bridging, which needs Hyper-V and says what it does to this PC.
const BRIDGED_NOTE: &str = "Bridged: the guest joins this PC's network, with an address of its own from the router, \
                            so other devices reach it directly. It needs Windows' Hyper-V feature. The first time, \
                            this PC's own network can drop for a few seconds.";

/// A network, as the list says it.
fn network_words(mode: &NetworkMode, main: &Main) -> String {
    match mode {
        NetworkMode::Shared => "Shared, with a private address".to_owned(),
        NetworkMode::Bridged { adapter } => {
            let known = main
                .bridging()
                .and_then(|bridging| bridging.adapters.iter().find(|known| known.name == *adapter));
            match known {
                Some(known) if known.wifi => {
                    format!("Bridged onto {adapter} (Wi-Fi: often can't), {}", known.description)
                }
                Some(known) => format!("Bridged onto {adapter}, {}", known.description),
                None => format!("Bridged onto {adapter} (not available now)"),
            }
        }
    }
}

/// The port forwards: a row of two ports each, and one to add.
fn forwards_row(draft: &Draft, enabled: bool, context: &ViewContext<Main>) -> View {
    let bridged = matches!(draft.network_mode, NetworkMode::Bridged { .. });
    let mut rows: Vec<View> = Vec::new();
    for (place, forward) in draft.forwards.iter().enumerate() {
        let host = port_box(
            "This PC's port",
            forward.host,
            enabled,
            context.callback(move |value| Message::SetForwardHost(place, value)),
        );
        let guest = port_box(
            "Guest's port",
            forward.guest,
            enabled,
            context.callback(move |value| Message::SetForwardGuest(place, value)),
        );
        let remove = square(glyph::CLOSE, "Remove the forward")
            .is_enabled(enabled)
            .on_click(context.message(Message::RemoveForward(place)));
        let line = StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children((
            TextBlock::new().text("This PC's port").vertical_alignment(VerticalAlignment::Center),
            host,
            TextBlock::new().text("to the guest's").vertical_alignment(VerticalAlignment::Center),
            guest,
            remove,
        ));
        rows.push(line.into());
    }
    let add = button("Add a forward", glyph::ADD, context.message(Message::AddForward), false)
        .is_enabled(enabled && draft.forwards.len() < guesthouse_control::MOST_FORWARDS);
    rows.push(add.into());
    if let Some(problem) = draft.forwards_problem().filter(|_| !draft.forwards.is_empty()) {
        rows.push(caption(&problem).foreground(ThemeBrush::SystemCritical).into());
    }
    row_under(
        glyph::SHARE,
        "Port forwards",
        if bridged { BRIDGED_FORWARDS_NOTE } else { FORWARDS_NOTE },
        StackPanel::new().spacing(8.0).children(rows).into(),
    )
}

/// The shared folders: a folder's field, its Browse button, its read-only box and its remove button
/// each, and one to add.
fn shares_row(system: System, draft: &Draft, enabled: bool, context: &ViewContext<Main>) -> View {
    let mut rows: Vec<View> = Vec::new();
    for (place, share) in draft.shares.iter().enumerate() {
        let field = TextBox::new()
            .placeholder_text("A folder of this PC's to share")
            .text(share.folder.as_str())
            .is_enabled(enabled)
            .automation_name("Shared folder")
            .on_text_changed(
                context.callback(move |text: Rc<str>| Message::SetShareFolder(place, text.to_string())),
            );
        let browse = button(
            "Browse…",
            glyph::FOLDER,
            context.message(Message::BrowseFolder(FolderFor::Machine(place))),
            false,
        )
        .is_enabled(enabled);
        let read_only = CheckBox::new()
            .content("Read-only")
            .is_checked(share.read_only)
            .is_enabled(enabled)
            .vertical_alignment(VerticalAlignment::Center)
            .on_is_checked_changed(
                context.callback(move |checked| Message::SetShareReadOnly(place, checked)),
            );
        let remove = square(glyph::CLOSE, "Remove the shared folder")
            .is_enabled(enabled)
            .on_click(context.message(Message::RemoveShare(place)));
        let line = Grid::new()
            .columns([GridLength::STAR, GridLength::Auto, GridLength::Auto, GridLength::Auto])
            .column_spacing(8.0)
            .children((field, browse.grid_column(1), read_only.grid_column(2), remove.grid_column(3)));
        rows.push(line.into());
    }
    let add = button("Add a folder", glyph::ADD, context.message(Message::AddShare), false)
        .is_enabled(enabled && draft.shares.len() < guesthouse_control::MOST_SHARES);
    rows.push(add.into());
    if let Some(problem) = draft.shares_problem().filter(|_| !draft.shares.is_empty()) {
        rows.push(caption(&problem).foreground(ThemeBrush::SystemCritical).into());
    }
    row_under(
        glyph::SHARE,
        "Shared folders",
        shares_words(system),
        StackPanel::new().spacing(8.0).children(rows).into(),
    )
}

/// Where a machine's shared folders are in the guest.
fn shares_words(system: System) -> &'static str {
    match system {
        System::Windows => {
            "Each is a drive of its own from S:, linked on the guest's desktop as Shared folder, Shared folder 2 \
             and on, from its next start. The machine's own account is let into each folder. At most 8."
        }
        System::Linux => {
            "At /mnt/shared in the guest, then /mnt/shared2 and on, from its next start. The machine's own \
             account is let into each folder. At most 8."
        }
    }
}

/// What a forward does, which needs the shared network and a running machine.
const FORWARDS_NOTE: &str = "While the machine runs, a TCP port of this PC's leads to a port of the guest's, so other \
                             devices on this network reach it at this PC's address. It also opens that port in \
                             Windows Firewall, to this network only, and closes both when the machine stops. Needs \
                             Windows' IP Helper service. At most 16.";

const BRIDGED_FORWARDS_NOTE: &str = "Not made while it's bridged: the guest has an address of its own on the network, \
                                     and its ports are reached there.";

/// A box for a port.
fn port_box(name: &str, value: Option<f64>, enabled: bool, changed: Callback<Option<f64>>) -> NumberBox {
    NumberBox::new()
        .value(value)
        .minimum(1.0)
        .maximum(65535.0)
        .is_enabled(enabled)
        .width(130.0)
        .automation_name(name)
        .on_value_changed(changed)
}

/// What a card is to the guest, under the card's list.
const CARD_NOTE: &str = "The card the guest shares with this PC. Default is whichever has the most memory of its own \
                         when it boots.";

/// Why an Ubuntu guest's card can't change, under the card's list.
const CARD_KEPT_NOTE: &str = "The card the guest shares with this PC. An Ubuntu guest's files disk was made for its \
                              card's driver, so it keeps the card it was made with.";

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

/// The disk's size, which only grows, and is asked for on its own as it can't be undone.
fn disk_row(machine: &Machine, draft: &Draft, stopped: bool, context: &ViewContext<Main>) -> View {
    let now = match machine.size_gb {
        Some(gb) => format!("Now {gb} GB at most, and it only takes what the guest writes."),
        None => "Its size isn't known to the service.".to_owned(),
    };
    let field = NumberBox::new()
        .value(draft.size_gb)
        .minimum(1.0)
        .small_change(16.0)
        .spin_button_placement_mode(NumberBoxSpinButtonPlacementMode::Inline)
        .is_enabled(stopped)
        .width(180.0)
        .automation_name("Disk (GB)")
        .on_value_changed(context.callback(Message::SetSize));
    let grow = button("Grow", glyph::GROW, context.message(Message::Grow), false)
        .is_enabled(stopped && machine.working.is_none() && draft.growth(machine).is_some())
        .automation_name("Grow the disk");
    let right = StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children((field, grow));
    row(
        glyph::DISK,
        "Disk (GB)",
        &[&now, "A disk only grows. The guest's own partition takes the room at its next start."],
        Some(right.into()),
    )
}

/// What a TPM is for, for a system or for either.
fn tpm_words(system: Option<System>) -> &'static str {
    match system {
        Some(System::Linux) => "Only a Windows guest is given one, so this changes nothing for Ubuntu.",
        _ => {
            "Windows 11 asks for one to set up. Without it, BitLocker and Windows Hello in the guest have none."
        }
    }
}

/// What nested virtualization lets a guest do, and what of it hasn't been tried.
fn nested_words(system: System) -> &'static str {
    match system {
        System::Windows => {
            "Lets the guest run virtual machines of its own: WSL, Docker, Windows Sandbox. With it on, a guest \
             with Secure Boot and a TPM may turn on memory integrity, which runs Windows' hypervisor inside the \
             guest. How that sits with GPU-PV hasn't been tried yet. Takes effect when the machine starts."
        }
        System::Linux => {
            "Lets the guest run virtual machines of its own, such as KVM and Docker's. Takes effect when the \
             machine starts."
        }
    }
}

/// Where the shared folder shows in the guest, for a system or for either.
fn share_words(system: Option<System>) -> &'static str {
    match system {
        Some(System::Windows) => {
            "On the guest's desktop as Shared folder, from its next start. The machine's own account is let into the folder."
        }
        Some(System::Linux) => {
            "At /mnt/shared in the guest, from its next start. The machine's own account is let into the folder."
        }
        None => {
            "On a Windows guest's desktop as Shared folder, and at /mnt/shared in Ubuntu. The machine's own account is let into the folder."
        }
    }
}

fn toggle(name: &str, on: bool, enabled: bool, toggled: Callback<bool>) -> View {
    ToggleSwitch::new().is_on(on).is_enabled(enabled).automation_name(name).on_toggled(toggled).into()
}

/// A folder's field with its Browse button, and its read-only box under them.
fn share_view(field: TextBox, browse: Button, read_only: CheckBox) -> View {
    let line = Grid::new()
        .columns([GridLength::STAR, GridLength::Auto])
        .column_spacing(8.0)
        .children((field, browse.grid_column(1)));
    StackPanel::new().spacing(8.0).children((line, read_only)).into()
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
    let user_ok = form.user.trim().is_empty() || guesthouse_control::valid_user(form.user.trim());
    let user = TextBox::new()
        .placeholder_text(guesthouse_control::DEFAULT_USER)
        .text(form.user.as_str())
        .width(280.0)
        .is_enabled(!busy)
        .automation_name("User")
        .on_text_changed(context.callback(|text: Rc<str>| Message::User(text.to_string())));
    let user_row = row_with(
        glyph::PERSON,
        "User",
        caption(if user_ok {
            "The guest's administrator. Its password is made for it."
        } else {
            "Up to 20 lowercase letters, digits, - and _, starting with a letter, and not one of Windows' own."
        })
        .foreground(if user_ok { ThemeBrush::SecondaryText } else { ThemeBrush::SystemCritical }),
        Some(user.into()),
    );
    let editions: Vec<String> =
        EDITIONS
            .iter()
            .map(|edition| {
                if edition.is_empty() {
                    "Pro, or the image's first".to_owned()
                } else {
                    (*edition).to_owned()
                }
            })
            .collect();
    let edition = ComboBox::new()
        .items_source(editions)
        .selected_index(Some(form.edition))
        .is_enabled(!busy)
        .width(280.0)
        .automation_name("Edition")
        .on_selection_changed(context.callback(Message::Edition));
    let edition_row = row(
        glyph::WINDOWS,
        "Edition",
        &["Which of a Windows image's editions to install. An Ubuntu image has one."],
        Some(edition.into()),
    );
    sections.push(group("Machine", vec![name_row, image_row, user_row, edition_row]));

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

    sections.push(group(
        "Devices",
        vec![
            row(glyph::SHIELD, "TPM", &[tpm_words(None)], Some(toggle("TPM", form.tpm, !busy, context.callback(Message::Tpm)))),
            row(
                glyph::NETWORK,
                "Network",
                &["Through Windows' own network for its machines. Without it the guest has no address to reach it on."],
                Some(toggle("Network", form.network, !busy, context.callback(Message::Network))),
            ),
            row_under(
                glyph::SHARE,
                "Shared folder",
                share_words(None),
                share_view(
                    TextBox::new()
                        .placeholder_text("None. A folder of this PC's to share")
                        .text(form.share_folder.as_str())
                        .is_enabled(!busy)
                        .automation_name("Shared folder")
                        .on_text_changed(context.callback(|text: Rc<str>| Message::ShareFolder(text.to_string()))),
                    button("Browse…", glyph::FOLDER, context.message(Message::BrowseFolder(FolderFor::Form)), false)
                        .is_enabled(!busy),
                    CheckBox::new()
                        .content("Read-only")
                        .is_checked(form.share_read_only)
                        .is_enabled(!busy)
                        .on_is_checked_changed(context.callback(Message::ShareReadOnly)),
                ),
            ),
        ],
    ));

    let ready = form.machine().is_some() && !busy;
    let mut foot: Vec<View> =
        vec![button("Make it", glyph::ADD, context.message(Message::Create), true).is_enabled(ready).into()];
    if !ready && !busy {
        foot.push(
            caption("Give it a name and an image first, and a user the rules above take if you name one.")
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
    if let Some(update) = update_card(main, context) {
        sections.insert(0, update);
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

fn snapshot_dialog(ask: &app::SnapshotAsk, context: &ViewContext<Main>) -> ContentDialog {
    let (title, words, button) = if ask.revert {
        (
            format!("Go back to {}?", ask.label),
            format!(
                "{} gets its disk, its firmware and its TPM as they were in this snapshot. What it holds now and \
                 hasn't kept in a snapshot is gone, and that can't be undone.",
                ask.machine
            ),
            "Go back",
        )
    } else {
        (
            format!("Delete the snapshot {}?", ask.label),
            "It can't be gone back to after this. Its space comes back once no other snapshot, or the machine \
             itself, is made on it."
                .to_owned(),
            "Delete",
        )
    };
    ContentDialog::new()
        .title(title)
        .content(TextBlock::new().text(words).text_wrapping(TextWrapping::Wrap))
        .primary_button_text(button)
        .close_button_text("Cancel")
        .default_button(ContentDialogButton::Close)
        .is_open(true)
        .on_closed(context.callback(Message::SnapshotAnswered))
}

/// Asks before a machine is first bridged, as Windows can drop this PC's own network for a few
/// seconds while it binds its virtual switch to the adapter.
fn bridge_dialog(main: &Main, context: &ViewContext<Main>) -> ContentDialog {
    let adapter = match main.draft.as_ref().map(|draft| &draft.network_mode) {
        Some(NetworkMode::Bridged { adapter }) => adapter.as_str(),
        _ => "the adapter",
    };
    ContentDialog::new()
        .title(format!("Bridge the machine onto {adapter}?"))
        .content(
            TextBlock::new()
                .text(format!(
                    "The first time a machine starts bridged, Windows binds a virtual switch to {adapter}. This \
                     PC's own network, and anything running on it, can drop for a few seconds. Many Wi-Fi \
                     adapters can't be bridged at all. Save it only when nothing on this PC needs the network \
                     for the next minute."
                ))
                .text_wrapping(TextWrapping::Wrap),
        )
        .primary_button_text("Save")
        .close_button_text("Cancel")
        .default_button(ContentDialogButton::Close)
        .is_open(true)
        .on_closed(context.callback(Message::BridgeAnswered))
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

// ---- Report a problem ----

/// What goes in a report, and what never does, said before it's made.
const REPORT_ABOUT: &str = "A report is what the developer needs to see what went wrong: the service's log, this PC's \
     Windows version and whether the services Guesthouse needs are running, the Hyper-V events about \
     Guesthouse's machines, and for a machine you pick, its settings, its snapshots' list and its events. \
     Never a guest's password or screen key, its disks, or what it holds.";

/// The report page. `standalone` is when the service can't be reached, so the page has no navigation
/// beside it and says how to go back.
fn report_page(main: &Main, context: &ViewContext<Main>, standalone: bool) -> View {
    let report = &main.report;
    let mut sections: Vec<View> = Vec::new();
    if standalone {
        sections.push(
            button("Back", glyph::BACK, context.message(Message::CloseReport), false)
                .automation_name("Back")
                .into(),
        );
    }
    sections.push(
        StackPanel::new()
            .spacing(4.0)
            .children((
                title("Report a problem"),
                caption(
                    "Prepare makes the report and shows you what's in it. Nothing is sent until you press Send.",
                ),
            ))
            .into(),
    );
    match &report.stage {
        ReportStage::Writing { problem } => report_form(main, problem.as_deref(), context, &mut sections),
        ReportStage::Preparing => sections.push(
            StackPanel::new()
                .spacing(HEADING_GAP)
                .children((
                    strong("Making the report"),
                    ProgressBar::new().is_indeterminate(true),
                    caption("Asking the service for its files, taking out names, and packing them."),
                ))
                .into(),
        ),
        ReportStage::Ready(ready) => report_ready(ready, context, &mut sections),
    }
    page(sections)
}

fn report_form(main: &Main, problem: Option<&str>, context: &ViewContext<Main>, sections: &mut Vec<View>) {
    let report = &main.report;
    let names: Vec<&str> = main
        .status()
        .map(|status| status.machines.iter().map(|machine| machine.name.as_str()).collect())
        .unwrap_or_default();
    let mut machines: Vec<String> = vec!["None, the PC itself".to_owned()];
    machines.extend(names.iter().map(|name| (*name).to_owned()));
    let chosen = report
        .machine
        .as_deref()
        .and_then(|machine| names.iter().position(|name| *name == machine))
        .map_or(0, |at| at + 1);
    let machine = ComboBox::new()
        .items_source(machines)
        .selected_index(Some(chosen))
        .width(280.0)
        .automation_name("Machine")
        .on_selection_changed(context.callback(Message::ReportMachine));
    let kinds: Vec<String> = ProblemKind::ALL.iter().map(|kind| kind.label().to_owned()).collect();
    let kind = ComboBox::new()
        .items_source(kinds)
        .selected_index(ProblemKind::ALL.iter().position(|kind| *kind == report.kind))
        .width(280.0)
        .automation_name("Kind of problem")
        .on_selection_changed(context.callback(Message::ReportKind));
    let what = TextBox::new()
        .placeholder_text("What you did, what you expected, and what happened instead")
        .text(report.what.as_str())
        .accepts_return(true)
        .text_wrapping(TextWrapping::Wrap)
        .height(140.0)
        .automation_name("What went wrong")
        .on_text_changed(context.callback(|text: Rc<str>| Message::ReportWhat(text.to_string())));
    let contact = TextBox::new()
        .placeholder_text("An email or a name, if you'd like an answer")
        .text(report.contact.as_str())
        .width(280.0)
        .automation_name("Contact")
        .on_text_changed(context.callback(|text: Rc<str>| Message::ReportContact(text.to_string())));
    sections.push(group(
        "The problem",
        vec![
            row(
                glyph::SETUP,
                "Machine",
                &["The machine it's about, if it's about one. Its settings and events go in the report."],
                Some(machine.into()),
            ),
            row(glyph::PROBLEM, "Kind", &[], Some(kind.into())),
            row_under(glyph::EDIT, "What went wrong", "In your own words.", what.into()),
            row(
                glyph::MAIL,
                "Contact",
                &["Optional. It goes to the developer with the report, and not in a public issue."],
                Some(contact.into()),
            ),
        ],
    ));
    sections.push(caption(&format!("{REPORT_ABOUT} It's kept {KEPT_DAYS} days.")).into());
    if let Some(problem) = problem {
        sections.push(
            InfoBar::new()
                .is_open(true)
                .is_closable(false)
                .severity(InfoBarSeverity::Error)
                .title("The report wasn't made")
                .message(problem)
                .into(),
        );
    }
    sections.push(
        button("Prepare", glyph::FEEDBACK, context.message(Message::ReportPrepare), true)
            .horizontal_alignment(HorizontalAlignment::Left)
            .into(),
    );
}

fn report_ready(ready: &Ready, context: &ViewContext<Main>, sections: &mut Vec<View>) {
    let prepared = &ready.prepared;
    let sent = ready.id.is_some();
    let file_words = format!("{} files, {} packed.", prepared.files.len(), size_words(prepared.bytes));
    let intro = if sent {
        "It's sent."
    } else {
        "This is everything in the report. Send posts it to the developer, who reads it and deletes it \
         after 30 days. Save puts a copy where you choose, to attach to an issue yourself."
    };
    sections.push(StackPanel::new().spacing(4.0).children((strong(&file_words), caption(intro))).into());
    let files: Vec<View> = prepared
        .files
        .iter()
        .map(|(name, bytes)| row(glyph::DOCUMENT, name, &[&size_words(*bytes as u64)], None))
        .collect();
    sections.push(group("In the report", files));
    if !prepared.missing.is_empty() {
        let missing: Vec<View> = prepared
            .missing
            .iter()
            .map(|(name, reason)| row(glyph::WARNING, name, &[reason], None))
            .collect();
        sections.push(group("Not in it", missing));
    }
    if let Some(id) = &ready.id {
        sections.push(
            InfoBar::new()
                .is_open(true)
                .is_closable(false)
                .severity(InfoBarSeverity::Success)
                .title("Sent")
                .message(format!(
                    "The report's ID is {id}. Give it to the developer, or open an issue with it."
                ))
                .into(),
        );
        sections.push(
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(8.0)
                .children((
                    TextBlock::new()
                        .text(id.as_str())
                        .font_family(MONO)
                        .is_text_selection_enabled(true)
                        .vertical_alignment(VerticalAlignment::Center),
                    button("Copy", glyph::COPY, context.message(Message::Copy(id.clone())), false)
                        .automation_name("Copy the report's ID"),
                    button("Open an issue", glyph::OPEN, context.message(Message::ReportIssue), true),
                ))
                .into(),
        );
        sections.push(
            caption("The issue holds only the ID and what sorts the report, as an issue is public.").into(),
        );
    }
    if let Some(problem) = &ready.problem {
        sections.push(
            InfoBar::new()
                .is_open(true)
                .is_closable(false)
                .severity(InfoBarSeverity::Error)
                .title("That didn't work")
                .message(problem.as_str())
                .into(),
        );
    }
    if let Some(saved) = &ready.saved {
        sections.push(caption(&format!("A copy is saved as {saved}.")).into());
    }
    let mut buttons: Vec<View> = Vec::new();
    if !sent {
        buttons.push(
            button("Send", glyph::SEND, context.message(Message::ReportSend), true)
                .is_enabled(!ready.sending)
                .into(),
        );
    }
    buttons.push(
        button("Save…", glyph::SAVE, context.message(Message::ReportSave), false)
            .is_enabled(!ready.sending)
            .into(),
    );
    buttons.push(
        button("Start over", glyph::UNDO, context.message(Message::ReportStartOver), false)
            .is_enabled(!ready.sending)
            .into(),
    );
    if ready.sending {
        buttons.push(ProgressRing::new().is_active(true).width(20.0).height(20.0).into());
        buttons.push(caption("Sending.").vertical_alignment(VerticalAlignment::Center).into());
    }
    sections
        .push(StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children(buttons).into());
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
