//! A machine's page: its part of the title bar (the switcher, and what can be done to it), the bar
//! of its sections, and its Overview: where it stands, its facts, how to reach it, and its log.

use std::rc::Rc;

use guesthouse_control::{Card, Machine, NetworkMode, State, System};
use windows_reactor::*;

use super::parts::*;
use super::{remote_view, settings};
use crate::app::{self, Account, ConsoleState, DELETE_KEY, Main, Message, NEW_TAG, REPORT_TAG, Section};
use crate::palette;
use crate::remote_view::proposed;

/// A machine's memory, as it was given: in GB when it's a whole number of them, and in MB otherwise.
pub use guesthouse_basics::words::memory_as_given as memory_words;

/// The widest the Overview is. Wider, its cards get hard to read across.
const OVERVIEW_WIDTH: f64 = 1400.0;
/// The big panel's least height, where the machine's screen would be.
const HERO_HEIGHT: f64 = 300.0;
const HERO_HEIGHT_NARROW: f64 = 220.0;
/// The size of the app's mark in the big panel, while there's no picture of the screen.
const HERO_MARK: f64 = 120.0;

pub fn page(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let section = if main.section.offered(machine.system) { main.section } else { Section::Overview };
    let items: Vec<KeyedView> = Section::ALL
        .into_iter()
        .filter(|item| item.offered(machine.system))
        .map(|item| {
            keyed(item.label(), SelectorBarItem::new().text(item.label()).is_selected(item == section))
        })
        .collect();
    let bar = SelectorBar::new()
        .keyed_items(items)
        .automation_name("Sections")
        .margin(Thickness::new(SIDE - 12.0, 0.0, SIDE, 4.0))
        .on_selected_text_changed(
            context.callback(|text: Option<Rc<str>>| Message::Section(text.map(|text| text.to_string()))),
        );
    let body = match section {
        Section::Overview => overview(main, machine, context),
        Section::RemoteView => remote_view::section(main, context),
        Section::Hardware => settings::hardware(main, machine, context),
        Section::Network => settings::network(main, machine, context),
        Section::Storage => settings::storage(main, machine, context),
    };
    let mut parts: Vec<View> = vec![bar.grid_row(0).into(), Border::new().grid_row(1).content(body).into()];
    if section.has_settings()
        && let Some(draft) = main.draft.as_ref().filter(|draft| draft.machine == machine.name)
    {
        parts.push(Border::new().grid_row(2).content(settings::save_bar(machine, draft, context)).into());
    }
    Grid::new().rows([GridLength::Auto, GridLength::STAR, GridLength::Auto]).children(parts).into()
}

/// What a machine that needs an owner says, on its card and its page.
pub const NEEDS_AN_OWNER: &str = "Needs an owner";

/// The page of a machine that needs an owner: what that means, and Make it mine, which Windows asks
/// an administrator of this PC to allow.
pub fn needs_owner(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let asking = main.assigning.as_deref() == Some(machine.name.as_str());
    let mut parts: Vec<View> = vec![
        icon(glyph::PERSON, 48.0).foreground(ThemeBrush::TertiaryText).into(),
        subtitle(&format!("{} needs an owner", machine.name)).text_alignment(TextAlignment::Center).into(),
        centred(
            "Each machine is the user's who made it. This one was made before Guesthouse kept that, and \
             nothing says whose it is. Until it has an owner nobody can start it, change it or see more \
             of it here.",
        ),
        centred(
            "Make it mine gives it to you. Windows asks an administrator of this PC to allow that, as \
             another user may have made it.",
        ),
        button("Make it mine", context.message(Message::MakeMine), true)
            .is_enabled(!asking)
            .horizontal_alignment(HorizontalAlignment::Center)
            .margin(Thickness::new(0.0, 8.0, 0.0, 0.0))
            .into(),
    ];
    if asking {
        parts.push(
            side_by_side((ring(16.0), caption("Waiting for Windows' prompt to be answered.")))
                .horizontal_alignment(HorizontalAlignment::Center)
                .into(),
        );
    }
    one_thing(parts)
}

// ---- The title bar's part ----

