//! What `guesthouse-clock-helper` decides, apart from the card and the files it does it with.
//!
//! An NVIDIA card left to clock itself stays at its idle clocks through the short bursts of a desktop
//! that changes now and then, and a guest's encoder then takes three or four times as long for each
//! picture. Holding the card's memory clock up fixes most of it. That takes root, so install.sh puts
//! this program in setuid root, and the remote view starts it while a viewer streams.
//!
//! It takes two words and nothing else: `hold ADDRESS` or `reset ADDRESS`. The address is a PCI
//! address, checked here to be one. The clock is the card's own highest memory clock, as the card
//! says at run time, so no number comes from the caller.
//!
//! `hold` locks the clock and keeps it locked until its standard input closes or it is told to end.
//! Then it puts the clock back, unless another holder still holds the same card. Holders count
//! themselves through a shared lock on one file per card, which the kernel lets go of when a
//! holder dies however it dies. A holder that couldn't put the clock back leaves a mark, and `reset`
//! puts back a card that has the mark and no holders. A clock somebody locked by hand has no mark,
//! so it is left alone.

use std::fmt;

pub const HOLD: &str = "hold";
pub const RESET: &str = "reset";
pub const USAGE: &str = "usage: guesthouse-clock-helper hold|reset DOMAIN:BUS:DEVICE.FUNCTION";
/// Longer than any PCI address, with the domain in eight digits as `nvidia-smi` writes it.
const LONGEST_ADDRESS: usize = 16;
/// A clock above this isn't believed: the fastest memory there is runs at a few tens of thousands.
const HIGHEST_BELIEVABLE_MHZ: u32 = 100_000;

/// What the program exits with, so the remote view can say in one line why nothing was held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Done = 0,
    Failed = 1,
    Usage = 2,
    /// Not running as root: a build of the repo, a copy without the setuid bit, or a `nosuid` mount.
    NotRoot = 3,
    /// There's no NVIDIA display card at that address.
    NotNvidia = 4,
    /// NVIDIA's management library isn't there, or doesn't know the card.
    NoLibrary = 5,
    /// The card can't have its memory clock locked.
    NotSupported = 6,
    /// The user who started it can't open the card's device file, so the card isn't theirs to change.
    NotTheirs = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Hold,
    Reset,
}

/// What the program was asked to do, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    pub verb: Verb,
    /// The card's address as sysfs writes it: `0000:01:00.0`.
    pub card: String,
}

/// Why the words were refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{USAGE}", self.0)
    }
}

/// Reads the words after the program's name. Exactly two, each plain text, or nothing is done.
pub fn parse<I, S>(words: I) -> Result<Order, Refusal>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let words: Vec<String> = words
        .into_iter()
        .map(|word| word.as_ref().to_str().map(str::to_owned))
        .collect::<Option<_>>()
        .ok_or_else(|| Refusal("Every word must be plain text.".into()))?;
    let [verb, card] = &words[..] else {
        return Err(Refusal(format!("It takes two words, not {}.", words.len())));
    };
    let verb = match verb.as_str() {
        HOLD => Verb::Hold,
        RESET => Verb::Reset,
        other => return Err(Refusal(format!("{other:?} is not hold or reset."))),
    };
    Ok(Order { verb, card: pci_address(card)? })
}

fn hex_field(text: &str, digits: usize) -> Option<u32> {
    (text.len() == digits && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(text, 16).ok())
        .flatten()
}

/// A PCI address as sysfs writes it, `0000:01:00.0`, from one written that way or with the eight-digit
/// domain `nvidia-smi` writes. Anything else is refused, so the address can name a file safely.
pub fn pci_address(text: &str) -> Result<String, Refusal> {
    let refuse = || Refusal(format!("{text:?} is not a PCI address such as 0000:01:00.0."));
    if text.len() > LONGEST_ADDRESS {
        return Err(refuse());
    }
    let (domain, rest) = text.split_once(':').ok_or_else(refuse)?;
    let (bus, rest) = rest.split_once(':').ok_or_else(refuse)?;
    let (device, function) = rest.split_once('.').ok_or_else(refuse)?;
    let domain = hex_field(domain, 4).or_else(|| hex_field(domain, 8)).filter(|domain| *domain <= 0xffff);
    let bus = hex_field(bus, 2);
    let device = hex_field(device, 2).filter(|device| *device < 0x20);
    let function = hex_field(function, 1).filter(|function| *function < 8);
    match (domain, bus, device, function) {
        (Some(domain), Some(bus), Some(device), Some(function)) => {
            Ok(format!("{domain:04x}:{bus:02x}:{device:02x}.{function:x}"))
        }
        _ => Err(refuse()),
    }
}

