//! A machine's Hardware, Network and Storage: what it's given at each boot, changed while it's off
//! and saved together with the bar at their foot, and its disk, its disks' place and its snapshots,
//! each asked for on its own.

use std::rc::Rc;

use guesthouse_control::{Card, CardInfo, Machine, NetworkMode, State, System};
use windows_reactor::*;

use super::parts::*;
use crate::app::{self, Draft, FolderFor, Main, Message, Section};

/// The widest these sections are. Wider, their rows get hard to read across.
const SECTION_WIDTH: f64 = 900.0;

/// A card's memory, to the nearest GB, as a card's driver gives a little under its size.
use guesthouse_basics::words::card_memory_to_nearest_gb as card_memory_words;

/// A section's heading and the line under it. The heading takes the keyboard's focus when the page
/// opens on this section.
pub fn heading(main: &Main, text: &str, line: Option<&str>) -> View {
    let mut words: Vec<View> = vec![
        Border::new()
            .is_tab_stop(true)
            .element_ref(&main.focus)
            .automation_name(text)
            .content(title(text))
            .into(),
    ];
    if let Some(line) = line {
        words.push(quiet(line).into());
    }
    StackPanel::new().spacing(2.0).children(words).into()
}

fn section(parts: Vec<View>) -> View {
    scrolled(parts, SECTION_WIDTH)
}

// ---- Hardware ----

pub fn hardware(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let mut parts = vec![heading(
        main,
        Section::Hardware.label(),
        Some("Changed while it's off. They take effect when it next starts."),
    )];
    let Some(draft) = draft_of(main, machine) else { return section(parts) };
    let open = machine.state == State::Off && !draft.saving;
    parts.push(group(vec![
        row(
            "Memory (MB)",
            &["Taken from this PC while the machine runs."],
            Some(
                number_box(
                    "Memory (MB)",
                    draft.memory_mb,
                    1024.0,
                    open,
                    context.callback(Message::SetMemory),
                )
                .into(),
            ),
        ),
        row(
            "Processors",
            &[],
            Some(
                number_box(
                    "Processors",
                    draft.processors,
                    1.0,
                    open,
                    context.callback(Message::SetProcessors),
                )
                .into(),
            ),
        ),
    ]));
    // An Ubuntu guest's files disk was made for its card's driver, so the service keeps its card.
    let (card_line, card_note) = match machine.system {
        System::Linux => ("An Ubuntu guest keeps the card it was made with.", CARD_KEPT_NOTE),
        System::Windows => ("The card the guest shares with this PC.", CARD_NOTE),
    };
    let cards = card_box(
        main,
        &main.settings_cards(),
        &draft.card,
        open && machine.system == System::Windows,
        context.callback(Message::SetCard),
    );
    let mut graphics =
        vec![row_with("Card", caption(card_line), Some(("More about the card", card_note)), Some(cards))];
    if app::offers_dlss(machine.system, &draft.card, main.cards()) {
        let switch = toggle("DLSS", draft.nvapi_wrapper, open, context.callback(Message::SetDlss));
        graphics.push(row_with(
            "DLSS",
            caption("Guesthouse's NVAPI in Windows' place. Off for games whose anti-cheat refuses it."),
            Some(("More about DLSS", DLSS_NOTE)),
            Some(switch),
        ));
    }
    parts.push(group(graphics));
    let tpm = match machine.system {
        System::Linux => row(
            "TPM",
            &[tpm_words(Some(System::Linux))],
            Some(toggle("TPM", draft.tpm, open, context.callback(Message::SetTpm))),
        ),
        System::Windows => row_with(
            "TPM",
            caption("Windows 11 asks for one to set up."),
            Some(("More about the TPM", tpm_words(Some(System::Windows)))),
            Some(toggle("TPM", draft.tpm, open, context.callback(Message::SetTpm))),
        ),
    };
    let nested_line = match machine.system {
        System::Windows => "Lets the guest run virtual machines of its own: WSL, Docker, Windows Sandbox.",
        System::Linux => "Lets the guest run virtual machines of its own, such as KVM and Docker's.",
    };
    let nested = row_with(
        "Nested virtualization",
        caption(nested_line),
        Some(("More about nested virtualization", nested_words(machine.system))),
        Some(toggle(
            "Nested virtualization",
            draft.nested_virtualization,
            open,
            context.callback(Message::SetNested),
        )),
    );
    parts.push(group(vec![tpm, nested]));
    section(parts)
}

