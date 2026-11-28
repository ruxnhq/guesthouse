//! The cards a Windows machine's GPU-PV can run on, as the user is offered them. A machine names its
//! card in `amd_gpu`: a card's PCI address, or empty for the NVIDIA card setup chose, and `gpu_maker`
//! says whose it is where the address can't. Each card is offered once, and setup's NVIDIA card is
//! offered with no address, as every machine from before keeps it. Any card can be picked when the
//! machine is made. After that it can move only to a card of the same maker.

use crate::context::Context;
use crate::gpu_maker::GpuMaker;
use crate::linux::amd::default_linux_gpu;
use crate::machine::Machine;
use crate::render_cards::render_cards;
use crate::setup::cards::{Card, Support, amd_support, card_in_use, find_cards, learn_card_memory};
use crate::setup::intel::supported_intel_cards;

/// Whether a new machine, or a settings page, is offered Intel's cards. They are, as an experiment:
/// an Arc B580 has drawn Windows' desktop, and the other cards are untried.
const OFFER_INTEL_CARDS: bool = true;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardChoice {
    pub maker: GpuMaker,
    /// What goes in the machine's `amd_gpu`.
    pub pci: String,
    /// Where the card is, as `0000:01:00.0`. It's `pci` for every card but setup's NVIDIA card, which
    /// has its address here where it's known.
    pub address: String,
    pub label: String,
    /// The card's PCI device id, 0 where it isn't known.
    pub device: u32,
}

impl CardChoice {
    /// Whether this is the card `machine` runs on. A machine given setup's NVIDIA card by its address
    /// is on the same card as one given none.
    pub fn is_machines(&self, machine: &Machine) -> bool {
        let by_address = !machine.amd_gpu.is_empty() && self.address == machine.amd_gpu;
        self.pci == machine.amd_gpu || (by_address && machine.windows_maker() == Some(self.maker))
    }
}

fn offered(maker: GpuMaker, intel_offered: bool) -> bool {
    match maker {
        GpuMaker::Nvidia | GpuMaker::Amd => true,
        GpuMaker::Intel => intel_offered,
    }
}

/// Each NVIDIA card GPU-PV handles, if this PC has one. Setup's card comes first, with no address.
/// Alone it's named as it always was, and beside others each is named with its address. A setup
/// whose card can't be told, or has gone, still has its entry, so a machine on it shows that.
fn nvidia_choices(context: &Context) -> Vec<CardChoice> {
    let cards = find_cards(context);
    // The best NVIDIA card comes first, so if that one isn't handled none is.
    if !cards.first().is_some_and(|card| card.support != Support::Unsupported) {
        return Vec::new();
    }
    let setups = card_in_use(context);
    let setups_address = setups.as_ref().map(|card| card.pci.clone()).unwrap_or_default();
    let others: Vec<Card> = cards
        .into_iter()
        .filter(|card| card.support != Support::Unsupported && card.pci != setups_address)
        .collect();
    let named = |card: &Card| {
        if others.is_empty() { card.name.clone() } else { format!("{} at {}", card.name, card.pci) }
    };
    let setups_label = match &setups {
        Some(card) => named(card),
        None if others.is_empty() => "NVIDIA".to_owned(),
        None => "The NVIDIA card setup chose".to_owned(),
    };
    let mut choices = vec![CardChoice {
        maker: GpuMaker::Nvidia,
        pci: String::new(),
        address: setups_address,
        label: setups_label,
        device: setups.map_or(0, |card| card.device),
    }];
    choices.extend(others.iter().map(|card| CardChoice {
        maker: GpuMaker::Nvidia,
        pci: card.pci.clone(),
        address: card.pci.clone(),
        label: named(card),
        device: card.device,
    }));
    choices
}

fn choices_offering(context: &Context, intel_offered: bool) -> Vec<CardChoice> {
    let mut choices = Vec::new();
    for maker in GpuMaker::ALL.into_iter().filter(|maker| offered(*maker, intel_offered)) {
        match maker {
            GpuMaker::Nvidia => choices.extend(nvidia_choices(context)),
            GpuMaker::Amd | GpuMaker::Intel => {
                let cards = match maker {
                    GpuMaker::Intel => supported_intel_cards(context),
                    _ => render_cards(context, maker)
                        .into_iter()
                        .filter(|card| amd_support(context, card) != Support::Unsupported)
                        .collect(),
                };
                choices.extend(cards.into_iter().map(|card| CardChoice {
                    maker,
                    label: format!("{} at {}", card.name, card.pci),
                    address: card.pci.clone(),
                    pci: card.pci,
                    device: card.device,
                }));
            }
        }
    }
    choices
}