/// The clock to hold: the highest the card says it supports. None if it says nothing believable.
pub fn highest_clock(supported: &[u32]) -> Option<u32> {
    supported.iter().copied().filter(|mhz| (1..=HIGHEST_BELIEVABLE_MHZ).contains(mhz)).max()
}

/// Whether a card is an NVIDIA display card, from its sysfs `vendor` and `class` files.
pub fn is_nvidia_display(vendor: &str, class: &str) -> bool {
    // The class's top byte is 03 for every kind of display controller, 3D ones too.
    vendor.trim().eq_ignore_ascii_case("0x10de") && class.trim().to_ascii_lowercase().starts_with("0x03")
}

/// The card's device file, from its file in the driver's `/proc/driver/nvidia/gpus`.
pub fn device_file(information: &str) -> Option<String> {
    let minor = information.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "Device Minor").then(|| value.trim().parse::<u32>().ok())?
    })?;
    Some(format!("/dev/nvidia{minor}"))
}

/// Why the card wouldn't do it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub exit: Exit,
    pub reason: String,
}

impl Fault {
    pub fn new(exit: Exit, reason: impl Into<String>) -> Self {
        Self { exit, reason: reason.into() }
    }
}

/// The card's clocks, through NVIDIA's library or a stand-in.
pub trait Card {
    fn supported_memory_clocks(&mut self) -> Result<Vec<u32>, Fault>;
    fn lock_memory_clock(&mut self, mhz: u32) -> Result<(), Fault>;
    fn reset_memory_clock(&mut self) -> Result<(), Fault>;
}

/// The holders of one card, and the mark a lock of Guesthouse's leaves on it.
pub trait Holders {
    /// Joins the holders, waiting while one is putting the clock back.
    fn join(&mut self) -> Result<(), Fault>;
    /// Leaves, and says whether this was the last holder, who then has the card to itself until it ends.
    fn leave_last(&mut self) -> bool;
    /// Whether nobody holds the card, and if so takes it until the program ends.
    fn take_if_free(&mut self) -> bool;
    fn mark(&mut self, mhz: u32);
    fn marked(&self) -> bool;
    fn unmark(&mut self);
}

/// Locks the card's memory clock at its highest, calls `held` with the clock and lets it wait for as
/// long as the clock is wanted, then puts the clock back if this is the last holder.
pub fn hold(card: &mut dyn Card, holders: &mut dyn Holders, held: impl FnOnce(u32)) -> Result<u32, Fault> {
    holders.join()?;
    // A holder killed before it could put the clock back left its mark.
    let left_behind = holders.marked();
    let locked = card.supported_memory_clocks().and_then(|supported| {
        let mhz = highest_clock(&supported)
            .ok_or_else(|| Fault::new(Exit::NotSupported, "The card says it has no memory clocks."))?;
        // Marked first, so a holder killed half way still leaves the mark that `reset` looks for.
        holders.mark(mhz);
        card.lock_memory_clock(mhz).map(|()| mhz)
    });
    if let Ok(mhz) = locked {
        held(mhz);
    }
    let last = holders.leave_last();
    if last && holders.marked() && locked.is_err() && !left_behind {
        // Nothing was changed, so there's nothing to put back, and a lock by hand stays.
        holders.unmark();
    } else if last && holders.marked() {
        let put_back = card.reset_memory_clock();
        if put_back.is_ok() {
            holders.unmark();
        }
        // A lock that never happened has nothing to put back, and its own fault says more.
        if locked.is_ok() {
            put_back?;
        }
    }
    locked
}

/// What `reset` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swept {
    /// A lock of Guesthouse's was left with no holder, and is put back.
    PutBack,
    /// Somebody still holds the card.
    Held,
    /// Nothing of Guesthouse's is there. A clock locked by hand is left as it is.
    Nothing,
}

