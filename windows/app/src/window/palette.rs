//! The command palette: a dialog with a box to type in and what it finds under it, a heading over
//! each machine's entries. The keyboard's focus stays in the box: Up and Down move the highlight,
//! which a screen reader is told of, Enter runs it, and Escape closes the dialog, which gives the
//! focus back to where it was. Each entry is a button too, for the pointer and for UI Automation.
//! What it offers and finds is `crate::palette`'s.

use windows_reactor::*;

use super::parts::*;
use crate::app::{Main, Message};
use crate::palette::{Key, MOST_SHOWN};

/// How wide the palette's list is, as the dialog lets it be.
const WIDTH: f64 = 470.0;

/// The box in the title bar, which opens the palette, with the shortcut beside its
/// words, as Windows' own apps have it.
pub fn title_box(main: &Main, context: &ViewContext<Main>) -> View {
    let words = Grid::new()
        .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
        .column_spacing(10.0)
        .children((
            icon(glyph::SEARCH, 14.0)
                .foreground(ThemeBrush::SecondaryText)
                .vertical_alignment(VerticalAlignment::Center),
            TextBlock::new()
                .text(if main.layout.wide { "Search or run a command" } else { "Search" })
                .foreground(ThemeBrush::SecondaryText)
                .text_trimming(TextTrimming::CharacterEllipsis)
                .vertical_alignment(VerticalAlignment::Center)
                .grid_column(1),
            caption("Ctrl+K").vertical_alignment(VerticalAlignment::Center).grid_column(2),
        ));
    Button::new()
        .content(words)
        .horizontal_content_alignment(HorizontalAlignment::Stretch)
        .width(if main.layout.wide { 320.0 } else { 220.0 })
        .vertical_alignment(VerticalAlignment::Center)
        .automation_name("Search or run a command")
        .automation_help_text("Ctrl+K")
        .on_click(context.message(Message::OpenPalette))
        .into()
}

pub fn dialog(main: &Main, context: &ViewContext<Main>) -> ContentDialog {
    let query = main.palette.as_ref().map(|palette| palette.query.as_str()).unwrap_or_default();
    let (found, all) = main.palette_found();
    let at = main.palette.as_ref().map_or(0, |palette| palette.at.min(found.len().saturating_sub(1)));
    let field = TextBox::new()
        .text(query)
        .placeholder_text("A machine, a section, or what to do")
        .automation_name("Search or run a command")
        .automation_help_text("Up and Down choose, Enter runs, Escape closes.")
        .element_ref(&main.palette_box)
        .on_text_changed(context.callback(|text: std::rc::Rc<str>| Message::PaletteQuery(text.to_string())));
    let mut rows: Vec<View> = Vec::new();
    let mut heading = None;
    for (place, entry) in found.iter().enumerate() {
        let group = entry.group.heading();
        if heading.as_ref() != Some(&group) {
            rows.push(
                caption(&group)
                    .margin(Thickness::new(10.0, if place == 0 { 0.0 } else { 8.0 }, 0.0, 2.0))
                    .into(),
            );
            heading = Some(group);
        }
        rows.push(row(entry, place == at, context));
    }
    if found.is_empty() {
        rows.push(quiet("Nothing here is called that.").margin(Thickness::new(10.0, 4.0, 0.0, 4.0)).into());
    } else if all > MOST_SHOWN {
        let more = all - MOST_SHOWN;
        let words = if more == 1 {
            "1 more. Type to narrow them down.".to_owned()
        } else {
            format!("{more} more. Type to narrow them down.")
        };
        rows.push(caption(&words).margin(Thickness::new(10.0, 6.0, 0.0, 0.0)).into());
    }
    let list = StackPanel::new().spacing(2.0).automation_name("What the palette found").children(rows);
    let foot = caption(
        "Up and Down to move, Enter to run, Escape to close. Every action here has a button on its page too.",
    );
    let body = StackPanel::new().spacing(12.0).width(WIDTH).children((field, list, foot));
    let sender = context.sender();
    // Up, Down and Enter, before the box has them, as the box keeps the focus.
    let keys =
        Border::new().content(body).on_preview_key_down(RoutedCallback::new(move |key: KeyEventInfo| {
            let plain = !key.modifiers.contains(InputModifiers::CONTROL)
                && !key.modifiers.contains(InputModifiers::ALT);
            match Key::of(key.key.0).filter(|_| plain) {
                Some(pressed) => {
                    let _ = sender.send(Message::PaletteKey(pressed));
                    true
                }
                None => false,
            }
        }));
    ContentDialog::new()
        .title("Search or run a command")
        .content(keys)
        .is_open(true)
        .on_closed(context.callback(|_: ContentDialogResult| Message::PaletteClosed))
}

/// One entry: its name, and its hint at the right, highlighted when it's the one Enter runs.
fn row(entry: &crate::palette::Entry, highlighted: bool, context: &ViewContext<Main>) -> View {
    let mut cells: Vec<View> = vec![
        TextBlock::new()
            .text(entry.name.as_str())
            .font_weight(if highlighted { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
            .text_trimming(TextTrimming::CharacterEllipsis)
            .into(),
    ];
    if !entry.hint.is_empty() {
        cells.push(caption(&entry.hint).grid_column(1).vertical_alignment(VerticalAlignment::Center).into());
    }
    let words =
        Grid::new().columns([GridLength::STAR, GridLength::Auto]).column_spacing(12.0).children(cells);
    let mut button = Button::new()
        .content(words)
        .style(ButtonStyle::Subtle)
        .horizontal_alignment(HorizontalAlignment::Stretch)
        .horizontal_content_alignment(HorizontalAlignment::Stretch)
        .automation_name(entry.spoken())
        .on_click(context.message(Message::PaletteRun(entry.key.clone())));
    if highlighted {
        button = button.automation_help_text("Highlighted: Enter runs it.");
    }
    let mut around = Border::new().corner_radius(6.0).content(button);
    if highlighted {
        around = around
            .border_thickness(2.0)
            .border_brush(ThemeBrush::Accent)
            .background(ThemeBrush::SubtleSecondary);
    }
    around.into()
}
