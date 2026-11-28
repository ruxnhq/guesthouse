//! What the window shows. There's no sidebar: Home has the machines, a card each, and a machine's
//! page is gone into from its card and out of with Back, in the title bar, or Alt+Left. Ctrl+K, or
//! the box in the middle of the title bar, opens the command palette. The title bar holds what
//! belongs to the whole page: on Home, New machine and Report a problem; on a machine's page, Back,
//! a switcher to another machine, and what can be done to the machine. Under it, a machine's page
//! has a bar of its sections: Overview, Remote View for a Windows guest, Hardware, Network and
//! Storage. New machine and Report a problem are pages of their own, with Back to where they were
//! opened from.
//!
//! It's drawn from `Main` each time something changes, and holds nothing of its own. Mica is under
//! everything, the title bar is drawn by the app, and every colour is one of WinUI's theme brushes,
//! so light and dark both follow Windows, and the accent is the system's.
//!
//! `home.rs` is Home, `machine.rs` a machine's page and its Overview, `settings.rs` its Hardware,
//! Network and Storage, `remote_view.rs` its Remote View, `forms.rs` New machine and Report a
//! problem, `app_settings.rs` the app's own Settings, from the gear in the title bar,
//! `palette.rs` the command palette, Ctrl+K, and `parts.rs` the pieces they're all made of.

mod app_settings;
mod forms;
mod home;
mod machine;
mod palette;
mod parts;
mod remote_view;
mod settings;

use std::cell::RefCell;

use guesthouse_control::client::Unreached;
use guesthouse_control::{NetworkMode, State};
use windows_reactor::*;

use crate::app::{self, Layout, Main, Message, Page};
use crate::remote_view::proposed;
use crate::shell::is_back;
use guesthouse_remote_view_core::wording;
use parts::*;

/// The window's first size, and the least it can be made, in WinUI's units.
pub const WIDTH: f64 = 1180.0;
const HEIGHT: f64 = 820.0;
const LEAST_WIDTH: f64 = 760.0;
const LEAST_HEIGHT: f64 = 520.0;
/// The size of the exe's icon the title bar shows, in pixels: the largest of the plainer drawings
/// for small sizes, which WinUI shrinks to the title bar's 16 units at most scales.
const TITLE_ICON_FRAME: u8 = 24;

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
    let body: View = match (&main.seen, main.page) {
        (_, Page::AppSettings) => app_settings::page(main, context),
        (Err(_), Page::Report) => forms::report_page(main, context),
        (Err(words), _) => away(words, context),
        (Ok(_), Page::Home) => home::page(main, context),
        (Ok(_), Page::Machine) => match main.machine() {
            Some(machine) if machine.needs_owner => machine::needs_owner(main, machine, context),
            Some(machine) => machine::page(main, machine, context),
            None => home::page(main, context),
        },
        (Ok(_), Page::New) => forms::new_page(main, context),
        (Ok(_), Page::Report) => forms::report_page(main, context),
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
        title_bar(main, context).grid_row(0),
        Border::new().grid_row(1).content(body),
        news.grid_row(1),
        // Draws nothing: it watches the window's width, and says when the layout changes.
        View::component::<Breakpoints>(context.callback(Message::Layout)),
    ));
    let content = keys(main, context, content);
    let removing = main.viewing.as_ref().and_then(|viewing| viewing.removing.as_ref());
    match (&main.deleting, &main.snapshot_ask) {
        (Some(name), _) => content.content_dialog(delete_dialog(name, context)),
        (None, Some(ask)) => content.content_dialog(snapshot_dialog(ask, context)),
        (None, None) if main.bridge_ask => content.content_dialog(bridge_dialog(main, context)),
        (None, None) if main.install_ask => content.content_dialog(install_dialog(main, context)),
        (None, None) if main.hyper_v_ask => content.content_dialog(hyper_v_dialog(context)),
        (None, None) if main.restart_ask => content.content_dialog(restart_dialog(main, context)),
        (None, None) if main.palette.is_some() => content.content_dialog(palette::dialog(main, context)),
        (None, None) if removing.is_some() => {
            let name = removing.map(|(_, name)| name.as_str()).unwrap_or_default();
            content.content_dialog(remove_dialog(name, context))
        }
        (None, None) => content.into(),
    }
}