/// Every card here a new Windows machine could run on, NVIDIA's first.
pub fn windows_card_choices(context: &Context) -> Vec<CardChoice> {
    choices_offering(context, OFFER_INTEL_CARDS)
}

/// The cards an existing machine may move to: those of its own maker. The one it has stays listed even
/// when it's gone from this PC, so the choice still shows it.
pub fn same_maker_choices(context: &Context, machine: &Machine) -> Vec<CardChoice> {
    let mut choices: Vec<CardChoice> = windows_card_choices(context)
        .into_iter()
        .filter(|choice| machine.card_change_problem(choice.maker).is_none())
        .collect();
    if !choices.iter().any(|choice| choice.is_machines(machine)) {
        let label = match machine.amd_gpu.as_str() {
            "" => "The NVIDIA card setup chose, which isn't in this PC now".to_owned(),
            pci => format!("The card at {pci}, which isn't in this PC now"),
        };
        let maker = machine.windows_maker().unwrap_or(GpuMaker::Nvidia);
        let pci = machine.amd_gpu.clone();
        choices.push(CardChoice { maker, address: pci.clone(), pci, label, device: 0 });
    }
    choices
}

/// Writes down the memory of the NVIDIA card a machine was given by its address, if setup hasn't, so
/// its video memory cap goes by that card. Until it's known the machine can only have no cap, so a
/// failure here costs no more than that.
pub fn learn_machines_card(context: &Context, machine: &Machine) {
    if let (Some(GpuMaker::Nvidia), Some(pci)) = (machine.windows_maker(), machine.windows_card_address()) {
        let _ = learn_card_memory(context, pci);
    }
}

/// Which of `choices` is the card `machine` runs on, or the first if none is.
pub fn machines_choice(choices: &[CardChoice], machine: &Machine) -> usize {
    choices.iter().position(|choice| choice.is_machines(machine)).unwrap_or(0)
}

/// The choice for the card at `address`, as a person names it: setup's NVIDIA card by its address too.
pub fn choice_at<'a>(choices: &'a [CardChoice], address: &str) -> Option<&'a CardChoice> {
    choices.iter().find(|choice| !choice.address.is_empty() && choice.address == address.trim())
}

