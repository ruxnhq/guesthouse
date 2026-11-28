//! A Windows machine's Remote View: the switch, how a device connects, pairing, and the devices
//! that can view it. What it shows and asks is `crate::remote_view`'s; this only draws it.

use guesthouse_remote_view_core::share::ShareState;
use guesthouse_remote_view_core::wording;
use windows_reactor::*;

use super::parts::*;
use super::settings::heading;
use crate::app::{self, Main, Message, Section};
use crate::remote_view::{self, Ask, PairingShown, Shown, proposed};

/// The widest the section is.
const SECTION_WIDTH: f64 = 980.0;
/// A pairing code's size, large enough to compare with a device's across a room.
const CODE_SIZE: f64 = 44.0;
/// How often a countdown is drawn again.
const TICK: std::time::Duration = std::time::Duration::from_secs(1);

pub fn section(main: &Main, context: &ViewContext<Main>) -> View {
    let mut parts = vec![heading(main, Section::RemoteView.label(), None)];
    let (Some(viewing), Some(facts)) = (&main.viewing, main.remote_view_facts()) else {
        return scrolled(parts, SECTION_WIDTH);
    };
    let shown = viewing.shown(&facts);
    let listening = viewing.info.as_ref().is_some_and(|info| info.state == ShareState::Listening);
    parts.push(switch_card(&shown, listening, context));
    if shown.on {
        if let Some(viewer) = &shown.viewer {
            parts.push(panel(&shown.headline, Some(&proposed::takes_screen(viewer)), None, Vec::new()));
        }
        let pairing = shown.pairing.as_ref().map(|pairing| pairing_card(pairing, shown.held, context));
        let connect = connect_card(main, &shown, context);
        parts.push(match pairing {
            Some(pairing) => pair(main.layout.wide, 1.2, connect, pairing),
            None => connect,
        });
        parts.push(devices_card(&shown, context));
    }
    scrolled(parts, SECTION_WIDTH)
}

/// The switch, with where it stands and what it means.
fn switch_card(shown: &Shown, listening: bool, context: &ViewContext<Main>) -> View {
    let headline = strong(&shown.headline).foreground(if shown.on && listening && shown.refused.is_none() {
        ThemeBrush::SystemSuccess
    } else {
        ThemeBrush::SecondaryText
    });
    let mut words: Vec<View> = vec![
        subtitle(wording::ALLOW_TITLE).into(),
        headline.into(),
        caption(wording::NETWORK_WARNING).into(),
    ];
    if let Some(note) = &shown.note {
        words.push(caption(note).into());
    }
    if let Some(refused) = &shown.refused {
        words.push(bad(refused).into());
    }
    if shown.bridged {
        words.push(caption(proposed::BRIDGED).into());
    }
    let switch = toggle(
        wording::ALLOW_TITLE,
        shown.on,
        shown.switch_enabled,
        context.callback(|on| Message::RemoteView(Ask::Switch(on))),
    );
    card(Grid::new().columns([GridLength::STAR, GridLength::Auto]).column_spacing(16.0).children((
        StackPanel::new().spacing(2.0).children(words),
        Border::new().grid_column(1).vertical_alignment(VerticalAlignment::Top).content(switch),
    )))
    .padding(18.0)
    .into()
}

/// What a device types in: this PC's address and the machine's port, and the fingerprint to check.
fn connect_card(main: &Main, shown: &Shown, context: &ViewContext<Main>) -> View {
    let mut cells: Vec<(&str, View)> = Vec::new();
    if let Some(machine) = main.machine() {
        cells.push(("Name", TextBlock::new().text(machine.name.as_str()).into()));
    }
    if let Some(address) = &shown.address {
        let heading = if address.contains(':') { proposed::ADDRESS } else { proposed::PORT };
        cells.push((heading, mono(address).foreground(ThemeBrush::PrimaryText).into()));
    }
    if let Some(fingerprint) = &shown.fingerprint {
        let text = mono(&wording::short_fingerprint(fingerprint))
            .foreground(ThemeBrush::PrimaryText)
            .automation_name(format!("{} {fingerprint}", proposed::FINGERPRINT))
            .tooltip(fingerprint);
        cells.push((proposed::FINGERPRINT, text));
    }
    let rows = cells.len();
    let grid: Vec<View> = cells
        .into_iter()
        .enumerate()
        .flat_map(|(at, (label, value))| {
            let at = i32::try_from(at).unwrap_or_default();
            [
                quiet(label).grid_row(at).into(),
                Border::new().grid_row(at).grid_column(1).content(value).into(),
            ]
        })
        .collect();
    let mut body: Vec<View> = vec![
        Grid::new()
            .columns([GridLength::Pixel(100.0), GridLength::STAR])
            .rows(vec![GridLength::Auto; rows])
            .column_spacing(12.0)
            .row_spacing(8.0)
            .children(grid)
            .into(),
        caption(wording::PAIRED_HINT).into(),
    ];
    if let Some(address) = &shown.address {
        body.push(
            button(wording::COPY_ADDRESS, context.message(Message::Copy(address.clone())), false)
                .horizontal_alignment(HorizontalAlignment::Left)
                .into(),
        );
    }
    panel(wording::CONNECT_TITLE, None, None, body)
}