/// Puts back a lock a holder left behind, if nobody holds the card now. The card is only opened when
/// there is something to put back, so a reset with nothing to do never loads NVIDIA's library.
pub fn reset<C: Card>(
    holders: &mut dyn Holders,
    open: impl FnOnce() -> Result<C, Fault>,
) -> Result<Swept, Fault> {
    if !holders.take_if_free() {
        return Ok(Swept::Held);
    }
    if !holders.marked() {
        return Ok(Swept::Nothing);
    }
    open()?.reset_memory_clock()?;
    holders.unmark();
    Ok(Swept::PutBack)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_device_file_is_the_one_the_driver_lists() {
        let information = "Model: \t\t NVIDIA Card\nBus Location: \t 0000:01:00.0\nDevice Minor: \t 1\n";
        assert_eq!(super::device_file(information).as_deref(), Some("/dev/nvidia1"));
        assert_eq!(super::device_file("Device Minor: x\n"), None);
        assert_eq!(super::device_file(""), None);
    }

    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Shared {
        holders: u32,
        taken: bool,
        mark: Option<u32>,
        locked: Option<u32>,
        resets: u32,
    }

    #[derive(Clone)]
    struct StandInCard {
        state: Rc<RefCell<Shared>>,
        supported: Result<Vec<u32>, Fault>,
        lock_fails: bool,
    }

    impl Card for StandInCard {
        fn supported_memory_clocks(&mut self) -> Result<Vec<u32>, Fault> {
            self.supported.clone()
        }
        fn lock_memory_clock(&mut self, mhz: u32) -> Result<(), Fault> {
            if self.lock_fails {
                return Err(Fault::new(Exit::NotSupported, "not on this card"));
            }
            self.state.borrow_mut().locked = Some(mhz);
            Ok(())
        }
        fn reset_memory_clock(&mut self) -> Result<(), Fault> {
            let mut state = self.state.borrow_mut();
            state.locked = None;
            state.resets += 1;
            Ok(())
        }
    }

    struct StandInHolders {
        state: Rc<RefCell<Shared>>,
        joined: bool,
    }

    impl Holders for StandInHolders {
        fn join(&mut self) -> Result<(), Fault> {
            self.state.borrow_mut().holders += 1;
            self.joined = true;
            Ok(())
        }
        fn leave_last(&mut self) -> bool {
            let mut state = self.state.borrow_mut();
            if self.joined {
                state.holders -= 1;
                self.joined = false;
            }
            state.holders == 0
        }
        fn take_if_free(&mut self) -> bool {
            let mut state = self.state.borrow_mut();
            state.taken = state.holders == 0;
            state.taken
        }
        fn mark(&mut self, mhz: u32) {
            self.state.borrow_mut().mark = Some(mhz);
        }
        fn marked(&self) -> bool {
            self.state.borrow().mark.is_some()
        }
        fn unmark(&mut self) {
            self.state.borrow_mut().mark = None;
        }
    }

    fn rig(state: &Rc<RefCell<Shared>>) -> (StandInCard, StandInHolders) {
        (
            StandInCard {
                state: Rc::clone(state),
                supported: Ok(vec![405, 810, 7001, 13801, 14001]),
                lock_fails: false,
            },
            StandInHolders { state: Rc::clone(state), joined: false },
        )
    }

    #[test]
    fn only_a_verb_and_a_pci_address_are_taken() {
        assert_eq!(
            parse(["hold", "0000:01:00.0"]),
            Ok(Order { verb: Verb::Hold, card: "0000:01:00.0".into() })
        );
        assert_eq!(parse(["reset", "00000000:0A:00.0"]).unwrap().card, "0000:0a:00.0");
        for words in [
            vec![],
            vec!["hold"],
            vec!["hold", "0000:01:00.0", "9501"],
            vec!["lock", "0000:01:00.0"],
            vec!["HOLD", "0000:01:00.0"],
            vec!["--hold", "0000:01:00.0"],
        ] {
            assert!(parse(&words).is_err(), "{words:?}");
        }
    }

    #[test]
    fn an_address_that_could_name_another_file_is_refused() {
        for text in [
            "",
            "0000:01:00",
            "0000:01:00.8",
            "0000:01:20.0",
            "0000:1:00.0",
            "000:01:00.0",
            "0000:01:00.0/../x",
            "../../etc/passwd",
            "0000:01:00.0 ",
            "0000:01:00.0\n",
            "g000:01:00.0",
            "+000:01:00.0",
            "0001ffff:01:00.0",
            "0000:01:00.0.0",
        ] {
            assert!(pci_address(text).is_err(), "{text:?}");
        }
        assert_eq!(pci_address("0000:41:1F.7").unwrap(), "0000:41:1f.7");
    }

    #[test]
    fn the_clock_is_the_highest_the_card_says_it_has() {
        assert_eq!(highest_clock(&[405, 9501, 810, 9251]), Some(9501));
        assert_eq!(highest_clock(&[]), None);
        assert_eq!(highest_clock(&[0, u32::MAX]), None, "nothing believable");
    }

    #[test]
    fn only_an_nvidia_display_card_is_one() {
        assert!(is_nvidia_display("0x10de\n", "0x030000\n"));
        assert!(is_nvidia_display("0x10DE", "0x030200"));
        assert!(!is_nvidia_display("0x10de", "0x040300"), "the card's sound function");
        assert!(!is_nvidia_display("0x1002", "0x030000"));
    }

    #[test]
    fn a_holder_puts_the_clock_back_when_it_ends() {
        let state = Rc::default();
        let (mut card, mut holders) = rig(&state);
        let mut while_held = None;
        let mhz = hold(&mut card, &mut holders, |mhz| while_held = Some(state.borrow().locked == Some(mhz)));
        assert_eq!(mhz, Ok(14001));
        assert_eq!(while_held, Some(true), "locked at the highest while held");
        let state = state.borrow();
        assert_eq!((state.locked, state.mark, state.resets, state.holders), (None, None, 1, 0));
    }

    #[test]
    fn the_last_of_several_holders_puts_it_back() {
        let state: Rc<RefCell<Shared>> = Rc::default();
        // Another machine on the same card holds it already.
        state.borrow_mut().holders = 1;
        let (mut card, mut holders) = rig(&state);
        hold(&mut card, &mut holders, |_| {}).unwrap();
        assert_eq!(state.borrow().locked, Some(14001), "still held for the other");
        assert_eq!(state.borrow().resets, 0);
        // The other one ends.
        let (card, mut holders) = rig(&state);
        holders.joined = true;
        assert!(holders.leave_last());
        assert_eq!(reset(&mut holders, || Ok(card.clone())), Ok(Swept::PutBack));
        assert_eq!(state.borrow().locked, None);
    }

    #[test]
    fn a_card_that_cannot_lock_is_left_as_it_was() {
        let state = Rc::default();
        let (mut card, mut holders) = rig(&state);
        card.lock_fails = true;
        let mut called = false;
        let fault = hold(&mut card, &mut holders, |_| called = true).unwrap_err();
        assert_eq!(fault.exit, Exit::NotSupported);
        assert!(!called, "nothing to wait for");
        assert_eq!(state.borrow().mark, None, "no mark left for reset to find");
        assert_eq!(state.borrow().resets, 0, "nothing was locked, so nothing is put back");

        let (mut card, mut holders) = rig(&state);
        card.supported = Ok(vec![]);
        assert_eq!(hold(&mut card, &mut holders, |_| {}).unwrap_err().exit, Exit::NotSupported);
        assert_eq!(state.borrow().resets, 0, "nothing was locked");
    }

    #[test]
    fn reset_leaves_a_clock_locked_by_hand_and_one_still_held() {
        let state: Rc<RefCell<Shared>> = Rc::default();
        let (card, mut holders) = rig(&state);
        state.borrow_mut().locked = Some(9501);
        assert_eq!(reset(&mut holders, || Ok(card.clone())), Ok(Swept::Nothing));
        assert_eq!(state.borrow().locked, Some(9501), "somebody's own lock");
        let never =
            || -> Result<StandInCard, Fault> { panic!("the card was opened with nothing to put back") };
        assert_eq!(reset(&mut holders, never), Ok(Swept::Nothing));

        state.borrow_mut().mark = Some(9501);
        state.borrow_mut().holders = 1;
        assert_eq!(reset(&mut holders, || Ok(card.clone())), Ok(Swept::Held));
        assert_eq!(state.borrow().locked, Some(9501));

        // The holder was killed: the kernel let go of its share, and its mark is still there.
        state.borrow_mut().holders = 0;
        assert_eq!(reset(&mut holders, || Ok(card.clone())), Ok(Swept::PutBack));
        assert_eq!((state.borrow().locked, state.borrow().mark), (None, None));
    }
}