/// Whether Back goes anywhere now: from any page but Home, and not while a dialog asks something.
fn can_go_back(main: &Main) -> bool {
    let asking = main.asking() || main.palette.is_some();
    let page = match main.seen {
        Ok(_) => main.page != Page::Home,
        // With the service away only the report page and the app's settings have somewhere to go
        // back to.
        Err(_) => matches!(main.page, Page::Report | Page::AppSettings),
    };
    page && !asking
}

/// Alt+Left, and the Back key some keyboards and mice have, go back, as the title bar's Back does,
/// and Ctrl+K opens the command palette, wherever the focus is, a box being typed in too: they're
/// taken before the box has them, so Ctrl+K never reaches it. Escape is left alone: a list that's
/// open, or a box being typed in, has it first. A dialog's keys never come here, as WinUI keeps a
/// dialog's controls apart from the window's.
fn keys(main: &Main, context: &ViewContext<Main>, content: Grid) -> Border {
    let sender = context.sender();
    let back = can_go_back(main);
    Border::new().content(content).on_preview_key_down(RoutedCallback::new(move |key: KeyEventInfo| {
        let held = |modifier| key.modifiers.contains(modifier);
        if crate::palette::is_palette_key(
            key.key.0,
            held(InputModifiers::CONTROL),
            held(InputModifiers::ALT),
            held(InputModifiers::WINDOWS),
        ) {
            let _ = sender.send(Message::OpenPalette);
            return true;
        }
        if back && is_back(key.key.0, held(InputModifiers::ALT)) {
            let _ = sender.send(Message::Back);
            return true;
        }
        false
    }))
}

/// The title bar: the app's own on Home, and on a machine's page Back, the switcher and the
/// machine's actions, with what's left of the bar to drag the window by.
fn title_bar(main: &Main, context: &ViewContext<Main>) -> TitleBar {
    let small = match crate::system::icon_frame(TITLE_ICON_FRAME) {
        Some(png) => Icon::image_data(EncodedImage::from_static(png)),
        None => Icon::image_svg(logo()),
    };
    let bar = TitleBar::new()
        .preferred_height(WindowTitleBarHeight::Tall)
        // As tall on every page, so nothing under it moves as pages change.
        .min_height(48.0)
        // Named as the app, whichever page shows: a machine's page has no title of its own there.
        .automation_name("Guesthouse")
        .is_back_button_visible(can_go_back(main) || main.page != Page::Home && main.seen.is_ok())
        .is_back_button_enabled(can_go_back(main))
        .on_back_requested(context.message(Message::Back));
    let machine = main.machine().filter(|_| main.page == Page::Machine && main.seen.is_ok());
    let (left, right): (Option<View>, Option<View>) = match machine {
        // One that needs an owner has nothing to be done to it but that, which its page has.
        Some(machine) if machine.needs_owner => (Some(machine::switcher(main, machine, context)), None),
        Some(machine) => {
            (Some(machine::switcher(main, machine, context)), Some(machine::actions(main, machine, context)))
        }
        None if main.page == Page::Home && main.seen.is_ok() => (None, Some(home::actions(main, context))),
        // The page that says the service can't be reached has the app's settings, as an update is
        // what puts a service of another version right.
        None if main.seen.is_err() && !matches!(main.page, Page::Report | Page::AppSettings) => (
            None,
            Some(
                Border::new()
                    .margin(Thickness::new(0.0, 0.0, 8.0, 0.0))
                    .content(app_settings::gear(main, context))
                    .into(),
            ),
        ),
        None => (None, None),
    };
    let mut bar = bar.margin(Thickness::new(0.0, 0.0, nudge(main), 0.0));
    // The palette's box, in the middle, where there's room for it.
    if !main.layout.narrow {
        bar = bar.content(palette::title_box(main, context));
    }
    if let Some(left) = left {
        bar = bar.left_header(left);
    }
    if let Some(right) = right {
        bar = bar.right_header(right);
    }
    if machine.is_none() {
        bar = bar.icon(small).title("Guesthouse");
        if let Ok(status) = &main.seen {
            bar = bar.subtitle(format!("service {}", status.version));
        }
    }
    bar
}

thread_local! {
    /// What the title bar last held, and the nudge it was given then.
    static TITLE_BAR: RefCell<(String, f64)> = const { RefCell::new((String::new(), 0.0)) };
}

