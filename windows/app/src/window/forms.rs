//! New machine and Report a problem: pages of their own, each with Back in the title bar to where it
//! was opened from.

use std::rc::Rc;

use guesthouse_control::report::{KEPT_DAYS, ProblemKind, stage::size_words};
use windows_reactor::*;

use super::parts::*;
use super::settings::{CARD_NOTE, card_box, tpm_words};
use crate::app::{EDITIONS, FolderFor, Main, Message, Ready, ReportStage};

/// The widest a form is.
const FORM_WIDTH: f64 = 900.0;

/// A form's heading and the line under it.
fn heading(text: &str, line: &str) -> View {
    StackPanel::new()
        .spacing(2.0)
        .margin(Thickness::new(0.0, 12.0, 0.0, 0.0))
        .children((title(text), quiet(line)))
        .into()
}

/// A section of a form: its heading over a card of its rows.
fn titled(heading: &str, rows: Vec<View>) -> View {
    StackPanel::new().spacing(8.0).children((strong(heading), group(rows))).into()
}

// ---- New machine ----

pub fn new_page(main: &Main, context: &ViewContext<Main>) -> View {
    let form = &main.form;
    let busy = main.busy();
    let mut sections: Vec<View> = vec![heading(
        "New machine",
        "The service makes its disk from the image. Its first boot sets it up without the card, and it has its \
         card from the next boot on.",
    )];

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
        .element_ref(&main.focus)
        .on_text_changed(context.callback(|text: Rc<str>| Message::Name(text.to_string())));
    let name_row =
        row_with("Name", if name_ok { caption(name_hint) } else { bad(name_hint) }, None, Some(name.into()));
    let image = Grid::new().columns([GridLength::STAR, GridLength::Auto]).column_spacing(8.0).children((
        TextBox::new()
            .placeholder_text("A path to an ISO, install.wim or install.esd")
            .text(form.image.as_str())
            .is_enabled(!busy)
            .automation_name("Image")
            .on_text_changed(context.callback(|text: Rc<str>| Message::Image(text.to_string()))),
        button("Browse…", context.message(Message::Browse), false).is_enabled(!busy).grid_column(1),
    ));
    let image_row = row_under(
        "Image",
        "A Windows 11 ISO, or its install.wim or install.esd, or an Ubuntu Server ISO.",
        image.into(),
    );
    let user_problem = guesthouse_control::user_problem(form.user.trim());
    let user = TextBox::new()
        .placeholder_text(guesthouse_control::DEFAULT_USER)
        .text(form.user.as_str())
        .width(280.0)
        .is_enabled(!busy)
        .automation_name("User")
        .on_text_changed(context.callback(|text: Rc<str>| Message::User(text.to_string())));
    let user_row = row_with(
        "User",
        match &user_problem {
            Some(problem) => bad(problem),
            None => caption("The guest's administrator. Its password is made for it."),
        },
        None,
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
        "Edition",
        &["Which of a Windows image's editions to install. An Ubuntu image has one."],
        Some(edition.into()),
    );
    let location = Grid::new().columns([GridLength::STAR, GridLength::Auto]).column_spacing(8.0).children((
        TextBox::new()
            .placeholder_text("Empty keeps it where the service keeps machines")
            .text(form.location.as_str())
            .is_enabled(!busy)
            .automation_name("Location")
            .on_text_changed(context.callback(|text: Rc<str>| Message::Location(text.to_string()))),
        button("Browse…", context.message(Message::BrowseFolder(FolderFor::Location)), false)
            .is_enabled(!busy)
            .grid_column(1),
    ));
    let location_row = row_under(
        "Location",
        "A folder on one of this PC's own drives. Its disks go in a folder of the machine's name there, and can be moved later.",
        location.into(),
    );
    sections.push(titled("Machine", vec![name_row, image_row, location_row, user_row, edition_row]));

    let number = |name: &str, value: Option<f64>, step: f64, changed: Callback<Option<f64>>| -> View {
        number_box(name, value, step, !busy, changed).into()
    };
    sections.push(titled(
        "Size",
        vec![
            row(
                "Memory (MB)",
                &["Taken from this PC while the machine runs."],
                Some(number("Memory (MB)", form.memory_mb, 1024.0, context.callback(Message::Memory))),
            ),
            row(
                "Processors",
                &[],
                Some(number("Processors", form.processors, 1.0, context.callback(Message::Processors))),
            ),
            row(
                "Disk (GB)",
                &["The disk's largest size. It only takes what the guest writes."],
                Some(number("Disk (GB)", form.size_gb, 16.0, context.callback(Message::Size))),
            ),
        ],
    ));

    let cards = card_box(main, &main.card_choices(), &form.card, !busy, context.callback(Message::Card));
    sections.push(titled("Graphics", vec![row("Card", &[CARD_NOTE], Some(cards))]));

    let share = StackPanel::new().spacing(8.0).children((
        Grid::new().columns([GridLength::STAR, GridLength::Auto]).column_spacing(8.0).children((
            TextBox::new()
                .placeholder_text("None. A folder of this PC's to share")
                .text(form.share_folder.as_str())
                .is_enabled(!busy)
                .automation_name("Shared folder")
                .on_text_changed(context.callback(|text: Rc<str>| Message::ShareFolder(text.to_string()))),
            button("Browse…", context.message(Message::BrowseFolder(FolderFor::Form)), false)
                .is_enabled(!busy)
                .grid_column(1),
        )),
        CheckBox::new()
            .content("Read-only")
            .is_checked(form.share_read_only)
            .is_enabled(!busy)
            .on_is_checked_changed(context.callback(Message::ShareReadOnly)),
    ));
    sections.push(titled(
        "Devices",
        vec![
            row("TPM", &[tpm_words(None)], Some(toggle("TPM", form.tpm, !busy, context.callback(Message::Tpm)))),
            row(
                "Network",
                &["Through Windows' own network for its machines. Without it the guest has no address to reach it on."],
                Some(toggle("Network", form.network, !busy, context.callback(Message::Network))),
            ),
            row_under(
                "Shared folder",
                "On a Windows guest's desktop as Shared folder, and at /mnt/shared in Ubuntu. The machine's own \
                 account is let into the folder.",
                share.into(),
            ),
        ],
    ));

    let ready = form.machine().is_some() && !busy;
    let mut foot: Vec<View> =
        vec![button("Make it", context.message(Message::Create), true).is_enabled(ready).into()];
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
                        None => "It's on Home. Start it there, and its first boot sets it up.",
                        Some(why) => why.as_str(),
                    })
                    .into(),
            ),
        }
        parts.push(
            card(log(&making.lines, "Waiting for the service."))
                .padding(Thickness::new(16.0, 12.0, 16.0, 8.0))
                .into(),
        );
        // Under the title, where it's seen at once, rather than under the form that started it.
        sections.insert(1, StackPanel::new().spacing(8.0).children(parts).into());
    }
    scrolled(sections, FORM_WIDTH)
}

