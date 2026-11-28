//! What a GPU machine needs from the kernel and from this user's rights beyond the card itself: the
//! jail's parts, and the card's render node. Setup says so before the first start does.

use super::cards::find_cards;
use super::checks::{Check, CheckState, Part};
use super::intel::supported_intel_cards;
use crate::context::Context;
use crate::linux::amd::amd_cards;
use crate::render_cards::RenderCard;
use std::fs;

const LANDLOCK: &str = "landlock";
/// Always on, and not one `lsm=` names.
const CAPABILITIES: &str = "capability";
const ALLOW_UNJAILED: &str = "Or let a machine start without one, unjailed: turn on \"Start without a jail\" \
                              in its settings, or run guesthouse-vm set NAME start-unjailed on.";

/// Whether a GPU machine can be jailed here. Only a warning: a machine allowed to start without a jail
/// still runs, and its row says so.
pub fn check_jail(context: &Context) -> Option<Check> {
    const TITLE: &str = "Jail";
    let support = (context.jail_support)();
    let reason = support.refusal()?;
    let fix = if support.landlock_abi == 0 { landlock_fix(context) } else { ALLOW_UNJAILED.to_owned() };
    Some(Check::new(TITLE, CheckState::Warning, format!("A GPU machine can't be jailed here: {reason}"), fix))
}

/// The `lsm=` list that turns Landlock on beside what's on now, when the kernel says what that is.
fn landlock_fix(context: &Context) -> String {
    let on_now = fs::read_to_string(&context.security_modules).unwrap_or_default();
    let named: Vec<&str> =
        on_now.trim().split(',').filter(|module| !module.is_empty() && *module != CAPABILITIES).collect();
    if named.is_empty() || named.contains(&LANDLOCK) {
        return ALLOW_UNJAILED.to_owned();
    }
    format!(
        "If the kernel has Landlock, add lsm={LANDLOCK},{} to its command line and restart. {ALLOW_UNJAILED}",
        named.join(",")
    )
}

/// The daemon works through a card's render node, which a desktop session is usually given. A user
/// with no session, as on a server, has it through the render group only. A maker fails only when
/// none of its cards can be opened, as a machine can run on any of them, and Intel's cards count only
/// where the card checks look at them: on a PC with neither NVIDIA's nor AMD's.
pub fn check_render_nodes(context: &Context) -> Vec<Check> {
    let amd = amd_cards(context);
    let intel_counts = amd.is_empty() && find_cards(context).is_empty();
    let intel = if intel_counts { supported_intel_cards(context) } else { Vec::new() };
    [("AMD", Part::Amd, amd), ("Intel", Part::Intel, intel)]
        .into_iter()
        .filter_map(|(maker, part, cards)| {
            let closed = all_closed(context, &cards)?;
            let check = Check::new(
                &format!("{maker} card's render node"),
                CheckState::Failed,
                format!("This user cannot open {}.", closed.render_node.display()),
                "It usually belongs to the render group: run sudo usermod -aG render $USER, then log in again.",
            );
            Some(check.about(part))
        })
        .collect()
}

/// The first card, when this user can open none of their render nodes. A recorded card has no node,
/// and isn't held back by one.
fn all_closed<'a>(context: &Context, cards: &'a [RenderCard]) -> Option<&'a RenderCard> {
    let closed = |card: &RenderCard| {
        !card.render_node.as_os_str().is_empty() && !(context.may_open)(&card.render_node)
    };
    cards.iter().all(closed).then(|| cards.first()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::amd::stand_in::fake_render_node;
    use crate::setup::cards::stand_in::fake_pci;
    use crate::testing::{Scratch, scratch_context};
    use guesthouse_jail::Support;

    #[test]
    fn a_kernel_with_the_jail_says_nothing() {
        let scratch = Scratch::new();
        assert_eq!(check_jail(&scratch_context(&scratch)), None);
    }

    #[test]
    fn landlock_left_out_of_lsm_is_told_how_to_put_back() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.jail_support = || Support { landlock_abi: 0, seccomp: true };
        context.security_modules = scratch.write("lsm", "capability,lockdown,yama,integrity,apparmor\n");
        let found = check_jail(&context).unwrap();
        assert_eq!(found.state, CheckState::Warning);
        assert!(found.text.contains("no Landlock"), "{}", found.text);
        assert!(found.fix.contains("add lsm=landlock,lockdown,yama,integrity,apparmor to"), "{}", found.fix);
        assert!(found.fix.contains("guesthouse-vm set NAME start-unjailed on"), "{}", found.fix);
    }

    #[test]
    fn with_no_list_to_go_by_only_starting_unjailed_is_offered() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.jail_support = || Support { landlock_abi: 0, seccomp: true };
        context.security_modules = scratch.path("no-such-file");
        assert_eq!(check_jail(&context).unwrap().fix, ALLOW_UNJAILED);
        context.jail_support = || Support { landlock_abi: 10, seccomp: false };
        let found = check_jail(&context).unwrap();
        assert!(found.text.contains("seccomp") && found.fix == ALLOW_UNJAILED, "{found:?}");
    }

    #[test]
    fn a_render_node_this_user_cannot_open_fails_that_makers_card() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        assert!(check_render_nodes(&context).is_empty(), "no card, nothing to open");
        fake_render_node(&scratch, 128, "0000:c4:00.0", "0x1002", 0x15bf);
        assert!(check_render_nodes(&context).is_empty());
        context.may_open = |_| false;
        let found = check_render_nodes(&context);
        assert_eq!(found.len(), 1);
        assert!(found[0].state == CheckState::Failed && found[0].part == Part::Amd);
        assert!(found[0].text.ends_with("renderD128.") && found[0].fix.contains("render group"), "{found:?}");
        fake_render_node(&scratch, 129, "0000:03:00.0", "0x1002", 0x7550);
        context.may_open = |node| node.ends_with("renderD129");
        assert!(check_render_nodes(&context).is_empty(), "one card that opens will do");
    }

    #[test]
    fn intel_cards_count_only_where_the_card_checks_look_at_them() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.may_open = |_| false;
        fake_render_node(&scratch, 128, "0000:03:00.0", "0x8086", 0xe20b);
        let alone = check_render_nodes(&context);
        assert!(alone.len() == 1 && alone[0].part == Part::Intel, "{alone:?}");
        fake_pci(&scratch, "0000:01:00.0", 0x2b85, "0x10de", "0x030000");
        assert!(check_render_nodes(&context).is_empty(), "the NVIDIA card is the one that runs");
    }
}
