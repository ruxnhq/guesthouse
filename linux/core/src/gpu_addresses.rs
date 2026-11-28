//! Handing out GPU addresses, and taking them back.

pub use guesthouse_backend::GPU_RANGES;
use guesthouse_backend::{BackendError, GpuRange};
use guesthouse_messages::wire::align_up;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Past the last address any mapping can have, which also makes it more than any size can be.
pub const ADDRESSES_END: u64 = GPU_RANGES[1].end;

/// The alignment of ordinary mappings.
pub const ALLOCATION_ALIGN: u64 = 64 << 10;

/// The alignment of mappings big enough for 2 MiB pages, which must start on a 2 MiB boundary.
pub const LARGE_ALIGN: u64 = 2 << 20;

/// What a range of addresses is for. A range only ever goes back to the same use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressUse {
    /// The host's own, for channels and copies. Once in use, these are never freed.
    Host,
    /// A slot of the frame ring, which comes and goes with viewers and screen modes.
    FrameRing,
    /// A whole allocation in video memory, mapped where the host chose.
    Mapped,
    /// The same in system memory: the guest's own pages, or video memory a first lock moved there.
    MappedSystem,
    /// Set aside by the guest, for mappings it places itself.
    Reserved,
}

impl AddressUse {
    /// The use for an allocation mapped where the host chose, by the memory behind it.
    pub fn mapped(system_memory: bool) -> Self {
        if system_memory { Self::MappedSystem } else { Self::Mapped }
    }

    /// Where a range can come from once no new addresses are left: a range that held the other kind
    /// of memory, which is better than refusing the mapping.
    fn last_resort(self) -> Option<Self> {
        match self {
            Self::Mapped => Some(Self::MappedSystem),
            Self::MappedSystem => Some(Self::Mapped),
            Self::Host | Self::FrameRing | Self::Reserved => None,
        }
    }
}

/// A bump allocator with a free list per shape of range. Addresses stay unique across every address
/// space at any one time, so an address always names one allocation.
///
/// A program that makes and frees memory all day would run out of new addresses in the end. Freed
/// ranges are not split or joined, and go back only to the same use, size and alignment, because
/// handing one to a different use has made the vendor's driver refuse mappings.
///
/// A mapped range also goes back only to the kind of memory it held last. Video memory mapped where
/// system memory was has made the vendor's compute launches fault with an invalid address space,
/// however long the range rested and whatever caches were dropped first. Ranges that held video
/// memory go back to video memory without trouble.
#[derive(Debug)]
pub struct GpuAddresses {
    next: u64,
    free: HashMap<(AddressUse, u64, u64), Vec<u64>>,
    /// Where new addresses come from: the ranges, less what a backend keeps for the guest.
    hand_out: Vec<GpuRange>,
    /// (start, end) of each range a guest placed, which hand-outs stay clear of in every space. The
    /// same range can be here twice, as two processes can each hold it.
    placed: Vec<(u64, u64)>,
    /// (start, end) of what the host took for itself.
    host: Vec<(u64, u64)>,
    /// Past the ranges, where only the guest may choose addresses. See
    /// `Backend::guest_placed_gpu_ranges`.
    guest_only: &'static [GpuRange],
    /// How long a range given back rests before it is handed out again, if a backend asked for a
    /// rest. See [`GpuAddresses::holding_freed`].
    hold: Option<Duration>,
    /// When each resting range was given back, by its start. Only kept while there is a hold.
    freed_at: HashMap<u64, Instant>,
}

/// Where a request will accept addresses: from `from`, with everything ending by `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from: u64,
    pub to: u64,
}

impl Window {
    pub const ANYWHERE: Self = Self { from: 0, to: u64::MAX };

    fn holds(&self, base: u64, size: u64) -> bool {
        base >= self.from && size <= self.to.saturating_sub(base)
    }

    fn limit(&self, range: &GpuRange) -> Option<GpuRange> {
        let limited = GpuRange { start: range.start.max(self.from), end: range.end.min(self.to) };
        (limited.start < limited.end).then_some(limited)
    }
}