/// Which of `choices` a new machine starts on: NVIDIA's card if this PC has one, and else the first AMD.
pub fn default_choice(context: &Context, choices: &[CardChoice]) -> usize {
    let usual = default_linux_gpu(context);
    choices.iter().position(|choice| choice.pci == usual).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::amd::stand_in::{fake_graphics, fake_render_node};
    use crate::setup::cards::stand_in::{NAMES, fake_pci};
    use crate::testing::{Scratch, scratch_context};

    const IGPU: &str = "0000:7c:00.0";
    const RDNA3_GRAPHICS: u32 = 11;
    const POLARIS_GRAPHICS: u32 = 8;

    const INTEL_IGPU: &str = "0000:00:02.0";

    fn with_cards(scratch: &Scratch, nvidia: bool) -> Context {
        let mut context = scratch_context(scratch);
        context.pci_devices = scratch.path("pci");
        context.pci_names = scratch.write("pci.ids", NAMES);
        if nvidia {
            fake_pci(scratch, "0000:01:00.0", 0x2B85, "0x10de", "0x030000");
        }
        fake_render_node(scratch, 128, IGPU, "0x1002", 0x13c0);
        fake_graphics(scratch, IGPU, RDNA3_GRAPHICS);
        fake_render_node(scratch, 129, INTEL_IGPU, "0x8086", 0x64a0);
        context
    }

    #[test]
    fn a_new_machine_is_offered_every_card_and_starts_on_nvidias() {
        let scratch = Scratch::new();
        let context = with_cards(&scratch, true);
        let choices = windows_card_choices(&context);
        let addresses: Vec<&str> = choices.iter().map(|choice| choice.pci.as_str()).collect();
        assert_eq!(addresses, ["", IGPU, INTEL_IGPU]);
        assert_eq!(choices[0].label, "NVIDIA GeForce RTX 5090", "one NVIDIA card is named as it always was");
        assert_eq!(choices[0].address, "0000:01:00.0");
        assert!(choices[1].label.ends_with("at 0000:7c:00.0"), "{choices:?}");
        assert_eq!((choices[0].device, choices[1].device), (0x2B85, 0x13C0));
        assert_eq!(default_choice(&context, &choices), 0);
    }

    const SECOND: &str = "0000:02:00.0";

    fn with_two_nvidia_cards(scratch: &Scratch) -> Context {
        let context = with_cards(scratch, true);
        fake_pci(scratch, SECOND, 0x2C02, "0x10de", "0x030000");
        context
    }

    fn setup_chose(context: &Context, card: serde_json::Value) {
        let fields = serde_json::json!({ "card": card }).as_object().cloned().unwrap();
        crate::settings::write_setup(context, fields).unwrap();
    }

    fn addresses(choices: &[CardChoice]) -> Vec<&str> {
        choices.iter().map(|choice| choice.pci.as_str()).collect()
    }

    #[test]
    fn each_nvidia_card_is_offered_and_setups_comes_first_with_no_address() {
        let scratch = Scratch::new();
        let context = with_two_nvidia_cards(&scratch);
        let choices = windows_card_choices(&context);
        assert_eq!(addresses(&choices), ["", SECOND, IGPU, INTEL_IGPU]);
        assert_eq!(choices[0].label, "NVIDIA GeForce RTX 5090 at 0000:01:00.0");
        assert_eq!(choices[1].label, "NVIDIA GeForce RTX 5080 at 0000:02:00.0");
        assert_eq!((choices[0].device, choices[1].device), (0x2B85, 0x2C02));
        assert_eq!((choices[0].address.as_str(), choices[1].address.as_str()), ("0000:01:00.0", SECOND));
        assert_eq!(default_choice(&context, &choices), 0, "a new machine starts on setup's card");
        setup_chose(&context, serde_json::json!({"device": 0x2C02, "pci": SECOND}));
        let choices = windows_card_choices(&context);
        assert_eq!(addresses(&choices), ["", "0000:01:00.0", IGPU, INTEL_IGPU]);
        assert_eq!(choices[0].label, "NVIDIA GeForce RTX 5080 at 0000:02:00.0");
    }

    #[test]
    fn a_machine_from_before_stays_on_setups_card_and_another_can_be_on_the_second() {
        let scratch = Scratch::new();
        let context = with_two_nvidia_cards(&scratch);
        setup_chose(&context, serde_json::json!({"device": 0x2B85, "pci": "0000:01:00.0"}));
        let old = Machine::new("w", "W", "/d", "/v");
        let offered = same_maker_choices(&context, &old);
        assert_eq!(addresses(&offered), ["", SECOND]);
        assert_eq!(machines_choice(&offered, &old), 0);
        let mut on_second = old.clone();
        on_second.use_windows_card(offered[1].maker, &offered[1].pci);
        assert_eq!(on_second.windows_maker(), Some(GpuMaker::Nvidia), "not taken for an AMD card");
        let offered = same_maker_choices(&context, &on_second);
        assert_eq!((addresses(&offered), machines_choice(&offered, &on_second)), (vec!["", SECOND], 1));
        let mut on_setups_by_address = old.clone();
        on_setups_by_address.use_windows_card(GpuMaker::Nvidia, "0000:01:00.0");
        let offered = same_maker_choices(&context, &on_setups_by_address);
        assert_eq!(addresses(&offered), ["", SECOND], "setup's card isn't listed twice");
        assert_eq!(machines_choice(&offered, &on_setups_by_address), 0);
        let mut on_gone = old;
        on_gone.use_windows_card(GpuMaker::Nvidia, "0000:03:00.0");
        let offered = same_maker_choices(&context, &on_gone);
        assert_eq!(addresses(&offered), ["", SECOND, "0000:03:00.0"]);
        assert_eq!(offered[2].maker, GpuMaker::Nvidia);
        assert!(offered[2].label.contains("isn't in this PC now"), "{offered:?}");
    }

    #[test]
    fn a_setup_whose_card_cannot_be_told_keeps_its_entry_beside_each_card() {
        let scratch = Scratch::new();
        let context = with_two_nvidia_cards(&scratch);
        setup_chose(&context, serde_json::json!({"device": 0x2B85}));
        let choices = windows_card_choices(&context);
        assert_eq!(addresses(&choices)[..3], ["", "0000:01:00.0", SECOND]);
        assert_eq!(choices[0].label, "The NVIDIA card setup chose");
        assert_eq!(machines_choice(&choices, &Machine::new("w", "W", "/d", "/v")), 0);
    }

    #[test]
    fn a_card_named_by_its_address_is_setups_entry_when_it_is_setups_card() {
        let scratch = Scratch::new();
        let context = with_two_nvidia_cards(&scratch);
        let choices = windows_card_choices(&context);
        assert_eq!(choice_at(&choices, "0000:01:00.0").map(|choice| choice.pci.as_str()), Some(""));
        assert_eq!(choice_at(&choices, SECOND).map(|choice| choice.pci.as_str()), Some(SECOND));
        assert_eq!(choice_at(&choices, IGPU).map(|choice| choice.maker), Some(GpuMaker::Amd));
        assert_eq!(choice_at(&choices, "0000:09:00.0"), None);
        assert_eq!(choice_at(&choices, ""), None, "no address names no card");
    }

    #[test]
    fn an_unsupported_second_nvidia_card_is_not_offered_and_changes_nothing() {
        let scratch = Scratch::new();
        let context = with_cards(&scratch, true);
        fake_pci(&scratch, SECOND, 0x1B80, "0x10de", "0x030000");
        let choices = windows_card_choices(&context);
        assert_eq!(addresses(&choices), ["", IGPU, INTEL_IGPU]);
        assert_eq!(choices[0].label, "NVIDIA GeForce RTX 5090");
    }

    #[test]
    fn with_no_nvidia_card_a_new_machine_starts_on_amds() {
        let scratch = Scratch::new();
        let context = with_cards(&scratch, false);
        let choices = windows_card_choices(&context);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[default_choice(&context, &choices)].pci, IGPU);
    }

    #[test]
    fn a_machine_is_offered_only_its_own_makers_cards() {
        let scratch = Scratch::new();
        let context = with_cards(&scratch, true);
        let on_nvidia = Machine::new("w", "W", "/d", "/v");
        let offered = same_maker_choices(&context, &on_nvidia);
        assert_eq!(offered.iter().map(|choice| choice.pci.as_str()).collect::<Vec<_>>(), [""]);
        let on_gone_amd = Machine { amd_gpu: "0000:0a:00.0".into(), ..on_nvidia };
        let offered = same_maker_choices(&context, &on_gone_amd);
        let addresses: Vec<&str> = offered.iter().map(|choice| choice.pci.as_str()).collect();
        assert_eq!(addresses, [IGPU, "0000:0a:00.0"]);
        assert!(offered[1].label.contains("isn't in this PC now"));
    }

    #[test]
    fn a_card_gpu_pv_does_not_handle_is_not_offered() {
        let scratch = Scratch::new();
        let context = with_cards(&scratch, false);
        fake_render_node(&scratch, 130, "0000:03:00.0", "0x1002", 0x67df);
        fake_graphics(&scratch, "0000:03:00.0", POLARIS_GRAPHICS);
        // An NVIDIA card from before the ones the daemon knows.
        fake_pci(&scratch, "0000:01:00.0", 0x1B80, "0x10de", "0x030000");
        let choices = windows_card_choices(&context);
        assert_eq!(choices.iter().map(|choice| choice.pci.as_str()).collect::<Vec<_>>(), [IGPU, INTEL_IGPU]);
    }

    #[test]
    fn intel_cards_are_offered_after_the_others_while_the_switch_is_on_and_not_when_it_is_off() {
        let scratch = Scratch::new();
        let context = with_cards(&scratch, true);
        let offered_now: Vec<GpuMaker> =
            windows_card_choices(&context).iter().map(|choice| choice.maker).collect();
        assert_eq!(offered_now, [GpuMaker::Nvidia, GpuMaker::Amd, GpuMaker::Intel]);
        let without: Vec<GpuMaker> =
            choices_offering(&context, false).iter().map(|choice| choice.maker).collect();
        assert_eq!(without, [GpuMaker::Nvidia, GpuMaker::Amd]);
        let all = choices_offering(&context, true);
        let intel: Vec<&str> = all
            .iter()
            .filter(|choice| choice.maker == GpuMaker::Intel)
            .map(|choice| choice.pci.as_str())
            .collect();
        assert_eq!(intel, [INTEL_IGPU]);
    }

    #[test]
    fn a_machine_on_intel_is_offered_only_intels_cards() {
        let scratch = Scratch::new();
        let context = with_cards(&scratch, true);
        let mut machine = Machine::new("w", "W", "/d", "/v");
        machine.use_windows_card(GpuMaker::Intel, INTEL_IGPU);
        // Listing as the switch will, with Intel on.
        let own: Vec<String> = choices_offering(&context, true)
            .into_iter()
            .filter(|choice| machine.card_change_problem(choice.maker).is_none())
            .map(|choice| choice.pci)
            .collect();
        assert_eq!(own, [INTEL_IGPU]);
    }
}