/// The selected machine's draft, when it's this machine's.
fn draft_of<'a>(main: &'a Main, machine: &Machine) -> Option<&'a Draft> {
    main.draft.as_ref().filter(|draft| draft.machine == machine.name)
}

/// What a card is to the guest, behind the card's info button.
pub const CARD_NOTE: &str = "The card the guest shares with this PC. Default is whichever has the most memory of its \
                             own when it boots.";

/// Why an Ubuntu guest's card can't change, behind the card's info button.
const CARD_KEPT_NOTE: &str = "The card the guest shares with this PC. An Ubuntu guest's files disk was made for its \
                              card's driver, so it keeps the card it was made with.";

/// The DLSS switch's note, in the Linux app's words.
const DLSS_NOTE: &str = "NVIDIA's NVAPI won't tell DLSS which card it's on in a virtual machine, so Guesthouse puts \
                         its own NVAPI in Windows' place for it. It isn't signed, and some games' anti-cheat refuses \
                         to start with it. Turn this off for those games, and NVIDIA's goes back. Takes effect when \
                         the machine starts.";

/// What a TPM is for, for a system or for either.
pub fn tpm_words(system: Option<System>) -> &'static str {
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

/// The list of cards, with `chosen` selected.
pub fn card_box(
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
        .width(320.0)
        .automation_name("Card")
        .on_selection_changed(changed)
        .into()
}

/// A card in a list: its name and its memory, so cards of one maker can be told apart.
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

// ---- Network ----

pub fn network(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let mut parts = vec![heading(main, Section::Network.label(), None)];
    let Some(draft) = draft_of(main, machine) else { return section(parts) };
    let open = machine.state == State::Off && !draft.saving;
    let network = row(
        "Network",
        &[
            "Through Windows' own network for its machines. Without it the guest has no address to reach it on.",
        ],
        Some(toggle("Network", draft.network, open, context.callback(Message::SetNetwork))),
    );
    parts.push(group(vec![
        network,
        connects_through(main, machine.system, draft, open && draft.network, context),
    ]));
    parts.push(forwards(draft, open && draft.network, context));
    if machine.system == System::Windows
        && let (Some(viewing), Some(facts)) = (&main.viewing, main.remote_view_facts())
    {
        let shown = viewing.shown(&facts);
        let go = button(
            "Open",
            context.message(Message::Section(Some(Section::RemoteView.label().to_owned()))),
            false,
        )
        .automation_name("Open Remote View");
        parts.push(group(vec![row(Section::RemoteView.label(), &[&shown.headline], Some(go.into()))]));
    }
    section(parts)
}

