//! Home: the machines, a card each with where it stands, to go into. With none yet, it's the first
//! run's page, which says what a machine is made from.

use guesthouse_control::{Machine, State};
use windows_reactor::*;

use super::app_settings::gear;
use super::parts::*;
use crate::app::{Main, Message};

/// A machine's card's picture, where its screen would be.
const PICTURE_HEIGHT: f64 = 130.0;
/// The first run's picture.
const FIRST_RUN_LOGO: f64 = 240.0;
/// The widest the first run's words are.
const FIRST_RUN_WIDTH: f64 = 460.0;

/// Home's part of the title bar: Report a problem, the app's settings and New machine. The first
/// run's page has its own Report a problem and New machine, so it has only the settings.
pub fn actions(main: &Main, context: &ViewContext<Main>) -> View {
    let machines = main.status().map_or(0, |status| status.machines.len());
    let mut parts: Vec<View> = Vec::new();
    if machines > 0 {
        let report = square(glyph::FEEDBACK, "Report a problem")
            .style(ButtonStyle::Subtle)
            .on_click(context.message(Message::OpenReport))
            .tooltip("Report a problem");
        parts.push(report);
    }
    parts.push(gear(main, context));
    if machines > 0 {
        parts.push(button("New machine", context.message(Message::NewMachine), true).into());
    }
    side_by_side(parts).margin(Thickness::new(0.0, 0.0, 8.0, 0.0)).into()
}

pub fn page(main: &Main, context: &ViewContext<Main>) -> View {
    let machines = main.status().map(|status| status.machines.as_slice()).unwrap_or_default();
    if machines.is_empty() {
        return first_run(main, context);
    }
    let mut parts: Vec<View> = Vec::new();
    let running = machines.iter().filter(|machine| machine.state == State::Running).count();
    let count = format!("{running} running · {} in all", machines.len());
    parts.push(
        StackPanel::new()
            .orientation(Orientation::Horizontal)
            .spacing(16.0)
            .children((
                display("Machines"),
                quiet(&count)
                    .vertical_alignment(VerticalAlignment::Bottom)
                    .margin(Thickness::new(0.0, 0.0, 0.0, 8.0)),
            ))
            .into(),
    );
    // Until Windows restarts to finish turning on Hyper-V, as the Network section says too.
    if main.hyper_v() == guesthouse_control::HyperV::RestartPending && !main.restart_line_closed {
        parts.push(
            InfoBar::new()
                .is_open(true)
                .is_closable(true)
                .severity(InfoBarSeverity::Warning)
                .title(crate::hyper_v::HOME_LINE)
                .message(crate::hyper_v::RESTART_MORE)
                .on_closed(context.message(Message::RestartLineClosed))
                .into(),
        );
    }
    let columns = main.layout.columns;
    let cards: Vec<View> = machines
        .iter()
        .enumerate()
        .map(|(at, machine)| {
            let card = machine_card(main, machine, context);
            Border::new()
                .grid_row(i32::try_from(at / columns).unwrap_or_default())
                .grid_column(i32::try_from(at % columns).unwrap_or_default())
                .content(card)
                .into()
        })
        .collect();
    let rows = machines.len().div_ceil(columns);
    parts.push(
        Grid::new()
            .columns(vec![GridLength::STAR; columns])
            .rows(vec![GridLength::Auto; rows])
            .column_spacing(18.0)
            .row_spacing(18.0)
            .children(cards)
            .into(),
    );
    ScrollViewer::new()
        .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
        .content(
            StackPanel::new().spacing(20.0).margin(Thickness::new(SIDE, 8.0, SIDE, 32.0)).children(parts),
        )
        .into()
}

/// A machine's card: a picture where its screen would be, and its name beside where it stands. The
/// whole card is the button that goes into its page, named as the list was: its name, system and
/// state.
fn machine_card(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let state = machine.state;
    let letter: String = machine.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    let mut picture: Vec<View> = vec![
        TextBlock::new()
            .text(letter)
            .font_family("Segoe UI Variable Display")
            .font_size(44.0)
            .font_weight(FontWeight::BOLD)
            .foreground(ThemeBrush::TertiaryText)
            .horizontal_alignment(HorizontalAlignment::Center)
            .vertical_alignment(VerticalAlignment::Center)
            .into(),
        icon(system_glyph(machine.system), 16.0)
            .foreground(ThemeBrush::TertiaryText)
            .horizontal_alignment(HorizontalAlignment::Left)
            .vertical_alignment(VerticalAlignment::Bottom)
            .margin(12.0)
            .into(),
    ];
    if working(state) || machine.working.is_some() {
        picture.push(
            ring(20.0)
                .horizontal_alignment(HorizontalAlignment::Right)
                .vertical_alignment(VerticalAlignment::Top)
                .margin(12.0)
                .into(),
        );
    }
    let picture = Border::new()
        .height(PICTURE_HEIGHT)
        .corner_radius(10.0)
        .background(ThemeBrush::SubtleSecondary)
        .content(Grid::new().children(picture));
    let mut about = vec![super::machine::memory_words(machine.memory_mb)];
    if let Some(card) = &machine.card {
        about.push(short_card(card).to_owned());
    }
    let foot = Grid::new()
        .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
        .column_spacing(10.0)
        .margin(Thickness::new(4.0, 0.0, 4.0, 2.0))
        .children((
            dot(state),
            StackPanel::new().grid_column(1).children((
                strong(&machine.name)
                    .text_wrapping(TextWrapping::NoWrap)
                    .text_trimming(TextTrimming::CharacterEllipsis),
                caption(&about.join(" · "))
                    .text_wrapping(TextWrapping::NoWrap)
                    .text_trimming(TextTrimming::CharacterEllipsis),
            )),
            caption(state.name()).grid_column(2).vertical_alignment(VerticalAlignment::Center),
        ));
    let mut button = Button::new()
        .content(StackPanel::new().spacing(12.0).children((picture, foot)))
        .style(ButtonStyle::Subtle)
        .horizontal_alignment(HorizontalAlignment::Stretch)
        .horizontal_content_alignment(HorizontalAlignment::Stretch)
        .automation_name(format!("{}, {}, {}", machine.name, system_words(machine.system), state.name()))
        .on_click(context.message(Message::Open(machine.name.clone())));
    // Back from a machine's page lands on its card.
    if main.selected.as_deref() == Some(machine.name.as_str())
        || main.selected.is_none()
            && main.status().and_then(|status| status.machines.first()).map(|m| &m.name)
                == Some(&machine.name)
    {
        button = button.element_ref(&main.focus);
    }
    clickable_card(button, MACHINE_RADIUS)
}