fn overlaps((start, end): (u64, u64), base: u64, size: u64) -> bool {
    start < base.saturating_add(size) && base < end
}

/// The first place in `range` from `from` on, stepping over what's blocked.
fn first_fit(range: &GpuRange, from: u64, size: u64, align: u64, blocked: &[(u64, u64)]) -> Option<u64> {
    let mut base = align_up(from.max(range.start), align);
    while size <= range.end.saturating_sub(base) {
        match blocked.iter().find(|&&block| overlaps(block, base, size)) {
            Some(&(_, end)) => base = align_up(end, align),
            None => return Some(base),
        }
    }
    None
}

/// `ranges` without `kept`, in order.
fn without(ranges: &[GpuRange], kept: &GpuRange) -> Vec<GpuRange> {
    let mut left = Vec::with_capacity(ranges.len() + 1);
    for range in ranges {
        let before = GpuRange { start: range.start, end: range.end.min(kept.start) };
        let after = GpuRange { start: range.start.max(kept.end), end: range.end };
        left.extend([before, after].into_iter().filter(|part| part.start < part.end));
    }
    left
}

impl GpuAddresses {
    pub fn new() -> Self {
        Self::keeping_clear(&[])
    }

    /// Never hands out anything in `kept`. Those addresses still work, as the guest may map there.
    pub fn keeping_clear(kept: &[GpuRange]) -> Self {
        let hand_out = kept.iter().fold(GPU_RANGES.to_vec(), |left, range| without(&left, range));
        Self {
            next: GPU_RANGES[0].start,
            free: HashMap::new(),
            hand_out,
            placed: Vec::new(),
            host: Vec::new(),
            guest_only: &[],
            hold: None,
            freed_at: HashMap::new(),
        }
    }

    /// Has a range given back rest for `hold` before it is handed out again, in every space and for
    /// every use, while new addresses are handed out meanwhile. `None` hands it out again at once,
    /// as without this. The cost is addresses, not memory: the memory behind a range is gone when
    /// it is given back, and what rests is only its addresses, as much as is given back in `hold`.
    /// When nothing else is left a resting range is handed out anyway.
    pub fn holding_freed(self, hold: Option<Duration>) -> Self {
        Self { hold: hold.filter(|hold| !hold.is_zero()), ..self }
    }

    /// Also lets the guest map at addresses it chose in `ranges`, which are never handed out.
    pub fn with_guest_only(self, ranges: &'static [GpuRange]) -> Self {
        Self { guest_only: ranges, ..self }
    }

    /// How many addresses the guest can be given or place: the ranges this backend hands out,
    /// and its ranges only the guest uses.
    pub fn guest_reach(&self) -> u64 {
        let size = |range: &GpuRange| range.end - range.start;
        self.hand_out.iter().chain(self.guest_only).map(size).fold(0, u64::saturating_add)
    }

    /// Whether the guest may map `size` bytes at `address`, an address it chose: wholly in one of
    /// the ranges, or wholly in a range only the guest uses.
    pub fn guest_may_map(&self, address: u64, size: u64) -> bool {
        let wholly_in = |range: &GpuRange| {
            range.contains(address) && address.checked_add(size).is_some_and(|end| end <= range.end)
        };
        GPU_RANGES.iter().any(wholly_in) || self.guest_only.iter().any(wholly_in)
    }

    /// Takes `size` bytes: what was given back for this use, or new addresses, moving to the next
    /// range if this one is too full.
    pub fn take(&mut self, size: u64, align: u64, usage: AddressUse) -> Result<u64, BackendError> {
        self.take_within(size, align, usage, Window::ANYWHERE)
            .ok_or_else(|| BackendError::new("no room left in the GPU address ranges"))
    }

    /// As [`GpuAddresses::take`], but only if what comes next lies in `window`. Nothing changes if not.
    pub fn take_within(&mut self, size: u64, align: u64, usage: AddressUse, window: Window) -> Option<u64> {
        self.take_clear_of(size, align, usage, window, &[])
    }