// ---- Report a problem ----

/// What goes in a report, and what never does, said before it's made.
const REPORT_ABOUT: &str = "A report is what the developer needs to see what went wrong: the service's log, this PC's \
     Windows version and whether the services Guesthouse needs are running, the Hyper-V events about \
     Guesthouse's machines, and for a machine you pick, its settings, its snapshots' list and its events. \
     Never a guest's password or screen key, its disks, or what it holds.";

/// The report page, which can be reached when the service can't be, too.
pub fn report_page(main: &Main, context: &ViewContext<Main>) -> View {
    let report = &main.report;
    let mut sections: Vec<View> = vec![heading(
        "Report a problem",
        "Prepare makes the report and shows you what's in it. Nothing is sent until you press Send.",
    )];
    match &report.stage {
        ReportStage::Writing { problem } => report_form(main, problem.as_deref(), context, &mut sections),
        ReportStage::Preparing => sections.push(
            StackPanel::new()
                .spacing(8.0)
                .children((
                    strong("Making the report"),
                    ProgressBar::new().is_indeterminate(true),
                    caption("Asking the service for its files, taking out names, and packing them."),
                ))
                .into(),
        ),
        ReportStage::Ready(ready) => report_ready(ready, context, &mut sections),
    }
    scrolled(sections, FORM_WIDTH)
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
        .element_ref(&main.focus)
        .on_text_changed(context.callback(|text: Rc<str>| Message::ReportWhat(text.to_string())));
    let contact = TextBox::new()
        .placeholder_text("An email or a name, if you'd like an answer")
        .text(report.contact.as_str())
        .width(280.0)
        .automation_name("Contact")
        .on_text_changed(context.callback(|text: Rc<str>| Message::ReportContact(text.to_string())));
    sections.push(titled(
        "The problem",
        vec![
            row(
                "Machine",
                &["The machine it's about, if it's about one. Its settings and events go in the report."],
                Some(machine.into()),
            ),
            row("Kind", &[], Some(kind.into())),
            row_under("What went wrong", "In your own words.", what.into()),
            row(
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
        button("Prepare", context.message(Message::ReportPrepare), true)
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
    let files: Vec<View> =
        prepared.files.iter().map(|(name, bytes)| row(name, &[&size_words(*bytes as u64)], None)).collect();
    sections.push(titled("In the report", files));
    if !prepared.missing.is_empty() {
        let missing: Vec<View> =
            prepared.missing.iter().map(|(name, reason)| row(name, &[reason], None)).collect();
        sections.push(titled("Not in it", missing));
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
            side_by_side((
                TextBlock::new()
                    .text(id.as_str())
                    .font_family(MONO)
                    .is_text_selection_enabled(true)
                    .vertical_alignment(VerticalAlignment::Center),
                button("Copy", context.message(Message::Copy(id.clone())), false)
                    .automation_name("Copy the report's ID"),
                button("Open an issue", context.message(Message::ReportIssue), true),
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
            button("Send", context.message(Message::ReportSend), true).is_enabled(!ready.sending).into(),
        );
    }
    buttons
        .push(button("Save…", context.message(Message::ReportSave), false).is_enabled(!ready.sending).into());
    buttons.push(
        button("Start over", context.message(Message::ReportStartOver), false)
            .is_enabled(!ready.sending)
            .into(),
    );
    if ready.sending {
        buttons.push(ring(20.0).into());
        buttons.push(caption("Sending.").vertical_alignment(VerticalAlignment::Center).into());
    }
    sections.push(side_by_side(buttons).into());
}