/// The machine's name, with the dot of where it stands, which opens a menu of every machine to
/// switch to, and New machine. Beside it, how many are running, when there's room.
pub fn switcher(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let machines = main.status().map(|status| status.machines.as_slice()).unwrap_or_default();
    let mut items: Vec<MenuItem> = machines
        .iter()
        .map(|other| {
            MenuItem::radio(
                other.name.as_str(),
                format!("{} · {}", other.name, other.state.name()),
                "machines",
                other.name == machine.name,
            )
            .glyph(system_glyph(other.system))
        })
        .collect();
    items.push(MenuItem::separator(" separator"));
    items.push(MenuItem::item(NEW_TAG, "New machine").glyph(glyph::ADD));
    let name = TextBlock::new()
        .text(machine.name.as_str())
        .font_weight(FontWeight::SEMI_BOLD)
        .text_trimming(TextTrimming::CharacterEllipsis)
        .max_width(240.0);
    let switcher = DropDownButton::new()
        .content(side_by_side((dot(machine.state), name)))
        .automation_name(format!("Switch machine, {}", machine.name))
        .vertical_alignment(VerticalAlignment::Center)
        .menu(Menu::new(
            items,
            context.callback(|key: Key| Message::Navigate(key.as_str().map(str::to_owned))),
        ));
    let mut parts: Vec<View> = vec![switcher];
    let running = machines.iter().filter(|machine| machine.state == State::Running).count();
    if !main.layout.narrow && running > 0 {
        parts.push(
            Border::new()
                .background(ThemeBrush::CardBackground)
                .corner_radius(10.0)
                .padding(Thickness::new(10.0, 2.0, 10.0, 3.0))
                .vertical_alignment(VerticalAlignment::Center)
                .content(caption(&format!("{running} running")))
                .into(),
        );
    }
    StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(10.0)
        .vertical_alignment(VerticalAlignment::Center)
        .children(parts)
        .into()
}

/// What can be done to the machine, at the right of the title bar, after the app's settings: Start,
/// or Stop while it's on; Open screen for a Windows guest; and More, with Delete and Report a
/// problem.
pub fn actions(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let state = machine.state;
    let mut parts: Vec<View> = vec![super::app_settings::gear(main, context)];
    if matches!(state, State::Off | State::Failed | State::Installing) {
        let start =
            button("Start", context.message(Message::Start), true).is_enabled(palette::can_start(machine));
        parts.push(start.into());
    } else {
        // Asked again while it shuts down, the service stops it at once.
        let stopping = matches!(state, State::Stopping | State::StoppingStalled);
        let stop = button(if stopping { "Stop now" } else { "Stop" }, context.message(Message::Stop), false)
            .is_enabled(stopping || palette::can_ask_to_stop(machine));
        parts.push(stop.into());
    }
    if machine.system == System::Windows {
        // remoteman's host is put in the guest at its boots after the first, which set it up.
        let streams = palette::can_open_screen(machine, app::address(machine).is_some());
        parts.push(
            button("Open screen", context.message(Message::Screen), streams).is_enabled(streams).into(),
        );
    }
    let delete = MenuItem::item(DELETE_KEY, "Delete…")
        .glyph(glyph::DELETE)
        .enabled(state == State::Off)
        .tooltip("Only a stopped machine can be deleted. Its disks go with it.");
    let report = MenuItem::item(REPORT_TAG, "Report a problem…").glyph(glyph::FEEDBACK);
    let more = square(glyph::MORE, &format!("More for {}", machine.name)).tooltip("More").menu(Menu::new(
        [delete, MenuItem::separator(" separator"), report],
        context.callback(|key: Key| Message::More(key.as_str().map(str::to_owned))),
    ));
    parts.push(more);
    side_by_side(parts).margin(Thickness::new(0.0, 0.0, 8.0, 0.0)).into()
}

// ---- Overview ----

fn overview(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let wide = main.layout.wide;
    let mut parts: Vec<View> = vec![pair(wide, 2.0, hero(main, machine), facts(main, machine))];
    let connect = group(connect_rows(main, machine, context));
    parts.push(pair(wide, 1.0, connect, log_card(main, context)));
    ScrollViewer::new()
        .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
        .content(
            at_most(OVERVIEW_WIDTH, StackPanel::new().spacing(GAP).children(parts))
                .margin(Thickness::new(SIDE, 8.0, SIDE, 32.0)),
        )
        .into()
}

