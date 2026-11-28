//! The pieces the pages are made of: the type, the cards and their rows, the buttons, the state's
//! dot and pill, and the log. Every colour is one of WinUI's theme brushes, so light, dark and high
//! contrast all follow Windows, and an accent button is the system's accent.

use std::cell::RefCell;

use guesthouse_control::{State, System};
use windows_reactor::*;

use crate::app::{Kind, Line, Main};
use crate::log::Log;

/// The app's icon, the same as the GTK app's, for where it's shown large.
const LOGO_SVG: &[u8] =
    include_bytes!("../../../../linux/app/data/icons/hicolor/scalable/apps/io.github.xnixy.Guesthouse.svg");
/// A log's font: Windows Terminal's, and Consolas where that isn't installed.
pub const MONO: &str = "Cascadia Mono, Consolas";
/// Between a page's cards.
pub const GAP: f64 = 16.0;
/// A page's margins at the sides, and above and below its content.
pub const SIDE: f64 = 40.0;
/// How tall a log is before it scrolls.
const LOG_HEIGHT: f64 = 280.0;
/// Between one line of a log and the next.
const LOG_LINE_GAP: f64 = 4.0;
/// A card's corners, and those of a machine's card on Home and of the big panel on its Overview.
pub const CARD_RADIUS: f64 = 12.0;
pub const MACHINE_RADIUS: f64 = 14.0;
pub const HERO_RADIUS: f64 = 16.0;
/// Inside a card, at its sides.
const CARD_SIDES: f64 = 18.0;
/// The widest an info button's note is.
const NOTE_WIDTH: f64 = 360.0;

/// Glyphs of Segoe Fluent Icons.
pub mod glyph {
    pub const WINDOWS: &str = "\u{E737}";
    pub const LINUX: &str = "\u{E83B}";
    pub const MORE: &str = "\u{E712}";
    pub const DELETE: &str = "\u{E74D}";
    pub const ADD: &str = "\u{E710}";
    pub const CLOSE: &str = "\u{E711}";
    pub const PROBLEM: &str = "\u{EA39}";
    pub const WARNING: &str = "\u{E7BA}";
    pub const FEEDBACK: &str = "\u{E939}";
    pub const INFO: &str = "\u{E946}";
    pub const CHEVRON: &str = "\u{E76C}";
    pub const TERMINAL: &str = "\u{E756}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const SEARCH: &str = "\u{E721}";
}

pub fn logo() -> EncodedImage {
    EncodedImage::from_static(LOGO_SVG)
}

// ---- Type, from WinUI's type ramp ----

/// The Display style, for Home's heading.
pub fn display(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_family("Segoe UI Variable Display")
        .font_size(40.0)
        .line_height(48.0)
        .font_weight(FontWeight::SEMI_BOLD)
        .text_trimming(TextTrimming::CharacterEllipsis)
}

/// A page's heading, between the Title and Display styles.
pub fn title(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_family("Segoe UI Variable Display")
        .font_size(32.0)
        .line_height(40.0)
        .font_weight(FontWeight::SEMI_BOLD)
        .text_trimming(TextTrimming::CharacterEllipsis)
}

/// The Subtitle style.
pub fn subtitle(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_family("Segoe UI Variable Display")
        .font_size(20.0)
        .line_height(28.0)
        .font_weight(FontWeight::SEMI_BOLD)
        .text_wrapping(TextWrapping::Wrap)
}

/// The BodyStrong style.
pub fn strong(text: &str) -> TextBlock {
    TextBlock::new().text(text).font_weight(FontWeight::SEMI_BOLD).text_wrapping(TextWrapping::Wrap)
}

/// The Body style, in the secondary text colour.
pub fn quiet(text: &str) -> TextBlock {
    TextBlock::new().text(text).foreground(ThemeBrush::SecondaryText).text_wrapping(TextWrapping::Wrap)
}

/// The Caption style, in the secondary text colour.
pub fn caption(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_size(12.0)
        .foreground(ThemeBrush::SecondaryText)
        .text_wrapping(TextWrapping::Wrap)
}

/// Words that say something went wrong.
pub fn bad(text: &str) -> TextBlock {
    caption(text).foreground(ThemeBrush::SystemCritical)
}