fn pairing_card(pairing: &PairingShown, held: bool, context: &ViewContext<Main>) -> View {
    let held: Option<View> = held.then(|| caption(proposed::HELD).into());
    match pairing {
        PairingShown::Closed { text, can_open } => {
            let open = button(
                wording::PAIR_NEW_DEVICE,
                context.message(Message::RemoteView(Ask::OpenPairing)),
                true,
            )
            .is_enabled(*can_open);
            panel(
                wording::PAIRING_TITLE,
                None,
                Some(open.into()),
                vec![
                    TextBlock::new().text(text.as_str()).text_wrapping(TextWrapping::Wrap).into(),
                    caption(wording::PAIRING_NOTE).into(),
                ],
            )
        }
        PairingShown::Open { until_ms } => {
            let stop = button(wording::STOP, context.message(Message::RemoteView(Ask::StopPairing)), false)
                .automation_name("Stop pairing");
            let mut body: Vec<View> = vec![countdown(*until_ms, None)];
            let steps: Vec<View> = wording::pairing_steps()
                .iter()
                .enumerate()
                .map(|(at, step)| {
                    TextBlock::new()
                        .text(format!("{}. {step}", at + 1))
                        .text_wrapping(TextWrapping::Wrap)
                        .into()
                })
                .collect();
            body.push(StackPanel::new().spacing(4.0).children(steps).into());
            body.push(caption(wording::PAIRING_NOTE).into());
            body.extend(held);
            panel(proposed::PAIRING_OPEN, None, Some(stop.into()), body)
        }
        PairingShown::Waiting { name, code, until_ms } => {
            let code_text = TextBlock::new()
                .text(code)
                .font_family(MONO)
                .font_size(CODE_SIZE)
                .font_weight(FontWeight::SEMI_BOLD)
                .is_text_selection_enabled(true)
                .automation_name(remote_view::spoken_code(code));
            let decline = button(wording::DECLINE, context.message(Message::RemoteView(Ask::Decline)), false)
                .automation_name(format!("{} {name}", wording::DECLINE));
            let accept = button(wording::ACCEPT, context.message(Message::RemoteView(Ask::Accept)), true)
                .automation_name(format!("{} {name}", wording::ACCEPT));
            let mut body: Vec<View> = vec![
                code_text.into(),
                countdown(Some(*until_ms), Some(code.clone())),
                caption(wording::COMPARE_CODE_LINE).into(),
                side_by_side((decline, accept)).into(),
            ];
            body.extend(held);
            panel(&wording::wants_to_pair(name), None, None, body)
        }
    }
}

fn devices_card(shown: &Shown, context: &ViewContext<Main>) -> View {
    if shown.devices.is_empty() {
        return panel(wording::DEVICES_TITLE, None, None, vec![quiet(wording::NO_DEVICES).into()]);
    }
    let mut body: Vec<View> = Vec::new();
    for device in &shown.devices {
        let remove = button(
            wording::REMOVE,
            context.message(Message::RemoveDevice(device.fingerprint.clone())),
            false,
        )
        .automation_name(format!("{} {}", wording::REMOVE, device.name));
        let mut parts: Vec<View> = vec![
            Border::new().height(1.0).background(ThemeBrush::Divider).into(),
            Grid::new()
                .columns([GridLength::STAR, GridLength::Auto])
                .children((
                    StackPanel::new()
                        .vertical_alignment(VerticalAlignment::Center)
                        .children((strong(&device.name), caption(&device.line)))
                        .tooltip(&device.fingerprint),
                    Border::new()
                        .grid_column(1)
                        .vertical_alignment(VerticalAlignment::Center)
                        .content(remove),
                ))
                .into(),
        ];
        for permission in &device.permissions {
            let (fingerprint, what) = (device.fingerprint.clone(), permission.what);
            let switch = ToggleSwitch::new()
                .is_on(permission.on)
                .automation_name(proposed::switch_name(permission.label, &device.name))
                .on_toggled(context.callback(move |on| {
                    Message::RemoteView(Ask::Allow { fingerprint: fingerprint.clone(), what, on })
                }));
            parts.push(
                Grid::new()
                    .columns([GridLength::STAR, GridLength::Auto])
                    .margin(Thickness::new(16.0, 0.0, 0.0, 0.0))
                    .children((
                        TextBlock::new().text(permission.label).vertical_alignment(VerticalAlignment::Center),
                        Border::new().grid_column(1).content(switch),
                    ))
                    .into(),
            );
        }
        body.push(StackPanel::new().spacing(6.0).children(parts).into());
    }
    panel(wording::DEVICES_TITLE, None, None, body)
}

/// The time left of an open pairing, or of a waiting device when `code` is its, counting down by
/// itself, so only its own words are drawn again each second and not the window.
fn countdown(until_ms: Option<u64>, code: Option<String>) -> View {
    match until_ms {
        Some(until_ms) => View::component::<Ticking>(Countdown { until_ms, waiting: code.is_some() }),
        None => caption("").into(),
    }
}

/// A countdown to a time in milliseconds since 1970.
#[derive(Clone, PartialEq)]
struct Countdown {
    until_ms: u64,
    /// A device waiting to be answered, or else an open pairing.
    waiting: bool,
}

/// The countdown's clock, which ticks it each second while it lasts.
struct Ticking {
    _tick: ComponentTimer,
}

impl Component for Ticking {
    type Input = Countdown;
    type Message = ();

    fn create(_: &Countdown, context: &ComponentContext<Self>) -> Self {
        Ticking { _tick: context.set_timeout(TICK, ()) }
    }

    fn update(&mut self, _: (), context: &ComponentContext<Self>) {
        self._tick = context.set_timeout(TICK, ());
    }

    fn view(&self, countdown: &Countdown, _: &mut ViewContext<Self>) -> View {
        let now = app::now_ms();
        let words = if countdown.waiting {
            remote_view::waiting_left(countdown.until_ms, now)
        } else {
            remote_view::pairing_left(Some(countdown.until_ms), now)
        };
        quiet(&words).into()
    }
}