/// The big panel, where the machine's screen would be: its name, where it stands, and what that
/// means, over the app's mark while there's no picture of the screen.
fn hero(main: &Main, machine: &Machine) -> View {
    let state = machine.state;
    let mut status: Vec<View> = vec![pill(state).into()];
    if working(state) {
        status.push(ring(16.0).into());
    }
    let mut words: Vec<View> = vec![
        side_by_side(status).into(),
        TextBlock::new()
            .text(machine.name.as_str())
            .font_family("Segoe UI Variable Display")
            .font_size(34.0)
            .line_height(42.0)
            .font_weight(FontWeight::SEMI_BOLD)
            .text_trimming(TextTrimming::CharacterEllipsis)
            .margin(Thickness::new(0.0, 6.0, 0.0, 0.0))
            .into(),
        quiet(state_line(state)).into(),
        caption(&format!("{}. {}", system_words(machine.system), setup_words(machine))).into(),
    ];
    if let Some(doing) = &machine.working {
        words.push(
            side_by_side((
                ring(16.0),
                strong(doing),
                caption("Nothing else can be done to it until this is done."),
            ))
            .margin(Thickness::new(0.0, 6.0, 0.0, 0.0))
            .into(),
        );
    }
    let mark = Image::new()
        .source_svg(logo())
        .width(HERO_MARK)
        .height(HERO_MARK)
        .opacity(0.35)
        .horizontal_alignment(HorizontalAlignment::Center)
        .vertical_alignment(VerticalAlignment::Center)
        .margin(Thickness::new(0.0, 0.0, 0.0, 24.0));
    let height = if main.layout.wide { HERO_HEIGHT } else { HERO_HEIGHT_NARROW };
    Border::new()
        .min_height(height)
        .corner_radius(HERO_RADIUS)
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .padding(Thickness::new(24.0, 20.0, 24.0, 20.0))
        // Where the keyboard's focus starts on the page, so a screen reader says where it stands.
        .is_tab_stop(true)
        .element_ref(&main.focus)
        .automation_name(format!("{}, {}, {}", machine.name, system_words(machine.system), state.name()))
        .content(
            Grid::new()
                .rows([GridLength::STAR, GridLength::Auto])
                .children((mark, StackPanel::new().grid_row(1).spacing(2.0).children(words))),
        )
        .into()
}

/// What a machine in each state does next, under its name.
fn state_line(state: State) -> &'static str {
    match state {
        State::Stopping => {
            "Its guest is shutting down. Stop now turns it off at once, as pulling the plug would."
        }
        State::Off => "Start boots it. Delete, under More, takes its disks with it.",
        State::Installing => "Its disks are being made.",
        State::Starting => "Its guest is booting. Stop asks it to shut down.",
        State::Running => "Stop asks its guest to shut down.",
        // The service doesn't say these two yet.
        State::StoppingStalled => {
            "Its guest hasn't shut down in the time it should. Stop now turns it off at once."
        }
        State::Failed => "It stopped when it wasn't asked to. Its log says why.",
    }
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

/// Four small cards of what the machine has: its card, its size, its network, and who views it, or
/// for an Ubuntu guest its SSH.
fn facts(main: &Main, machine: &Machine) -> View {
    let fact = |label: &str, value: &str, line: &str| -> View {
        card(StackPanel::new().children((
            caption(label),
            strong(value).text_wrapping(TextWrapping::NoWrap).text_trimming(TextTrimming::CharacterEllipsis),
            caption(line),
        )))
        .padding(14.0)
        .into()
    };
    let graphics = match (&machine.chosen_card, &machine.card) {
        (Card::None, _) => fact("Graphics", "None", "It has no card."),
        (Card::Default, _) => fact(
            "Graphics",
            "Default card",
            &match &machine.card {
                Some(card) => format!(
                    "The one with the most memory when it boots, now {}",
                    super::home::short_card(card)
                ),
                None => "The one with the most memory when it boots".to_owned(),
            },
        ),
        (Card::Named(name), _) => fact("Graphics", super::home::short_card(name), "Shared with this PC"),
    };
    let disk = match machine.size_gb {
        Some(gb) => format!("Disk {gb} GB at most"),
        None => "Its disk's size isn't known".to_owned(),
    };
    let processors = if machine.processors == 1 {
        "1 processor".to_owned()
    } else {
        format!("{} processors", machine.processors)
    };
    let size = fact("Size", &format!("{} · {processors}", memory_words(machine.memory_mb)), &disk);
    let forwards = match machine.forwards.len() {
        0 => "No forwards".to_owned(),
        1 => "1 forward".to_owned(),
        n => format!("{n} forwards"),
    };
    let address =
        app::address(machine).map_or_else(|| "No address yet".to_owned(), |address| address.to_string());
    let network = match (&machine.network, &machine.network_mode) {
        (false, _) => fact("Network", "Off", "It has no network."),
        (true, NetworkMode::Shared) => fact("Network", "Shared", &format!("{address} · {forwards}")),
        (true, NetworkMode::Bridged { adapter }) => {
            fact("Network", "Bridged", &format!("{address} · onto {adapter}"))
        }
    };
    let fourth = match machine.system {
        System::Windows => {
            let shown = main
                .viewing
                .as_ref()
                .zip(main.remote_view_facts())
                .map(|(viewing, facts)| viewing.shown(&facts));
            match shown {
                Some(shown) if shown.on => {
                    let paired = match shown.devices.len() {
                        1 => "1 paired".to_owned(),
                        n => format!("{n} paired"),
                    };
                    let value = match &shown.viewer {
                        Some(viewer) => format!("{viewer} is viewing"),
                        None => "None viewing".to_owned(),
                    };
                    fact("Other devices", &value, &paired)
                }
                _ => fact("Other devices", "Not shared", "Remote View is off"),
            }
        }
        System::Linux => match machine.ssh_port {
            Some(port) => fact("SSH", &format!("Port {port}"), "On this PC's loopback"),
            None => fact("SSH", "No port yet", "Once it's running"),
        },
    };
    let cells: Vec<View> = [graphics, size, network, fourth]
        .into_iter()
        .enumerate()
        .map(|(at, fact)| {
            Border::new()
                .grid_row(i32::from(at >= 2))
                .grid_column(i32::from(at % 2 == 1))
                .content(fact)
                .into()
        })
        .collect();
    Grid::new()
        .columns([GridLength::STAR, GridLength::STAR])
        .rows([GridLength::STAR, GridLength::STAR])
        .column_spacing(12.0)
        .row_spacing(12.0)
        .children(cells)
        .into()
}