/// Which network the machine's card is on: the shared one, or bridged onto one of this PC's
/// adapters, chosen from those it has.
fn connects_through(
    main: &Main,
    system: System,
    draft: &Draft,
    enabled: bool,
    context: &ViewContext<Main>,
) -> View {
    let choices = main.network_choices();
    let bridged_at = match draft.network_mode {
        NetworkMode::Bridged { .. } => choices.iter().position(|mode| *mode == draft.network_mode),
        NetworkMode::Shared => None,
    };
    let can_bridge = choices.len() > 1;
    let option = |name: &str, words: &str, index: Option<usize>, checked: bool, on: bool| -> View {
        let radio = RadioButton::new()
            .group_name("connects-through")
            .content(StackPanel::new().children((strong(name), caption(words))))
            .is_checked(checked)
            .is_enabled(on)
            .automation_name(name)
            .on_checked(context.callback(move |checked: Option<bool>| {
                Message::SetNetworkMode(index.filter(|_| checked == Some(true)))
            }));
        Border::new()
            .corner_radius(10.0)
            .background(ThemeBrush::SubtleSecondary)
            .border_brush(if checked { ThemeBrush::Accent } else { ThemeBrush::CardStroke })
            .border_thickness(1.0)
            .padding(Thickness::new(14.0, 10.0, 14.0, 12.0))
            .content(radio)
            .into()
    };
    let shared = option(
        "Shared",
        "Windows' own network for its machines. The guest has a private address that only this PC reaches; forward \
         ports below to reach it from other devices.",
        Some(0),
        bridged_at.is_none(),
        enabled,
    );
    let bridged = option(
        "Bridged",
        "The guest joins this PC's network, with an address of its own from the router, so other devices reach it \
         directly.",
        can_bridge.then_some(bridged_at.unwrap_or(1)),
        bridged_at.is_some(),
        enabled && can_bridge,
    );
    let mut parts: Vec<View> =
        vec![strong("Connects through").into(), pair(main.layout.wide, 1.0, shared, bridged)];
    if let Some(at) = bridged_at {
        let names: Vec<String> = choices[1..].iter().map(|mode| adapter_words(mode, main)).collect();
        let adapters = ComboBox::new()
            .items_source(names)
            .selected_index(Some(at - 1))
            .is_enabled(enabled)
            .width(420.0)
            .automation_name("Adapter to bridge onto")
            .on_selection_changed(
                context.callback(|index: Option<usize>| Message::SetNetworkMode(index.map(|at| at + 1))),
            );
        parts.push(
            Grid::new()
                .columns([GridLength::Auto, GridLength::STAR])
                .column_spacing(12.0)
                .children((
                    caption("Onto").vertical_alignment(VerticalAlignment::Center),
                    Border::new().grid_column(1).content(adapters),
                ))
                .into(),
        );
    }
    let mut notes: Vec<String> = Vec::new();
    match main.bridging() {
        Some(bridging) if !bridging.available => {
            match crate::hyper_v::shown(
                &bridging.hyper_v,
                system,
                &crate::hyper_v::holding_up(main.machines()),
                main.hyper_v_asking,
                bridging.reason.as_deref(),
            ) {
                Some(shown) => parts.push(hyper_v_part(&shown, context)),
                // On, but the adapters couldn't be read.
                None => notes.push(
                    bridging.reason.clone().unwrap_or_else(|| "This PC can't bridge a machine.".to_owned()),
                ),
            }
        }
        Some(bridging) if bridging.adapters.is_empty() => {
            notes.push(
                "No Ethernet or Wi-Fi adapter of this PC's is up, so there is nothing to bridge onto."
                    .to_owned(),
            );
        }
        _ => {}
    }
    if can_bridge {
        notes.push("The first time, this PC's own network can drop for a few seconds.".to_owned());
    }
    if let NetworkMode::Bridged { adapter } = &draft.network_mode {
        let wifi = main.bridging().is_some_and(|bridging| {
            bridging.adapters.iter().any(|known| known.name == *adapter && known.wifi)
        });
        if wifi {
            notes.push(
                "This is a Wi-Fi adapter. Many Wi-Fi adapters, and routers, refuse a second address on one \
                 connection, so the machine may not start, or get no address. Ethernet always can."
                    .to_owned(),
            );
        }
    }
    parts.extend(notes.iter().map(|note| caption(note).into()));
    StackPanel::new().spacing(10.0).margin(Thickness::new(0.0, 14.0, 0.0, 16.0)).children(parts).into()
}