/// Words in the middle of a page that says one thing.
pub fn centred(text: &str) -> View {
    quiet(text).text_alignment(TextAlignment::Center).into()
}

pub fn icon(glyph: &str, size: f64) -> FontIcon {
    FontIcon::new().glyph(glyph).font_size(size)
}

// ---- Buttons ----

/// A button of words, named by them, the accent's when it's what the page is for.
pub fn button(text: &str, clicked: Callback<()>, accent: bool) -> Button {
    let button = Button::new().content(text).automation_name(text).on_click(clicked);
    if accent { button.style(ButtonStyle::Accent) } else { button }
}

/// A square button with only a glyph, named for those who can't see it.
pub fn square(glyph: &str, name: &str) -> Button {
    Button::new().content(icon(glyph, 16.0)).automation_name(name)
}

/// A small round button that shows a longer note, named for what it's about.
pub fn info(name: &str, note: &str) -> View {
    let note = TextBlock::new().text(note).text_wrapping(TextWrapping::Wrap).max_width(NOTE_WIDTH);
    Button::new()
        .content(icon(glyph::INFO, 14.0))
        .style(ButtonStyle::Subtle)
        .automation_name(name)
        .vertical_alignment(VerticalAlignment::Center)
        .flyout_with(Flyout::rich(note).placement(FlyoutPlacement::Bottom))
}

/// A row of buttons, or of anything side by side.
pub fn side_by_side(children: impl IntoViews) -> StackPanel {
    StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(8.0)
        .vertical_alignment(VerticalAlignment::Center)
        .children(children)
}

// ---- Cards and their rows ----

/// A card: WinUI's card fill and stroke, rounded.
pub fn card(child: impl Into<View>) -> Border {
    Border::new()
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .corner_radius(CARD_RADIUS)
        .content(child)
}

/// A card of rows, with a hairline between each.
pub fn group(rows: Vec<View>) -> View {
    let mut inside: Vec<View> = Vec::new();
    for (n, row) in rows.into_iter().enumerate() {
        if n > 0 {
            inside.push(Border::new().height(1.0).background(ThemeBrush::Divider).into());
        }
        inside.push(row);
    }
    card(StackPanel::new().children(inside)).padding(Thickness::new(CARD_SIDES, 2.0, CARD_SIDES, 2.0)).into()
}

/// A card with its heading inside, a line under it, and what it holds below.
pub fn panel(heading: &str, line: Option<&str>, right: Option<View>, body: Vec<View>) -> View {
    let mut words: Vec<View> = vec![strong(heading).into()];
    if let Some(line) = line.filter(|line| !line.is_empty()) {
        words.push(caption(line).into());
    }
    let mut top: Vec<View> = vec![StackPanel::new().spacing(2.0).children(words).into()];
    if let Some(right) = right {
        top.push(
            Border::new().grid_column(1).vertical_alignment(VerticalAlignment::Top).content(right).into(),
        );
    }
    let mut parts: Vec<View> = vec![
        Grid::new().columns([GridLength::STAR, GridLength::Auto]).column_spacing(12.0).children(top).into(),
    ];
    parts.extend(body);
    card(StackPanel::new().spacing(12.0).children(parts))
        .padding(Thickness::new(CARD_SIDES, 14.0, CARD_SIDES, 16.0))
        .into()
}

/// A row of a card: a title with lines under it, an info button if it has a longer note, and
/// what's done with it on the right.
pub fn row(heading: &str, lines: &[&str], right: Option<View>) -> View {
    let words: Vec<View> =
        lines.iter().filter(|line| !line.is_empty()).map(|line| caption(line).into()).collect();
    row_with(heading, StackPanel::new().children(words), None, right)
}

/// A row whose words under the title are given as they are, with an info button's note if any.
pub fn row_with(
    heading: &str,
    under: impl Into<View>,
    note: Option<(&str, &str)>,
    right: Option<View>,
) -> View {
    let words = StackPanel::new()
        .spacing(2.0)
        .vertical_alignment(VerticalAlignment::Center)
        .children((strong(heading), under.into()));
    let mut cells: Vec<View> = vec![words.into()];
    if let Some((name, note)) = note {
        cells.push(
            Border::new()
                .grid_column(1)
                .vertical_alignment(VerticalAlignment::Center)
                .content(info(name, note))
                .into(),
        );
    }
    if let Some(right) = right {
        cells.push(
            Border::new().grid_column(2).vertical_alignment(VerticalAlignment::Center).content(right).into(),
        );
    }
    Grid::new()
        .columns([GridLength::STAR, GridLength::Auto, GridLength::Auto])
        .column_spacing(12.0)
        .margin(Thickness::new(0.0, 14.0, 0.0, 14.0))
        .children(cells)
        .into()
}

