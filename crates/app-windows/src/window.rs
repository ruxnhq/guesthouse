//! What the window shows: the machines on the left, and on the right the chosen machine's page or
//! the new machine's form. It's drawn from `Main` each time something changes, and holds nothing of
//! its own.

use guesthouse_control::{Card, Machine, State, System};
use windows_reactor::*;

use crate::app::{self, Account, Kind, Line, Main, Message, Page};

/// The app's icon, the same as the GTK app's.
const LOGO_SVG: &[u8] =
    include_bytes!("../../app/data/icons/hicolor/scalable/apps/io.github.xnixy.Guesthouse.svg");
/// The window's first size, in WinUI's units.
const WIDTH: f64 = 1100.0;
const HEIGHT: f64 = 760.0;
/// The list of machines on the left.
const LIST_WIDTH: f64 = 280.0;
/// How tall a log is before it scrolls.
const LOG_HEIGHT: f64 = 320.0;

pub fn view(main: &Main, context: &mut ViewContext<Main>) -> View {
    context.window_title("Guesthouse");
    let mut visuals = WindowVisuals::new().client_size(WIDTH, HEIGHT).backdrop(WindowBackdrop::Mica);
    if let Some(icon) = &main.icon {
        visuals = visuals.icon(icon.clone());
    }
    context.window_visuals(visuals);

    let mut title_bar =
        TitleBar::new().title("Guesthouse").icon(Icon::image_svg(EncodedImage::new(LOGO_SVG.to_vec())));
    if let Ok(status) = &main.seen {
        title_bar = title_bar.subtitle(format!("service {}", status.version));
    }
    let body: View = match &main.seen {
        Err(words) => away(words),
        Ok(_) => Grid::new()
            .columns([GridLength::Pixel(LIST_WIDTH), GridLength::STAR])
            .children((
                list(main, context),
                ScrollViewer::new().grid_column(1).content(
                    Border::new().padding(Thickness::new(32.0, 12.0, 32.0, 24.0)).content(
                        StackPanel::new().spacing(4.0).max_width(880.0).children(match main.page {
                            Page::Machine => machine_page(main, context),
                            Page::New => new_page(main, context),
                        }),
                    ),
                ),
            ))
            .into(),
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

/// In place of everything else when the service can't be asked.
fn away(words: &str) -> View {
    StackPanel::new()
        .spacing(8.0)
        .max_width(560.0)
        .margin(Thickness::new(32.0, 48.0, 32.0, 32.0))
        .horizontal_alignment(HorizontalAlignment::Center)
        .children((
            TextBlock::new().text(words).font_size(20.0).font_weight(FontWeight::SEMI_BOLD).text_wrapping(TextWrapping::Wrap),
            caption(
                "The Guesthouse service makes and runs the machines, as the system, and this app only shows \
                 what it says and asks it for what you choose. The machines go on as they are without the app.",
            ),
            caption("The app asks again every two seconds, and shows the machines as soon as it answers."),
        ))
        .into()
}

fn list(main: &Main, context: &ViewContext<Main>) -> View {
    let machines = main.status().map(|status| status.machines.as_slice()).unwrap_or_default();
    let selected = match main.page {
        Page::Machine => {
            main.selected.as_ref().and_then(|name| machines.iter().position(|m| &m.name == name))
        }
        Page::New => None,
    };
    let items = machines.iter().map(|machine| {
        let row = StackPanel::new().margin(Thickness::new(0.0, 6.0, 0.0, 6.0)).children((
            TextBlock::new().text(machine.name.as_str()),
            caption(&format!("{} · {}", system_words(machine.system), app::state_words(machine.state))),
        ));
        DataItem::new(machine.name.as_str(), machine.name.as_str()).content(row)
    });
    let mut parts: Vec<View> = vec![heading("Machines").margin(Thickness::new(16.0, 16.0, 0.0, 6.0)).into()];
    if machines.is_empty() {
        parts.push(caption("No machines yet.").margin(Thickness::new(16.0, 0.0, 16.0, 8.0)).into());
    }
    parts.push(
        ListView::new()
            .selection_mode(ListViewSelectionMode::Single)
            .items(items)
            .selected_index(selected)
            .automation_name("Machines")
            .on_selection_changed(context.callback(Message::Select))
            .into(),
    );
    parts.push(
        Button::new()
            .content("New machine…")
            .style(if main.page == Page::New { ButtonStyle::Accent } else { ButtonStyle::Default })
            .on_click(context.message(Message::NewMachine))
            .margin(Thickness::new(16.0, 12.0, 16.0, 16.0))
            .into(),
    );
    Border::new()
        .border_brush(ThemeBrush::Divider)
        .border_thickness(Thickness::new(0.0, 0.0, 1.0, 0.0))
        .content(ScrollViewer::new().content(StackPanel::new().children(parts)))
        .into()
}

fn machine_page(main: &Main, context: &ViewContext<Main>) -> Vec<View> {
    let mut page: Vec<View> = Vec::new();
    let Some(machine) = main.machine() else {
        page.push(title("No machine yet"));
        page.push(caption("Make one with New machine, from a Windows 11 or Ubuntu Server image.").into());
        return page;
    };
    page.push(title(&machine.name));

    let state = machine.state;
    let start =
        push_button("Start", context.message(Message::Start), true).is_enabled(state == State::Stopped);
    // Asked again while it shuts down, the service stops it at once.
    let stop_words = if state == State::Stopping { "Stop now" } else { "Stop" };
    let stop = push_button(stop_words, context.message(Message::Stop), false)
        .is_enabled(matches!(state, State::Starting | State::Running | State::Stopping));
    let delete =
        push_button("Delete…", context.message(Message::Delete), false).is_enabled(state == State::Stopped);
    let buttons =
        StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children((start, stop, delete));
    let state_line = match state {
        State::Stopping => {
            "Its guest is shutting down. Stop now turns it off at once, as pulling the plug would."
        }
        State::Stopped => "Delete takes its disks with it.",
        State::Preparing => "Its disks are being made.",
        _ => "Stop asks its guest to shut down.",
    };
    page.push(row(app::state_words(state), Some(state_line), Some(buttons.into())));

    let given = format!(
        "{} MB of memory, {} processors, and {}.",
        machine.memory_mb,
        machine.processors,
        machine.card.as_deref().unwrap_or("no card")
    );
    page.push(row(system_words(machine.system), Some(&given), None));
    page.push(row("Setup", Some(&setup_words(machine)), None));

    page.push(heading("Connect").into());
    connect(main, machine, context, &mut page);

    page.push(heading("Log").into());
    if !main.watching() {
        page.push(push_button("Watch again", context.message(Message::WatchAgain), false).into());
    }
    page.push(log(&main.log, "Nothing yet."));
    page
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

fn connect(main: &Main, machine: &Machine, context: &ViewContext<Main>, page: &mut Vec<View>) {
    let address = app::address(machine);
    let account = main.account.as_ref().filter(|account| account.machine == machine.name);
    let reachable = address.is_some() && machine.state == State::Running;
    let where_words = match address {
        Some(address) => format!("Its guest last said its address is {address}."),
        None => "No address yet. The guest says its address once it's running and on the network.".to_owned(),
    };
    match machine.system {
        System::Windows => {
            let open = push_button("Remote Desktop", context.message(Message::RemoteDesktop), true)
                .is_enabled(reachable);
            let words =
                format!("{where_words} Remote Desktop shows the screen of a Pro guest, and Home has none.");
            page.push(row("Remote Desktop", Some(&words), Some(open.into())));
        }
        System::Linux => {
            let line = match (account, address) {
                (Some(account), Some(address)) => Some(format!("ssh {}@{address}", account.user)),
                _ => None,
            };
            match line {
                Some(line) => page.push(copy_row("SSH", &line, context)),
                None => page.push(row("SSH", Some(&where_words), None)),
            }
        }
    }
    match account {
        Some(Account { user, password, .. }) => {
            page.push(copy_row("User", user, context));
            page.push(copy_row("Password", password, context));
        }
        None => {
            let show = push_button("Show", context.message(Message::SignIn), false);
            page.push(row(
                "User and password",
                Some("The guest's administrator, made for this machine."),
                Some(show.into()),
            ));
        }
    }
}

fn new_page(main: &Main, context: &ViewContext<Main>) -> Vec<View> {
    let form = &main.form;
    let busy = main.busy();
    let mut page: Vec<View> = vec![title("New machine")];
    page.push(
        caption(
            "The service makes its disk from the image. Its first boot sets it up without the card, and it \
             has its card from the next boot on.",
        )
        .margin(Thickness::new(0.0, 0.0, 0.0, 12.0))
        .into(),
    );

    let name_ok = form.name.trim().is_empty() || guesthouse_control::valid_name(form.name.trim());
    page.push(
        TextBox::new()
            .header("Name")
            .placeholder_text("win11")
            .text(form.name.as_str())
            .max_width(360.0)
            .horizontal_alignment(HorizontalAlignment::Left)
            .is_enabled(!busy)
            .on_text_changed(context.callback(|text: std::rc::Rc<str>| Message::Name(text.to_string())))
            .into(),
    );
    page.push(
        caption(if name_ok {
            "Letters, digits, - and _, up to 40."
        } else {
            "Only letters, digits, - and _, up to 40."
        })
        .foreground(if name_ok { ThemeBrush::SecondaryText } else { ThemeBrush::SystemCritical })
        .into(),
    );

    let image = Grid::new().columns([GridLength::STAR, GridLength::Auto]).column_spacing(8.0).children((
        TextBox::new()
            .header("Image")
            .placeholder_text("A path to an ISO, install.wim or install.esd")
            .text(form.image.as_str())
            .is_enabled(!busy)
            .on_text_changed(context.callback(|text: std::rc::Rc<str>| Message::Image(text.to_string()))),
        Button::new()
            .content("Browse…")
            .is_enabled(!busy)
            .vertical_alignment(VerticalAlignment::Bottom)
            .grid_column(1)
            .on_click(context.message(Message::Browse)),
    ));
    page.push(Border::new().margin(Thickness::new(0.0, 8.0, 0.0, 0.0)).content(image).into());
    page.push(
        caption("A Windows 11 ISO, or its install.wim or install.esd, or an Ubuntu Server ISO.").into(),
    );

    let number = |header: &str, value: Option<f64>, step: f64, changed: Callback<Option<f64>>| -> View {
        NumberBox::new()
            .header(header)
            .value(value)
            .minimum(1.0)
            .small_change(step)
            .spin_button_placement_mode(NumberBoxSpinButtonPlacementMode::Inline)
            .is_enabled(!busy)
            .min_width(180.0)
            .on_value_changed(changed)
            .into()
    };
    let sizes = StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(16.0)
        .margin(Thickness::new(0.0, 8.0, 0.0, 0.0))
        .children((
            number("Memory (MB)", form.memory_mb, 1024.0, context.callback(Message::Memory)),
            number("Processors", form.processors, 1.0, context.callback(Message::Processors)),
            number("Disk (GB)", form.size_gb, 16.0, context.callback(Message::Size)),
        ));
    page.push(sizes.into());
    page.push(caption("The disk's largest size. It only takes what the guest writes.").into());

    let choices = main.card_choices();
    let names = choices.iter().map(|card| match card {
        Card::Default => "Default: the card with the most memory of its own".to_owned(),
        Card::Named(name) => name.clone(),
        Card::None => "None".to_owned(),
    });
    page.push(
        ComboBox::new()
            .header("Card")
            .items_source(names.collect::<Vec<_>>())
            .selected_index(choices.iter().position(|card| *card == form.card))
            .is_enabled(!busy)
            .min_width(360.0)
            .margin(Thickness::new(0.0, 8.0, 0.0, 0.0))
            .automation_name("Card")
            .on_selection_changed(context.callback(Message::Card))
            .into(),
    );

    let ready = form.machine().is_some() && !busy;
    page.push(
        push_button("Make it", context.message(Message::Create), true)
            .is_enabled(ready)
            .margin(Thickness::new(0.0, 16.0, 0.0, 0.0))
            .into(),
    );

    if let Some(making) = &main.making {
        page.push(heading(&format!("Making {}", making.name)).into());
        match &making.ended {
            None => page.push(
                ProgressBar::new().is_indeterminate(true).margin(Thickness::new(0.0, 0.0, 0.0, 8.0)).into(),
            ),
            Some(ended) => page.push(
                InfoBar::new()
                    .is_open(true)
                    .is_closable(false)
                    .severity(if ended.is_none() { InfoBarSeverity::Success } else { InfoBarSeverity::Error })
                    .title(if ended.is_none() { "Made" } else { "Not made" })
                    .message(match ended {
                        None => "It's in the list. Start it there, and its first boot sets it up.",
                        Some(why) => why.as_str(),
                    })
                    .margin(Thickness::new(0.0, 0.0, 0.0, 8.0))
                    .into(),
            ),
        }
        page.push(log(&making.lines, "Waiting for the service."));
    }
    page
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

/// A page's title, in WinUI's Title style.
fn title(text: &str) -> View {
    TextBlock::new()
        .text(text)
        .font_family("Segoe UI Variable Display")
        .font_size(28.0)
        .font_weight(FontWeight::SEMI_BOLD)
        .margin(Thickness::new(0.0, 8.0, 0.0, 8.0))
        .into()
}

/// A section's heading, in WinUI's BodyStrong style.
fn heading(text: &str) -> TextBlock {
    TextBlock::new().text(text).font_weight(FontWeight::SEMI_BOLD).margin(Thickness::new(1.0, 24.0, 0.0, 6.0))
}

/// WinUI's Caption style, in the secondary text colour.
fn caption(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_size(12.0)
        .foreground(ThemeBrush::SecondaryText)
        .text_wrapping(TextWrapping::Wrap)
}

/// A row in a card: a title and a line under it on the left, and something on the right.
fn row(heading: &str, subtitle: Option<&str>, right: Option<View>) -> View {
    let mut words: Vec<View> = vec![TextBlock::new().text(heading).text_wrapping(TextWrapping::Wrap).into()];
    if let Some(subtitle) = subtitle.filter(|subtitle| !subtitle.is_empty()) {
        words.push(caption(subtitle).into());
    }
    let mut cells: Vec<View> =
        vec![StackPanel::new().vertical_alignment(VerticalAlignment::Center).children(words).into()];
    if let Some(right) = right {
        cells.push(
            Border::new().grid_column(1).vertical_alignment(VerticalAlignment::Center).content(right).into(),
        );
    }
    card()
        .min_height(64.0)
        .content(
            Grid::new().columns([GridLength::STAR, GridLength::Auto]).column_spacing(16.0).children(cells),
        )
        .into()
}

/// A row whose text can be copied, as a guest's sign-in is.
fn copy_row(heading: &str, text: &str, context: &ViewContext<Main>) -> View {
    let copy = push_button("Copy", context.message(Message::Copy(text.to_owned())), false)
        .automation_name(format!("Copy {heading}"));
    let words = StackPanel::new().vertical_alignment(VerticalAlignment::Center).children((
        TextBlock::new().text(heading),
        TextBlock::new()
            .text(text)
            .font_family("Consolas")
            .is_text_selection_enabled(true)
            .text_wrapping(TextWrapping::Wrap),
    ));
    card()
        .min_height(64.0)
        .content(Grid::new().columns([GridLength::STAR, GridLength::Auto]).column_spacing(16.0).children((
            words,
            Border::new().grid_column(1).vertical_alignment(VerticalAlignment::Center).content(copy),
        )))
        .into()
}

/// Lines of a log, newest at the foot, which the view keeps in sight as lines come.
fn log(lines: &[Line], empty: &str) -> View {
    let shown: Vec<View> =
        if lines.is_empty() { vec![caption(empty).into()] } else { lines.iter().map(log_line).collect() };
    card()
        .padding(Thickness::new(12.0, 8.0, 12.0, 8.0))
        .content(
            ScrollView::new()
                .height(LOG_HEIGHT)
                // Anchored at the foot, so a new line keeps the newest in view while the person
                // hasn't scrolled up.
                .vertical_anchor_ratio(1.0)
                .content(StackPanel::new().spacing(2.0).children(shown)),
        )
        .into()
}

fn log_line(line: &Line) -> View {
    let (prefix, brush) = match line.kind {
        Kind::Note => ("", ThemeBrush::PrimaryText),
        Kind::Problem => ("Problem: ", ThemeBrush::SystemCritical),
        Kind::Guest => ("guest: ", ThemeBrush::SecondaryText),
        Kind::Report => ("report: ", ThemeBrush::SecondaryText),
    };
    let text = TextBlock::new()
        .text(format!("{prefix}{}", line.text))
        .font_family("Consolas")
        .font_size(12.0)
        .foreground(brush)
        .is_text_selection_enabled(true)
        .text_wrapping(TextWrapping::Wrap);
    if line.kind == Kind::Problem { text.font_weight(FontWeight::SEMI_BOLD).into() } else { text.into() }
}

/// WinUI's look for a settings card: a thin layer over Mica, a hairline, rounded corners.
fn card() -> Border {
    Border::new()
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .corner_radius(6.0)
        .padding(Thickness::new(16.0, 12.0, 16.0, 12.0))
        .margin(Thickness::new(0.0, 0.0, 0.0, 2.0))
}

fn push_button(text: &str, clicked: Callback<()>, accent: bool) -> Button {
    let button = Button::new().content(text).on_click(clicked);
    if accent { button.style(ButtonStyle::Accent) } else { button }
}