/// What Windows' Hyper-V feature stands at, which bridging is made on, while a machine can't be
/// bridged: its line, more under it, a ring while it's being turned on, and what can be done. Its
/// background says whether it waits for the person (a restart) or went wrong.
fn hyper_v_part(shown: &crate::hyper_v::Shown, context: &ViewContext<Main>) -> View {
    use crate::hyper_v::{Action, Tone};
    let line = if shown.tone == Tone::Bad { bad(&shown.line) } else { strong(&shown.line) };
    let mut words: Vec<View> = vec![line.text_wrapping(TextWrapping::Wrap).into()];
    if let Some(more) = &shown.more {
        words.push(caption(more).into());
    }
    // A grid, so the words wrap at the card's width.
    let mut top: Vec<View> = Vec::new();
    if shown.busy {
        top.push(ring(16.0).vertical_alignment(VerticalAlignment::Top).into());
    }
    top.push(StackPanel::new().grid_column(1).spacing(2.0).children(words).into());
    let mut parts: Vec<View> = vec![
        Grid::new()
            .columns([GridLength::Auto, GridLength::STAR])
            .column_spacing(if shown.busy { 10.0 } else { 0.0 })
            .children(top)
            .into(),
    ];
    if let Some((action, can)) = shown.action {
        let message = match action {
            Action::TurnOn | Action::TryAgain => Message::TurnOnHyperV,
            Action::Restart => Message::RestartWindows,
        };
        let name = match action {
            Action::TryAgain => "Try turning on Hyper-V again",
            Action::Restart => "Restart Windows now",
            Action::TurnOn => action.label(),
        };
        parts.push(
            button(action.label(), context.message(message), true)
                .is_enabled(can)
                .automation_name(name)
                .horizontal_alignment(HorizontalAlignment::Left)
                .into(),
        );
    }
    Border::new()
        .corner_radius(10.0)
        .background(match shown.tone {
            Tone::Caution => ThemeBrush::SystemCautionBackground,
            Tone::Bad => ThemeBrush::SystemCriticalBackground,
            Tone::Plain => ThemeBrush::SubtleSecondary,
        })
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .padding(Thickness::new(14.0, 12.0, 14.0, 14.0))
        .automation_name("Hyper-V")
        .content(StackPanel::new().spacing(10.0).children(parts))
        .into()
}

/// An adapter to bridge onto, as the list says it.
fn adapter_words(mode: &NetworkMode, main: &Main) -> String {
    match mode {
        NetworkMode::Shared => "Shared".to_owned(),
        NetworkMode::Bridged { adapter } => {
            let known = main
                .bridging()
                .and_then(|bridging| bridging.adapters.iter().find(|known| known.name == *adapter));
            match known {
                Some(known) if known.wifi => format!("{adapter} (Wi-Fi: often can't), {}", known.description),
                Some(known) => format!("{adapter}, {}", known.description),
                None => format!("{adapter} (not available now)"),
            }
        }
    }
}

/// The port forwards: a row of two ports each, and one to add.
fn forwards(draft: &Draft, enabled: bool, context: &ViewContext<Main>) -> View {
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
        rows.push(
            side_by_side((
                TextBlock::new().text("This PC's port").vertical_alignment(VerticalAlignment::Center),
                host,
                TextBlock::new().text("to the guest's").vertical_alignment(VerticalAlignment::Center),
                guest,
                remove,
            ))
            .into(),
        );
    }
    let add = link_button("Add a forward", context.message(Message::AddForward))
        .is_enabled(enabled && draft.forwards.len() < guesthouse_control::MOST_FORWARDS);
    rows.push(add.into());
    if let Some(problem) = draft.forwards_problem().filter(|_| !draft.forwards.is_empty()) {
        rows.push(bad(&problem).into());
    }
    let (line, note) = if bridged {
        (BRIDGED_FORWARDS_NOTE, None)
    } else {
        (
            "While the machine runs, a TCP port of this PC's leads to a port of the guest's, so other devices on \
             this network reach it at this PC's address.",
            Some(info("More about port forwards", FORWARDS_NOTE)),
        )
    };
    panel("Port forwards", Some(line), note, rows)
}

/// What a forward does, which needs the shared network and a running machine.
const FORWARDS_NOTE: &str = "While the machine runs, a TCP port of this PC's leads to a port of the guest's, so other \
                             devices on this network reach it at this PC's address. It also opens that port in \
                             Windows Firewall, to this network only, and closes both when the machine stops. Needs \
                             Windows' IP Helper service. At most 16.";

const BRIDGED_FORWARDS_NOTE: &str = "Not made while it's bridged: the guest has an address of its own on the network, \
                                     and its ports are reached there.";

/// A button of the accent's words, with a + before them, to add a row.
fn link_button(text: &str, clicked: Callback<()>) -> Button {
    Button::new()
        .content(side_by_side((icon(glyph::ADD, 12.0), TextBlock::new().text(text))))
        .style(ButtonStyle::TextLink)
        .automation_name(text)
        .on_click(clicked)
}

// ---- Storage ----

pub fn storage(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let mut parts = vec![heading(main, Section::Storage.label(), None)];
    let Some(draft) = draft_of(main, machine) else { return section(parts) };
    let stopped = machine.state == State::Off;
    parts.push(group(vec![disk_row(machine, draft, stopped, context), location_row(machine, context)]));
    parts.push(shares(machine.system, draft, stopped && !draft.saving, context));
    parts.push(snapshots(main, machine, context));
    section(parts)
}