/// A row with a control across its width under its title, as a long path needs.
pub fn row_under(heading: &str, line: &str, control: View) -> View {
    StackPanel::new()
        .spacing(10.0)
        .margin(Thickness::new(0.0, 14.0, 0.0, 16.0))
        .children((StackPanel::new().spacing(2.0).children((strong(heading), caption(line))), control))
        .into()
}

/// A row whose text can be copied, as a guest's sign-in is.
pub fn copy_row(heading: &str, text: &str, context: &ViewContext<Main>) -> View {
    let copy = button("Copy", context.message(crate::app::Message::Copy(text.to_owned())), false)
        .automation_name(format!("Copy {heading}"));
    row_with(heading, mono(text).text_wrapping(TextWrapping::Wrap), None, Some(copy.into()))
}

/// Text in the fixed-width font, which can be selected and copied.
pub fn mono(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_family(MONO)
        .foreground(ThemeBrush::SecondaryText)
        .is_text_selection_enabled(true)
}

pub fn toggle(name: &str, on: bool, enabled: bool, toggled: Callback<bool>) -> View {
    ToggleSwitch::new().is_on(on).is_enabled(enabled).automation_name(name).on_toggled(toggled).into()
}

/// A box for a whole number, as memory and processors are given.
pub fn number_box(
    name: &str,
    value: Option<f64>,
    step: f64,
    enabled: bool,
    changed: Callback<Option<f64>>,
) -> NumberBox {
    NumberBox::new()
        .value(value)
        .minimum(1.0)
        .small_change(step)
        .spin_button_placement_mode(NumberBoxSpinButtonPlacementMode::Inline)
        .is_enabled(enabled)
        .width(180.0)
        .automation_name(name)
        .on_value_changed(changed)
}

/// A box for a port.
pub fn port_box(name: &str, value: Option<f64>, enabled: bool, changed: Callback<Option<f64>>) -> NumberBox {
    NumberBox::new()
        .value(value)
        .minimum(1.0)
        .maximum(65535.0)
        .is_enabled(enabled)
        .width(120.0)
        .automation_name(name)
        .on_value_changed(changed)
}

// ---- Where a machine stands ----

pub fn system_words(system: System) -> &'static str {
    match system {
        System::Windows => "Windows",
        System::Linux => "Linux",
    }
}

pub fn system_glyph(system: System) -> &'static str {
    match system {
        System::Windows => glyph::WINDOWS,
        System::Linux => glyph::LINUX,
    }
}

/// The colour of a machine's dot: green when running, amber on its way somewhere, grey when off, and
/// red when it failed.
pub fn state_brush(state: State) -> ThemeBrush {
    match state {
        State::Running => ThemeBrush::SystemSuccess,
        State::Starting | State::Stopping | State::StoppingStalled | State::Installing => {
            ThemeBrush::SystemCaution
        }
        State::Off => ThemeBrush::TertiaryText,
        State::Failed => ThemeBrush::SystemCritical,
    }
}

/// Whether a machine is on its way somewhere, which a ring beside it shows.
pub fn working(state: State) -> bool {
    matches!(state, State::Installing | State::Starting | State::Stopping | State::StoppingStalled)
}

pub fn dot(state: State) -> Ellipse {
    Ellipse::new()
        .width(8.0)
        .height(8.0)
        .fill(state_brush(state))
        .vertical_alignment(VerticalAlignment::Center)
}

/// Where a machine stands, as a dot and its word in a pill.
pub fn pill(state: State) -> Border {
    Border::new()
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .corner_radius(12.0)
        .padding(Thickness::new(10.0, 3.0, 12.0, 4.0))
        .horizontal_alignment(HorizontalAlignment::Left)
        .vertical_alignment(VerticalAlignment::Center)
        .content(side_by_side((
            dot(state),
            TextBlock::new().text(state.name()).font_size(12.0).font_weight(FontWeight::SEMI_BOLD),
        )))
}