    /// As [`GpuAddresses::take_within`], stepping over `mapped`: (start, end) of what is mapped now in
    /// any space. A guest can map pages of its own at addresses it chooses, inside a range that was
    /// handed out for something else, and keep them there after that range was given back.
    pub fn take_clear_of(
        &mut self,
        size: u64,
        align: u64,
        usage: AddressUse,
        window: Window,
        mapped: &[(u64, u64)],
    ) -> Option<u64> {
        let now = Instant::now();
        if let Some(spare) = self.spare(size, align, usage, window, mapped, Some(now)) {
            return Some(spare);
        }
        let Some(base) = self.next_clear(size, align, window, mapped) else {
            let rested =
                |addresses: &mut Self, usage| addresses.spare(size, align, usage, window, mapped, Some(now));
            let anyway =
                |addresses: &mut Self, usage| addresses.spare(size, align, usage, window, mapped, None);
            let other = usage.last_resort();
            return other
                .and_then(|other| rested(self, other))
                .or_else(|| anyway(self, usage))
                .or_else(|| other.and_then(|other| anyway(self, other)));
        };
        self.next = base + size;
        if usage == AddressUse::Host {
            self.host.push((base, base + size));
        }
        Some(base)
    }

    /// The newest range given back for this use that hasn't been placed or mapped over since. One
    /// that has stays on the shelf until it is clear again. With `now`, a range still resting then
    /// stays on the shelf too.
    fn spare(
        &mut self,
        size: u64,
        align: u64,
        usage: AddressUse,
        window: Window,
        mapped: &[(u64, u64)],
        now: Option<Instant>,
    ) -> Option<u64> {
        let spares = self.free.get_mut(&(usage, size, align))?;
        let resting = |base: &u64| match (now, self.hold, self.freed_at.get(base)) {
            (Some(now), Some(hold), Some(&freed)) => now.saturating_duration_since(freed) < hold,
            _ => false,
        };
        let usable = |base: &u64| {
            let blocked = |&range: &(u64, u64)| overlaps(range, *base, size);
            window.holds(*base, size)
                && !self.placed.iter().any(blocked)
                && !mapped.iter().any(blocked)
                && !resting(base)
        };
        let found = spares.iter().rposition(usable)?;
        let base = spares.remove(found);
        self.freed_at.remove(&base);
        Some(base)
    }

    /// The first new address with room in `window`, past whatever the guest has placed or mapped.
    fn next_clear(&self, size: u64, align: u64, window: Window, mapped: &[(u64, u64)]) -> Option<u64> {
        let blocked = [self.placed.as_slice(), mapped].concat();
        let in_window = self.hand_out.iter().filter_map(|range| window.limit(range));
        in_window.into_iter().find_map(|range| first_fit(&range, self.next, size, align, &blocked))
    }

    /// Room in `window`, whatever has been handed out, clear of `taken` and of the host's own ranges.
    /// The caller goes on to [`GpuAddresses::place`] it.
    pub fn find_gap(&self, window: Window, size: u64, align: u64, taken: &[(u64, u64)]) -> Option<u64> {
        let blocked = [taken, &self.host].concat();
        let in_window = self.hand_out.iter().filter_map(|range| window.limit(range));
        in_window.into_iter().find_map(|range| first_fit(&range, range.start, size, align, &blocked))
    }

    /// Whether any addresses this hands out lie in `window`.
    pub fn reaches(&self, window: Window) -> bool {
        self.hand_out.iter().any(|range| window.limit(range).is_some())
    }

    /// Notes a range the guest holds in one of its spaces, which nothing may be handed from.
    pub fn place(&mut self, base: u64, size: u64) {
        self.placed.push((base, base + size));
    }

    /// The opposite of [`GpuAddresses::place`]. Other spaces can hold the same range.
    pub fn unplace(&mut self, base: u64, size: u64) {
        if let Some(index) = self.placed.iter().position(|&range| range == (base, base + size)) {
            self.placed.swap_remove(index);
        }
    }