/// The disk's size, which only grows, and is asked for on its own as it can't be undone.
fn disk_row(machine: &Machine, draft: &Draft, stopped: bool, context: &ViewContext<Main>) -> View {
    let now = match machine.size_gb {
        Some(gb) => format!("Now {gb} GB at most, and it only takes what the guest writes."),
        None => "Its size isn't known to the service.".to_owned(),
    };
    let field = number_box("Disk (GB)", draft.size_gb, 16.0, stopped, context.callback(Message::SetSize));
    let grow = button("Grow", context.message(Message::Grow), false)
        .is_enabled(stopped && machine.working.is_none() && draft.growth(machine).is_some())
        .automation_name("Grow the disk");
    row(
        "Disk (GB)",
        &[&now, "A disk only grows. The guest's own partition takes the room at its next start."],
        Some(side_by_side((field, grow)).into()),
    )
}

/// Where the machine's disks are, and moving them, which a stopped machine's can be.
fn location_row(machine: &Machine, context: &ViewContext<Main>) -> View {
    let free = machine.state == State::Off && machine.working.is_none() && machine.problem.is_none();
    let shown = if machine.folder.as_os_str().is_empty() {
        "Where the service keeps its machines.".to_owned()
    } else {
        machine.folder.display().to_string()
    };
    let mut lines: Vec<View> = vec![
        TextBlock::new()
            .text(shown)
            .text_wrapping(TextWrapping::Wrap)
            .foreground(ThemeBrush::SecondaryText)
            .is_text_selection_enabled(true)
            .into(),
    ];
    match &machine.problem {
        Some(problem) => lines.push(bad(problem).into()),
        None => lines.push(
            caption(if free {
                "Move… asks for a folder, and copies its disks to a folder of its name there, then deletes the old copy."
            } else {
                "Its disks move while it's off."
            })
            .into(),
        ),
    }
    let to = button("Move…", context.message(Message::BrowseFolder(FolderFor::Move)), false)
        .is_enabled(free)
        .automation_name("Move its disks");
    let back = button("Default place", context.message(Message::MoveDefault), false)
        .is_enabled(free)
        .automation_name("Move its disks back to where the service keeps machines");
    row_with("Location", StackPanel::new().children(lines), None, Some(side_by_side((to, back)).into()))
}

/// The shared folders: a folder's field, its Browse button, its read-only box and its remove button
/// each, and one to add.
fn shares(system: System, draft: &Draft, enabled: bool, context: &ViewContext<Main>) -> View {
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
        let browse =
            button("Browse…", context.message(Message::BrowseFolder(FolderFor::Machine(place))), false)
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
        rows.push(
            Grid::new()
                .columns([GridLength::STAR, GridLength::Auto, GridLength::Auto, GridLength::Auto])
                .column_spacing(8.0)
                .children((field, browse.grid_column(1), read_only.grid_column(2), remove.grid_column(3)))
                .into(),
        );
    }
    let add = link_button("Add a folder", context.message(Message::AddShare))
        .is_enabled(enabled && draft.shares.len() < guesthouse_control::MOST_SHARES);
    rows.push(add.into());
    if let Some(problem) = draft.shares_problem().filter(|_| !draft.shares.is_empty()) {
        rows.push(bad(&problem).into());
    }
    let count = caption(&format!("{} of {}", draft.shares.len(), guesthouse_control::MOST_SHARES));
    panel("Shared folders", Some(shares_words(system)), Some(count.into()), rows)
}

/// Where a machine's shared folders are in the guest.
fn shares_words(system: System) -> &'static str {
    match system {
        System::Windows => {
            "Each is a drive of its own from S:, linked on the guest's desktop as Shared folder, Shared folder 2 \
             and on, from its next start. The machine's own account is let into each folder. At most 8. The \
             guest mounts them with WinFsp - Windows File System Proxy, Copyright (C) Bill Zissimopoulos, \
             github.com/winfsp/winfsp."
        }
        System::Linux => {
            "At /mnt/shared in the guest, then /mnt/shared2 and on, from its next start. The machine's own \
             account is let into each folder. At most 8."
        }
    }
}