/// The title bar's margin at its right, which moves by one unit each time what the bar holds
/// changes. WinUI's title bar works out where its buttons are, apart from the region that drags the
/// window, only when the bar itself changes size, not when the buttons in it change. Without this,
/// Stop taking Start's place, or a machine's buttons taking Home's, would sit under the drag region,
/// and a click there would go to the window's frame and not the button.
fn nudge(main: &Main) -> f64 {
    let machine = main.machine().filter(|_| main.page == Page::Machine);
    let running = main
        .status()
        .map_or(0, |status| status.machines.iter().filter(|machine| machine.state == State::Running).count());
    let holds = format!(
        "{:?} {} {:?} {:?} {} {running} {:?} {} {}",
        main.page,
        main.seen.is_ok(),
        machine.map(|machine| (&machine.name, machine.state, machine.set_up, machine.working.is_some())),
        machine.and_then(app::address),
        main.layout.narrow,
        main.status().map(|status| status.machines.len()),
        can_go_back(main),
        main.updates.dot(),
    );
    TITLE_BAR.with_borrow_mut(|(held, nudge)| {
        if *held != holds {
            *held = holds;
            *nudge = 1.0 - *nudge;
        }
        *nudge
    })
}

/// Watches the window's width, and tells the app only when it crosses into another layout, so a
/// window being resized isn't drawn again at every step. It draws nothing.
struct Breakpoints {
    layout: Layout,
}

impl Component for Breakpoints {
    type Input = Callback<Layout>;
    type Message = WindowSize;

    fn create(_: &Callback<Layout>, _: &ComponentContext<Self>) -> Self {
        Breakpoints { layout: Layout::of(WIDTH) }
    }

    fn update(&mut self, size: WindowSize, context: &ComponentContext<Self>) {
        let layout = Layout::of(size.width);
        if layout != self.layout {
            self.layout = layout;
            context.input().call(layout);
        }
    }

    fn view(&self, _: &Callback<Layout>, context: &mut ViewContext<Self>) -> View {
        let sized = context.callback(|size: WindowSize| size);
        context.on_window_size(sized);
        Border::new().into()
    }
}

// ---- Where the service can't be asked ----

/// In place of everything else when the service can't be asked.
fn away(words: &str, context: &ViewContext<Main>) -> View {
    let mut parts: Vec<View> = vec![
        icon(glyph::WARNING, 48.0).foreground(ThemeBrush::SystemCaution).into(),
        subtitle(words).text_alignment(TextAlignment::Center).into(),
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
        button("Report a problem", context.message(Message::OpenReport), false)
            .horizontal_alignment(HorizontalAlignment::Center)
            .margin(Thickness::new(0.0, 8.0, 0.0, 0.0))
            .into(),
    );
    parts.push(
        side_by_side((
            ring(16.0),
            caption("The app asks again every two seconds, and shows the machines as soon as it answers."),
        ))
        .horizontal_alignment(HorizontalAlignment::Center)
        .margin(Thickness::new(0.0, 8.0, 0.0, 0.0))
        .into(),
    );
    one_thing(parts)
}

// ---- Dialogs ----

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

/// Asks before Windows' Hyper-V feature is turned on, saying what changes on the PC, what it may
/// change for other virtualization apps, and how to undo it. Cancel is what Enter presses.
fn hyper_v_dialog(context: &ViewContext<Main>) -> ContentDialog {
    let words: Vec<View> = crate::hyper_v::ASK_WORDS
        .iter()
        .map(|words| TextBlock::new().text(*words).text_wrapping(TextWrapping::Wrap).into())
        .collect();
    ContentDialog::new()
        .title(crate::hyper_v::ASK_TITLE)
        .content(StackPanel::new().spacing(10.0).children(words))
        .primary_button_text(crate::hyper_v::ASK_TURN_ON)
        .close_button_text("Cancel")
        .default_button(ContentDialogButton::Close)
        .is_open(true)
        .on_closed(context.callback(Message::HyperVAnswered))
}

/// Asks again before Windows is restarted, naming the machines running now. Nothing is forced:
/// Windows asks each app, and the service its machines. Cancel is what Enter presses.
fn restart_dialog(main: &Main, context: &ViewContext<Main>) -> ContentDialog {
    let running = crate::hyper_v::running(main.machines());
    ContentDialog::new()
        .title(crate::hyper_v::RESTART_TITLE)
        .content(
            TextBlock::new().text(crate::hyper_v::restart_words(&running)).text_wrapping(TextWrapping::Wrap),
        )
        .primary_button_text(crate::hyper_v::RESTART_BUTTON)
        .close_button_text("Cancel")
        .default_button(ContentDialogButton::Close)
        .is_open(true)
        .on_closed(context.callback(Message::RestartAnswered))
}