/// A card that's a button as a whole: the card's fill and corners, with a plain button inside that
/// takes the pointer and the keyboard. WinUI's button keeps its own small corners, so it sits a
/// little inside the card's.
pub fn clickable_card(button: Button, radius: f64) -> View {
    Border::new()
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .corner_radius(radius)
        .padding(3.0)
        .content(button)
        .into()
}

/// A card's name without its maker's words in front, as a card has room for: "RTX 5090".
pub fn short_card(name: &str) -> &str {
    ["NVIDIA GeForce ", "NVIDIA ", "AMD Radeon ", "AMD ", "Intel(R) "]
        .iter()
        .find_map(|maker| name.strip_prefix(maker))
        .unwrap_or(name)
}

/// The first run's page: what a machine is made from, and New machine for either.
fn first_run(main: &Main, context: &ViewContext<Main>) -> View {
    let choice = |title: &str, line: &str, glyph: &str, first: bool| -> View {
        let tile = Border::new()
            .width(44.0)
            .height(44.0)
            .corner_radius(10.0)
            .background(ThemeBrush::SubtleSecondary)
            .content(
                icon(glyph, 22.0)
                    .foreground(ThemeBrush::AccentText)
                    .horizontal_alignment(HorizontalAlignment::Center)
                    .vertical_alignment(VerticalAlignment::Center),
            );
        let content = Grid::new()
            .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
            .column_spacing(14.0)
            .margin(Thickness::new(4.0, 6.0, 4.0, 6.0))
            .children((
                tile,
                StackPanel::new()
                    .grid_column(1)
                    .vertical_alignment(VerticalAlignment::Center)
                    .children((strong(title), caption(line))),
                icon(glyph::CHEVRON, 12.0)
                    .foreground(ThemeBrush::SecondaryText)
                    .grid_column(2)
                    .vertical_alignment(VerticalAlignment::Center),
            ));
        let mut button = Button::new()
            .content(content)
            .style(ButtonStyle::Subtle)
            .horizontal_alignment(HorizontalAlignment::Stretch)
            .horizontal_content_alignment(HorizontalAlignment::Stretch)
            .automation_name(format!("{title}, {line}"))
            .on_click(context.message(Message::NewMachine));
        if first {
            button = button.element_ref(&main.focus);
        }
        clickable_card(button, CARD_RADIUS)
    };
    let words = StackPanel::new().spacing(16.0).max_width(FIRST_RUN_WIDTH).vertical_alignment(VerticalAlignment::Center).children((
        display("No machines yet").text_wrapping(TextWrapping::Wrap),
        quiet(
            "Make one from a Windows 11 or Ubuntu Server image. Its first boot sets it up without the card, \
             and it has its card from the next boot on.",
        )
        .font_size(16.0)
        .line_height(24.0),
        choice("Windows 11", "A Windows 11 ISO, or its install.wim or install.esd", glyph::WINDOWS, true),
        choice("Ubuntu Server", "An Ubuntu Server ISO", glyph::TERMINAL, false),
        StackPanel::new().children((
            caption("The Guesthouse service runs the machines, so they go on when this window closes."),
            HyperlinkButton::new()
                .content("Report a problem")
                .automation_name("Report a problem")
                .on_click(context.message(Message::OpenReport)),
        )),
    ));
    let size = if main.layout.wide { FIRST_RUN_LOGO } else { FIRST_RUN_LOGO * 0.6 };
    let picture = Image::new()
        .source_svg(logo())
        .width(size)
        .height(size)
        .vertical_alignment(VerticalAlignment::Center);
    let both: View = if main.layout.wide {
        StackPanel::new()
            .orientation(Orientation::Horizontal)
            .spacing(56.0)
            .horizontal_alignment(HorizontalAlignment::Center)
            .vertical_alignment(VerticalAlignment::Center)
            .children((picture, words))
            .into()
    } else {
        StackPanel::new()
            .spacing(24.0)
            .horizontal_alignment(HorizontalAlignment::Center)
            .vertical_alignment(VerticalAlignment::Center)
            .children((picture.horizontal_alignment(HorizontalAlignment::Center), words))
            .into()
    };
    let parts: Vec<View> = vec![both];
    ScrollViewer::new()
        .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
        .content(
            StackPanel::new()
                .spacing(GAP)
                .margin(Thickness::new(48.0, 24.0, 48.0, 48.0))
                .vertical_alignment(VerticalAlignment::Center)
                .children(parts),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::short_card;

    #[test]
    fn a_cards_makers_words_are_left_off() {
        assert_eq!(short_card("NVIDIA GeForce RTX 5090"), "RTX 5090");
        assert_eq!(short_card("AMD Radeon RX 7900 XTX"), "RX 7900 XTX");
        assert_eq!(short_card("Intel(R) UHD Graphics 770"), "UHD Graphics 770");
        assert_eq!(short_card("Some Card"), "Some Card");
    }
}