/// The machine's snapshots, newest first, and taking another. Each is of a stopped machine that has
/// been set up.
fn snapshots(main: &Main, machine: &Machine, context: &ViewContext<Main>) -> View {
    let free = machine.state == State::Off && machine.working.is_none();
    let take_words = if !machine.set_up {
        "A machine has snapshots once its first boot has set it up."
    } else if machine.state != State::Off {
        "Stop it to take a snapshot. A snapshot keeps its disk, its firmware and its TPM as they are."
    } else {
        "Keeps its disk, its firmware and its TPM as they are now, to go back to. Its disk can't grow while it has snapshots."
    };
    let label = TextBox::new()
        .placeholder_text("A label, such as Before the update")
        .text(main.snapshot_label.as_str())
        .is_enabled(free && machine.set_up)
        .automation_name("Snapshot label")
        .on_text_changed(context.callback(|text: Rc<str>| Message::SnapshotLabel(text.to_string())));
    let take = button("Take", context.message(Message::TakeSnapshot), false)
        .is_enabled(free && machine.set_up && !main.snapshot_label.trim().is_empty())
        .automation_name("Take a snapshot");
    let mut body: Vec<View> = vec![
        Grid::new()
            .columns([GridLength::STAR, GridLength::Auto])
            .column_spacing(8.0)
            .children((label, take.grid_column(1)))
            .into(),
    ];
    for snapshot in machine.snapshots.iter().rev() {
        let mut when = crate::system::local_time(snapshot.taken);
        if snapshot.current {
            when.push_str(" · It runs on from here.");
        }
        let back = button("Go back", context.message(Message::RevertSnapshot(snapshot.id)), false)
            .is_enabled(free)
            .automation_name(format!("Go back to {}", snapshot.label));
        let delete = button("Delete", context.message(Message::DeleteSnapshot(snapshot.id)), false)
            .is_enabled(free)
            .automation_name(format!("Delete the snapshot {}", snapshot.label));
        let mark = Ellipse::new()
            .width(10.0)
            .height(10.0)
            .fill(if snapshot.current { ThemeBrush::Accent } else { ThemeBrush::TertiaryText })
            .vertical_alignment(VerticalAlignment::Center);
        body.push(
            Grid::new()
                .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
                .column_spacing(12.0)
                .margin(Thickness::new(4.0, 0.0, 0.0, 0.0))
                .children((
                    mark,
                    StackPanel::new().grid_column(1).children((strong(&snapshot.label), caption(&when))),
                    side_by_side((back, delete)).grid_column(2),
                ))
                .into(),
        );
    }
    panel("Snapshots", Some(take_words), None, body)
}

// ---- Saving ----

/// The bar at the foot of Hardware, Network and Storage: why the settings can't be saved yet, or
/// what saving does, with Undo and Save. The draft is one, so it saves what changed in any of them.
pub fn save_bar(machine: &Machine, draft: &Draft, context: &ViewContext<Main>) -> View {
    let stopped = machine.state == State::Off;
    let open = stopped && !draft.saving;
    let changed = draft.changed(machine);
    let save = button("Save", context.message(Message::SaveSettings), true)
        .is_enabled(open && changed && draft.settings().is_some() && draft.problem().is_none());
    let undo = button("Undo", context.message(Message::UndoSettings), false)
        .is_enabled(open && (changed || draft.refused.is_some()));
    let problem = draft.problem();
    let (words, is_bad) = match (&draft.refused, stopped) {
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
    let words: View = match words {
        Some(words) if is_bad => {
            bad(words).font_size(14.0).vertical_alignment(VerticalAlignment::Center).into()
        }
        Some(words) => quiet(words).vertical_alignment(VerticalAlignment::Center).into(),
        None => Border::new().into(),
    };
    let bar = card(
        Grid::new()
            .columns([GridLength::STAR, GridLength::Auto])
            .column_spacing(12.0)
            .children((words, side_by_side((undo, save)).grid_column(1))),
    )
    .padding(Thickness::new(16.0, 10.0, 12.0, 10.0));
    at_most(SECTION_WIDTH, bar).margin(Thickness::new(SIDE, 0.0, SIDE, 16.0)).into()
}
