//! `share`: how each card with a running machine is shared, in the words of the library page's panel.
//! Engine shares from the kernel need two looks, so it looks twice, a second apart. It only reads.

use crate::card_share::look::{CardShare, Looks, Running, look};
use crate::card_share::words::{card_note, machine_line, whole_line};
use crate::context::Context;
use crate::failure::Result;
use crate::machine::load_all;
use crate::tasks::{State, status};
use std::io::Write;
use std::time::Duration;

const BETWEEN_LOOKS: Duration = Duration::from_secs(1);

/// The panel's text: each card's name and note, the whole card, then each machine's part.
pub(super) fn card_text(cards: &[CardShare]) -> Vec<String> {
    let mut lines = Vec::new();
    for card in cards {
        lines.push(card.name.clone());
        lines.extend(card_note(card).map(str::to_owned));
        lines.push(format!("  Whole card: {}", whole_line(&card.whole)));
        lines.extend(
            card.machines.iter().map(|machine| format!("  {}: {}", machine.name, machine_line(machine))),
        );
    }
    lines
}

pub(super) fn run(context: &Context, out: &mut dyn Write) -> Result<()> {
    let (machines, _) = load_all(context);
    let up = |machine: &crate::machine::Machine| {
        let found = status(context, machine);
        let slot = found.slot.filter(|_| found.state == State::Running)?;
        Some(Running { machine: machine.clone(), slot, qemu: found.qemu.map(|seen| seen.pid) })
    };
    let running: Vec<Running> = machines.iter().filter_map(up).collect();
    let (first, looks) = look(context, &running, &Looks::new(), context.clock.now());
    if first.is_empty() {
        writeln!(out, "No machine with a GPU is running, so no card is being shared.")?;
        return Ok(());
    }
    context.clock.sleep(BETWEEN_LOOKS);
    let (cards, _) = look(context, &running, &looks, context.clock.now());
    for line in card_text(&cards) {
        writeln!(out, "{line}")?;
    }
    Ok(())
}