pub fn ring(size: f64) -> ProgressRing {
    ProgressRing::new().is_active(true).width(size).height(size)
}

// ---- Pages ----

/// A page of a column of parts, scrolled when they don't fit, as wide as the window lets it be up
/// to `widest`, at the left.
pub fn scrolled(parts: Vec<View>, widest: f64) -> View {
    ScrollViewer::new()
        .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
        .content(
            at_most(widest, StackPanel::new().spacing(GAP).children(parts))
                .margin(Thickness::new(SIDE, 8.0, SIDE, 32.0)),
        )
        .into()
}

/// `child` as wide as there's room for, up to `widest`, at the left.
pub fn at_most(widest: f64, child: impl Into<View>) -> Grid {
    Grid::new()
        .columns([GridLength::STAR.max(widest), GridLength::Auto])
        .children((Border::new().content(child),))
}

/// A page that says one thing, in the middle: a picture, a line, and what to do.
pub fn one_thing(parts: Vec<View>) -> View {
    StackPanel::new()
        .spacing(12.0)
        .max_width(480.0)
        .margin(32.0)
        .horizontal_alignment(HorizontalAlignment::Center)
        .vertical_alignment(VerticalAlignment::Center)
        .children(parts)
        .into()
}

/// Two parts side by side when the window is wide, the first `share` times the width of the
/// second, and one over the other when it's narrow.
pub fn pair(wide: bool, share: f64, first: View, second: View) -> View {
    if wide {
        Grid::new()
            .columns([GridLength::Star(share), GridLength::STAR])
            .column_spacing(GAP)
            .children((Border::new().content(first), Border::new().grid_column(1).content(second)))
            .into()
    } else {
        StackPanel::new().spacing(GAP).children((first, second)).into()
    }
}

// ---- Logs ----

/// Lines of a log, newest at the foot, which the view keeps in sight as lines come. It goes in a
/// card of its own.
///
/// A list that makes only the rows in view, each kept by its line's ID, so a new line adds a row
/// and leaves the rows drawn before it alone, and the rest of a long log costs nothing until it's
/// scrolled to.
pub fn log(lines: &Log<Line>, empty: &str) -> View {
    let shown: View = if lines.is_empty() {
        caption(empty).into()
    } else {
        ItemsRepeater::new().virtual_source(log_source(lines)).into()
    };
    ScrollView::new()
        .height(LOG_HEIGHT)
        // Anchored at the foot, so a new line keeps the newest in view while the person hasn't
        // scrolled up.
        .vertical_anchor_ratio(1.0)
        .content(shown)
        .into()
}

thread_local! {
    /// The sources the logs were last drawn from, by their logs' revisions, newest last.
    static LOG_SOURCES: RefCell<Vec<(u64, VirtualSource)>> = const { RefCell::new(Vec::new()) };
}
/// How many log sources are kept: more than the window shows at once.
const LOG_SOURCES_KEPT: usize = 4;

/// The rows of `lines`, made only as the list asks for them. A render that left the log as it was
/// gets the source it had before, which the list takes as unchanged, so it touches none of its rows,
/// where a new one, however alike, would have each row in view drawn again.
fn log_source(lines: &Log<Line>) -> VirtualSource {
    let revision = lines.revision();
    LOG_SOURCES.with_borrow_mut(|sources| {
        if let Some((_, source)) = sources.iter().find(|(kept, _)| *kept == revision) {
            return source.clone();
        }
        let (keys, rows) = (lines.lines(), lines.lines());
        let source = VirtualSource::new(
            // The keys are asked for again only when this or the count changes.
            revision,
            rows.len(),
            move |index| keys[index].0,
            move |index| log_line(&rows[index].1),
        );
        if sources.len() == LOG_SOURCES_KEPT {
            sources.remove(0);
        }
        sources.push((revision, source.clone()));
        source
    })
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
        // The gap between lines, as the list has none of its own.
        .margin(Thickness::new(0.0, 0.0, 0.0, LOG_LINE_GAP))
        .children((mark, text))
        .into()
}