/// Asks before an install that stops running machines, naming them, as the installer stops the
/// service, which shuts them down. Later is what Enter presses.
fn install_dialog(main: &Main, context: &ViewContext<Main>) -> ContentDialog {
    let version = main.updates.newer().map(|latest| latest.version.as_str()).unwrap_or_default();
    if main.status().is_none() {
        return ContentDialog::new()
            .title(format!("Install Guesthouse {version} now?"))
            .content(
                TextBlock::new()
                    .text(format!(
                        "{} A running machine is asked to shut down, and turned off if it's still running after a \
                         minute; anything unsaved in it is lost. A machine being made would be left half made.",
                        app_settings::UNKNOWN_MACHINES
                    ))
                    .text_wrapping(TextWrapping::Wrap),
            )
            .primary_button_text("Stop the service and install")
            .close_button_text("Later")
            .default_button(ContentDialogButton::Close)
            .is_open(true)
            .on_closed(context.callback(Message::InstallAnswered));
    }
    let mut parts: Vec<View> = vec![
        TextBlock::new()
            .text(
                "Installing stops the service, which asks each running machine to shut down, and turns off any \
                 still running after a minute. Anything unsaved in them is lost.",
            )
            .text_wrapping(TextWrapping::Wrap)
            .into(),
    ];
    for machine in crate::update::stopped_by_install(main.machines()) {
        parts.push(
            TextBlock::new()
                .text(format!("\u{2022}  {}, {}", machine.name, machine.state.name().to_lowercase()))
                .margin(Thickness::new(8.0, 0.0, 0.0, 0.0))
                .into(),
        );
    }
    ContentDialog::new()
        .title(format!("Install Guesthouse {version} now?"))
        .content(StackPanel::new().spacing(6.0).children(parts))
        .primary_button_text("Shut them down and install")
        .close_button_text("Later")
        .default_button(ContentDialogButton::Close)
        .is_open(true)
        .on_closed(context.callback(Message::InstallAnswered))
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

fn remove_dialog(name: &str, context: &ViewContext<Main>) -> ContentDialog {
    ContentDialog::new()
        .title(wording::remove_question(name))
        .content(TextBlock::new().text(wording::REMOVE_MESSAGE).text_wrapping(TextWrapping::Wrap))
        .primary_button_text(wording::REMOVE)
        .close_button_text(proposed::CANCEL)
        .default_button(ContentDialogButton::Close)
        .is_open(true)
        .on_closed(context.callback(Message::RemoveAnswered))
}

#[cfg(test)]
mod tests {
    /// A machine's memory, as it was given: in GB when it's a whole number of them, and in MB otherwise.
    use guesthouse_basics::words::memory_as_given as memory_words;

    /// A card's memory, to the nearest GB, as a card's driver gives a little under its size.
    use guesthouse_basics::words::card_memory_to_nearest_gb as card_memory_words;

    /// A machine's memory, held as it is at the edges: whole GB only when it is a whole number of them.
    #[test]
    fn memory_is_whole_gb_or_mb() {
        for (mb, said) in [
            (0, "0 MB"),
            (1, "1 MB"),
            (1023, "1023 MB"),
            (1024, "1 GB"),
            (1025, "1025 MB"),
            (1536, "1536 MB"),
            (2048, "2 GB"),
            (16_383, "16383 MB"),
            (16_384, "16 GB"),
            (u64::MAX, "18446744073709551615 MB"),
        ] {
            assert_eq!(memory_words(mb), said, "{mb} MB");
        }
    }

    /// A card's memory, held as it is at the edges: to the nearest GB from 1 GB, half a GB going up.
    #[test]
    fn card_memory_is_the_nearest_gb_or_mb() {
        for (mb, said) in [
            (0, "0 MB"),
            (1023, "1023 MB"),
            (1024, "1 GB"),
            (1535, "1 GB"),
            (1536, "2 GB"),
            (2047, "2 GB"),
            (8191, "8 GB"),
            (24_063, "23 GB"),
            (24_064, "24 GB"),
            (24_564, "24 GB"),
        ] {
            assert_eq!(card_memory_words(mb), said, "{mb} MB");
        }
    }
}