    /// The host's own ranges. Each is in use in some space, or in all of them.
    pub fn host_ranges(&self) -> &[(u64, u64)] {
        &self.host
    }

    /// Makes a range free for the same use again. What the host mapped there must be gone; pages the
    /// guest mapped there itself can stay, and keep it from being handed out until they go.
    pub fn give_back(&mut self, base: u64, size: u64, align: u64, usage: AddressUse) {
        self.free.entry((usage, size, align)).or_default().push(base);
        if self.hold.is_some() {
            self.freed_at.insert(base, Instant::now());
        }
    }

    pub fn in_a_range(address: u64) -> bool {
        GPU_RANGES.iter().any(|range| range.contains(address))
    }
}

impl Default for GpuAddresses {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_map_lies_wholly_in_one_range() {
        let addresses = GpuAddresses::new();
        assert!(addresses.guest_may_map(0xFFFF_E000, 0x2000));
        assert!(!addresses.guest_may_map(0xFFFF_F000, 0x2000));
        assert!(!addresses.guest_may_map(0x1_0000_0000, 0x1000));
    }

    const HOST: AddressUse = AddressUse::Host;

    #[test]
    fn addresses_follow_one_another() {
        let mut addresses = GpuAddresses::new();
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(0x400_0000));
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(0x401_0000));
    }

    #[test]
    fn alignment_can_leave_a_gap() {
        let mut addresses = GpuAddresses::new();
        addresses.take(0x1000, ALLOCATION_ALIGN, HOST).unwrap();
        assert_eq!(addresses.take(0x1000, LARGE_ALIGN, HOST), Ok(0x420_0000));
    }

    #[test]
    fn what_does_not_fit_moves_to_the_next_range() {
        let mut addresses = GpuAddresses::new();
        assert_eq!(addresses.take(0x1_0000_0000, ALLOCATION_ALIGN, HOST), Ok(GPU_RANGES[1].start));
    }

    #[test]
    fn the_hole_is_not_in_a_range() {
        assert!(!GpuAddresses::in_a_range(0x1_0000_0000));
        assert!(GpuAddresses::in_a_range(0x1_4000_0000));
    }

    #[test]
    fn a_kept_range_is_jumped_over_and_still_mappable() {
        let kept = GpuRange { start: 0x800_0000, end: GPU_RANGES[0].end };
        let mut addresses = GpuAddresses::keeping_clear(&[kept]);
        assert_eq!(addresses.take(0x400_0000, ALLOCATION_ALIGN, HOST), Ok(0x400_0000));
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(GPU_RANGES[1].start));
        assert!(GpuAddresses::in_a_range(kept.start));
    }

    #[test]
    fn a_kept_range_in_the_middle_leaves_both_sides() {
        let kept = GpuRange { start: 0x500_0000, end: 0x600_0000 };
        let mut addresses = GpuAddresses::keeping_clear(&[kept]);
        assert_eq!(addresses.take(0x100_0000, ALLOCATION_ALIGN, HOST), Ok(0x400_0000));
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(0x600_0000));
    }

    const PLACED_AT: u64 = 0x401_0000;

    #[test]
    fn new_addresses_step_over_what_a_guest_placed() {
        let mut addresses = GpuAddresses::new();
        addresses.place(PLACED_AT, 0x2_0000);
        assert_eq!(addresses.take(0x1_0000, ALLOCATION_ALIGN, HOST), Ok(0x400_0000));
        assert_eq!(addresses.take(0x1_0000, ALLOCATION_ALIGN, HOST), Ok(PLACED_AT + 0x2_0000));
    }

    #[test]
    fn a_spare_range_that_was_placed_over_stays_on_the_shelf() {
        let usage = AddressUse::Mapped;
        let mut addresses = GpuAddresses::new();
        addresses.give_back(PLACED_AT, 0x1_0000, ALLOCATION_ALIGN, usage);
        addresses.place(PLACED_AT, 0x1_0000);
        assert_ne!(addresses.take(0x1_0000, ALLOCATION_ALIGN, usage), Ok(PLACED_AT));
        addresses.unplace(PLACED_AT, 0x1_0000);
        assert_eq!(addresses.take(0x1_0000, ALLOCATION_ALIGN, usage), Ok(PLACED_AT));
    }

    #[test]
    fn a_spare_range_with_a_page_still_mapped_in_it_stays_on_the_shelf() {
        let usage = AddressUse::Mapped;
        let mut addresses = GpuAddresses::new();
        addresses.give_back(PLACED_AT, 0x2_0000, ALLOCATION_ALIGN, usage);
        let page = [(PLACED_AT + 0x1_0000, PLACED_AT + 0x2_0000)];
        let taken = addresses.take_clear_of(0x2_0000, ALLOCATION_ALIGN, usage, Window::ANYWHERE, &page);
        assert_ne!(taken, Some(PLACED_AT));
        let again = addresses.take_clear_of(0x2_0000, ALLOCATION_ALIGN, usage, Window::ANYWHERE, &[]);
        assert_eq!(again, Some(PLACED_AT));
    }

    #[test]
    fn new_addresses_step_over_what_is_mapped() {
        let mut addresses = GpuAddresses::new();
        let page = [(0x400_0000, 0x401_0000)];
        let taken = addresses.take_clear_of(0x1_0000, ALLOCATION_ALIGN, HOST, Window::ANYWHERE, &page);
        assert_eq!(taken, Some(0x401_0000));
    }

    #[test]
    fn a_range_two_spaces_placed_stays_clear_until_both_let_go() {
        let mut addresses = GpuAddresses::new();
        addresses.place(0x400_0000, 0x1_0000);
        addresses.place(0x400_0000, 0x1_0000);
        addresses.unplace(0x400_0000, 0x1_0000);
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(PLACED_AT));
    }

    #[test]
    fn a_window_ahead_of_the_next_address_is_taken_from_its_start() {
        let mut addresses = GpuAddresses::new();
        let window = Window { from: 0x500_0000, to: 0x600_0000 };
        assert_eq!(addresses.take_within(0x1000, ALLOCATION_ALIGN, HOST, window), Some(0x500_0000));
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(0x501_0000));
    }

    #[test]
    fn a_window_behind_the_next_address_has_nothing_new_and_changes_nothing() {
        let mut addresses = GpuAddresses::new();
        addresses.take(0x200_0000, ALLOCATION_ALIGN, HOST).unwrap();
        let window = Window { from: 0x400_0000, to: 0x500_0000 };
        assert_eq!(addresses.take_within(0x1000, ALLOCATION_ALIGN, AddressUse::Mapped, window), None);
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(0x600_0000));
    }

    #[test]
    fn a_gap_is_found_behind_the_next_address_and_clear_of_the_host() {
        let mut addresses = GpuAddresses::new();
        addresses.take(0x1_0000, ALLOCATION_ALIGN, HOST).unwrap();
        addresses.take(0x1000_0000, ALLOCATION_ALIGN, AddressUse::Mapped).unwrap();
        let window = Window { from: 0, to: 0x800_0000 };
        let gap = addresses.find_gap(window, 0x1_0000, ALLOCATION_ALIGN, &[(PLACED_AT, 0x402_0000)]);
        assert_eq!(gap, Some(0x402_0000));
    }

    #[test]
    fn a_window_past_every_range_is_out_of_reach() {
        let addresses = GpuAddresses::new();
        let far = Window { from: 1 << 40, to: 2 << 40 };
        assert!(!addresses.reaches(far));
        assert_eq!(addresses.find_gap(far, 0x1000, ALLOCATION_ALIGN, &[]), None);
        assert!(addresses.reaches(Window::ANYWHERE));
    }

    const ABOVE: GpuRange = GpuRange { start: GPU_RANGES[1].end, end: 1 << 47 };

    #[test]
    fn with_no_guest_only_ranges_the_guest_maps_only_in_the_ranges() {
        let addresses = GpuAddresses::new();
        assert!(addresses.guest_may_map(GPU_RANGES[1].start, 0x1000));
        assert!(!addresses.guest_may_map(0x255_f13c_0000, 0x1000));
    }

    #[test]
    fn a_guest_only_range_takes_the_guests_own_addresses_but_none_are_handed_out_there() {
        let mut addresses = GpuAddresses::new().with_guest_only(std::slice::from_ref(&ABOVE));
        assert!(addresses.guest_may_map(0x255_f13c_0000, 0x1000));
        assert!(!addresses.guest_may_map(ABOVE.end - 0x1000, 0x2000), "it ends past the range");
        assert!(!addresses.guest_may_map(ABOVE.end, 0x1000));
        assert!(!addresses.reaches(Window { from: ABOVE.start, to: ABOVE.end }));
        assert!(addresses.take(GPU_RANGES[1].end, ALLOCATION_ALIGN, HOST).is_err());
    }

    #[test]
    fn running_out_is_an_error() {
        assert!(GpuAddresses::new().take(1 << 50, ALLOCATION_ALIGN, HOST).is_err());
        assert!(GpuAddresses::new().take(u64::MAX, ALLOCATION_ALIGN, HOST).is_err());
    }

    #[test]
    fn a_freed_range_comes_back_for_the_same_use_only() {
        let mut addresses = GpuAddresses::new();
        let usage = AddressUse::Mapped;
        let first = addresses.take(0x10000, ALLOCATION_ALIGN, usage).unwrap();
        addresses.give_back(first, 0x10000, ALLOCATION_ALIGN, usage);
        assert_ne!(addresses.take(0x10000, ALLOCATION_ALIGN, AddressUse::Reserved), Ok(first));
        assert_ne!(addresses.take(0x20000, ALLOCATION_ALIGN, usage), Ok(first));
        assert_eq!(addresses.take(0x10000, ALLOCATION_ALIGN, usage), Ok(first));
    }

    #[test]
    fn a_range_that_held_system_memory_goes_back_to_system_memory() {
        let mut addresses = GpuAddresses::new();
        let (video, system) = (AddressUse::mapped(false), AddressUse::mapped(true));
        let first = addresses.take(0x10000, ALLOCATION_ALIGN, video).unwrap();
        addresses.give_back(first, 0x10000, ALLOCATION_ALIGN, system);
        assert_ne!(addresses.take(0x10000, ALLOCATION_ALIGN, video), Ok(first));
        assert_eq!(addresses.take(0x10000, ALLOCATION_ALIGN, system), Ok(first));
    }

    #[test]
    fn a_range_that_held_video_memory_goes_back_to_video_memory() {
        let mut addresses = GpuAddresses::new();
        let (video, system) = (AddressUse::mapped(false), AddressUse::mapped(true));
        let first = addresses.take(0x10000, ALLOCATION_ALIGN, system).unwrap();
        addresses.give_back(first, 0x10000, ALLOCATION_ALIGN, video);
        assert_ne!(addresses.take(0x10000, ALLOCATION_ALIGN, system), Ok(first));
        assert_eq!(addresses.take(0x10000, ALLOCATION_ALIGN, video), Ok(first));
    }

    #[test]
    fn with_no_new_addresses_left_a_range_of_the_other_memory_is_used() {
        let last = GPU_RANGES[1].end - 0x10000;
        let mut addresses = GpuAddresses::keeping_clear(&[GpuRange { start: 0, end: last }]);
        let (video, system) = (AddressUse::mapped(false), AddressUse::mapped(true));
        let only = addresses.take(0x10000, ALLOCATION_ALIGN, system).unwrap();
        addresses.give_back(only, 0x10000, ALLOCATION_ALIGN, system);
        assert_eq!(addresses.take(0x10000, ALLOCATION_ALIGN, video), Ok(only));
        assert!(addresses.take(0x10000, ALLOCATION_ALIGN, video).is_err());
    }

    #[test]
    fn the_last_range_given_back_goes_out_first() {
        let mut addresses = GpuAddresses::new();
        let usage = AddressUse::Reserved;
        addresses.give_back(0x1000, 1, 1, usage);
        addresses.give_back(0x2000, 1, 1, usage);
        assert_eq!(addresses.take(1, 1, usage), Ok(0x2000));
    }

    const AN_HOUR: Duration = Duration::from_secs(3600);

    #[test]
    fn without_a_hold_a_freed_range_comes_back_at_once() {
        for hold in [None, Some(Duration::ZERO)] {
            let mut addresses = GpuAddresses::new().holding_freed(hold);
            let usage = AddressUse::Mapped;
            let first = addresses.take(0x10000, ALLOCATION_ALIGN, usage).unwrap();
            addresses.give_back(first, 0x10000, ALLOCATION_ALIGN, usage);
            assert_eq!(addresses.take(0x10000, ALLOCATION_ALIGN, usage), Ok(first), "{hold:?}");
            assert!(addresses.freed_at.is_empty(), "nothing is timed without a hold");
        }
    }

    #[test]
    fn a_held_range_rests_while_new_addresses_are_handed_out() {
        let mut addresses = GpuAddresses::new().holding_freed(Some(AN_HOUR));
        let usage = AddressUse::Mapped;
        let first = addresses.take(0x10000, ALLOCATION_ALIGN, usage).unwrap();
        addresses.give_back(first, 0x10000, ALLOCATION_ALIGN, usage);
        let second = addresses.take(0x10000, ALLOCATION_ALIGN, usage).unwrap();
        assert_ne!(second, first);
        assert_eq!(second, first + 0x10000, "the next new address");
    }

    #[test]
    fn a_held_range_comes_back_once_it_has_rested() {
        let hold = Duration::from_millis(20);
        let mut addresses = GpuAddresses::new().holding_freed(Some(hold));
        let usage = AddressUse::Mapped;
        let first = addresses.take(0x10000, ALLOCATION_ALIGN, usage).unwrap();
        addresses.give_back(first, 0x10000, ALLOCATION_ALIGN, usage);
        std::thread::sleep(hold + Duration::from_millis(10));
        assert_eq!(addresses.take(0x10000, ALLOCATION_ALIGN, usage), Ok(first));
        assert!(addresses.freed_at.is_empty(), "its time went with it");
    }

    #[test]
    fn with_no_new_addresses_left_a_resting_range_is_handed_out_anyway() {
        let last = GPU_RANGES[1].end - 0x10000;
        let mut addresses =
            GpuAddresses::keeping_clear(&[GpuRange { start: 0, end: last }]).holding_freed(Some(AN_HOUR));
        let usage = AddressUse::Mapped;
        let only = addresses.take(0x10000, ALLOCATION_ALIGN, usage).unwrap();
        addresses.give_back(only, 0x10000, ALLOCATION_ALIGN, usage);
        assert_eq!(addresses.take(0x10000, ALLOCATION_ALIGN, usage), Ok(only));
        assert!(addresses.take(0x10000, ALLOCATION_ALIGN, usage).is_err());
    }

    #[test]
    fn a_rested_range_of_the_other_memory_goes_before_a_resting_one_of_the_same() {
        let last = GPU_RANGES[1].end - 0x20000;
        let mut addresses =
            GpuAddresses::keeping_clear(&[GpuRange { start: 0, end: last }]).holding_freed(Some(AN_HOUR));
        let (video, system) = (AddressUse::mapped(false), AddressUse::mapped(true));
        let a = addresses.take(0x10000, ALLOCATION_ALIGN, system).unwrap();
        let b = addresses.take(0x10000, ALLOCATION_ALIGN, video).unwrap();
        addresses.give_back(a, 0x10000, ALLOCATION_ALIGN, system);
        addresses.give_back(b, 0x10000, ALLOCATION_ALIGN, video);
        // Both rest, so the same memory's range goes out anyway, ahead of the other memory's.
        assert_eq!(addresses.take(0x10000, ALLOCATION_ALIGN, video), Ok(b));
        assert_eq!(addresses.take(0x10000, ALLOCATION_ALIGN, video), Ok(a));
    }
}
