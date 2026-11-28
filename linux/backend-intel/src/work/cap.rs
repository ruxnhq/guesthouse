//! How much video memory a guest may take, so the host's own desktop keeps room on the same card.

use crate::xe_query::MemoryRegion;

/// The least kept back for the host on a card with memory of its own.
pub const HOST_RESERVE: u64 = 1 << 30;
/// Or this part of the card, if that is more.
pub const HOST_RESERVE_PART: u64 = 4;

/// The least video memory a guest is worth starting work on the card for. With less left over, the
/// guest's first allocations would fail, so it is left listening instead.
pub const LEAST_FOR_A_GUEST: u64 = 512 << 20;

/// The most a guest may hold. On a card with memory of its own: what the card has, less what was
/// in use when it was opened, which is the host's desktop, less a reserve for the desktop to grow
/// into. On a card that only has the system's memory, `shared` is the share already worked out for
/// the guest.
pub fn video_cap(regions: &[MemoryRegion], shared: u64) -> u64 {
    let own: Vec<&MemoryRegion> =
        regions.iter().filter(|region| region.class == MemoryRegion::VRAM).collect();
    if own.is_empty() {
        return shared;
    }
    let total: u64 = own.iter().map(|region| region.total).sum();
    let used: u64 = own.iter().map(|region| region.used).sum();
    let reserve = HOST_RESERVE.max(total / HOST_RESERVE_PART);
    total.saturating_sub(used).saturating_sub(reserve)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card;

    const MIB: u64 = 1 << 20;

    fn vram(total: u64, used: u64) -> Vec<MemoryRegion> {
        let mut regions = card::tests::battlemage().regions;
        for region in regions.iter_mut().filter(|region| region.class == MemoryRegion::VRAM) {
            (region.total, region.used) = (total, used);
        }
        regions
    }

    #[test]
    fn a_card_with_its_own_memory_keeps_a_quarter_and_what_the_desktop_holds() {
        // The tester's B580 with only his desktop on it.
        assert_eq!(video_cap(&vram(12216 * MIB, 742 * MIB), 0), (12216 - 742 - 3054) * MIB);
    }

    #[test]
    fn a_small_card_keeps_a_whole_gibibyte() {
        assert_eq!(video_cap(&vram(2048 * MIB, 100 * MIB), 0), (2048 - 100 - 1024) * MIB);
    }

    #[test]
    fn a_busy_desktop_can_leave_a_guest_nothing() {
        assert_eq!(video_cap(&vram(4096 * MIB, 3500 * MIB), 0), 0);
    }

    #[test]
    fn a_card_on_the_systems_memory_gets_its_share() {
        assert_eq!(video_cap(&card::tests::lunar_lake().regions, 16 << 30), 16 << 30);
    }
}