/// How to reach the guest: its screen, Remote Desktop or SSH, its console, and its sign-in.
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
            // A device viewing the machine has its screen, which Open screen takes back.
            let viewer = main
                .viewing
                .as_ref()
                .zip(main.remote_view_facts())
                .and_then(|(viewing, facts)| viewing.shown(&facts).viewer)
                .map(|viewer| proposed::screen_line(&viewer));
            rows.push(row(
                "Screen",
                &[
                    "The guest's screen at full speed, drawn by its card and streamed by remoteman. Open screen, at the \
                     top, shows it once it's running and set up.",
                    viewer.as_deref().unwrap_or_default(),
                ],
                None,
            ));
            let open = button("Remote Desktop", context.message(Message::RemoteDesktop), false)
                .is_enabled(reachable);
            rows.push(row(
                "Remote Desktop",
                &[&where_words, "Remote Desktop shows the screen of a Pro guest, and Home has none."],
                Some(open.into()),
            ));
        }
        System::Linux => match account {
            Some(account) => {
                let lines = app::ssh_lines(machine, &account.user);
                for (heading, line) in &lines {
                    rows.push(copy_row(heading, line, context));
                }
                if lines.is_empty() {
                    rows.push(row("SSH", &[&where_words], None));
                }
            }
            None => rows.push(row("SSH", &[SSH_NOTE, &where_words], None)),
        },
    }
    rows.push(console_row(main, machine, context));
    match account {
        Some(Account { user, password, .. }) => {
            rows.push(copy_row("User", user, context));
            rows.push(copy_row("Password", password, context));
        }
        None => {
            let show = button("Show", context.message(Message::SignIn), false);
            rows.push(row(
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
        // The service names the console's pipe once the machine runs.
        let on = machine.console_pipe.is_some()
            && matches!(
                machine.state,
                State::Starting | State::Running | State::Stopping | State::StoppingStalled
            );
        let button = button("Open console", context.message(Message::Console), false).is_enabled(on);
        return row("Console", &[about], Some(button.into()));
    };
    let (words, ended) = match &open.state {
        ConsoleState::Waiting => ("Waiting for the machine's screen to be served.", false),
        ConsoleState::Connecting => ("Connecting.", false),
        ConsoleState::Connected => ("Open in its own window. Closing it leaves the machine running.", false),
        ConsoleState::Ended(why) => (why.as_str(), true),
    };
    let mut buttons: Vec<View> = Vec::new();
    if ended {
        buttons.push(button("Reconnect", context.message(Message::Reconnect), true).into());
    } else {
        buttons.push(
            button("Show", context.message(Message::Console), false)
                .automation_name("Show the console")
                .into(),
        );
    }
    buttons.push(
        button("Close", context.message(Message::CloseConsole), false)
            .automation_name("Close the console")
            .into(),
    );
    let state = if ended { bad(words) } else { caption(words) };
    row_with(
        "Console",
        StackPanel::new().children((caption(about), state)),
        None,
        Some(side_by_side(buttons).into()),
    )
}

/// The machine's log, as the service sends it, with how many problems it has and Watch again once
/// it stopped.
fn log_card(main: &Main, context: &ViewContext<Main>) -> View {
    let problems = main.log.iter().filter(|line| line.kind == app::Kind::Problem).count();
    let mut right: Vec<View> = Vec::new();
    if problems > 0 {
        let words = if problems == 1 { "1 problem".to_owned() } else { format!("{problems} problems") };
        right.push(
            side_by_side((icon(glyph::PROBLEM, 12.0).foreground(ThemeBrush::SystemCritical), bad(&words)))
                .into(),
        );
    }
    if !main.watching() {
        right.push(button("Watch again", context.message(Message::WatchAgain), false).into());
    }
    panel("Log", None, Some(side_by_side(right).into()), vec![log(&main.log, "Nothing yet.")])
}
