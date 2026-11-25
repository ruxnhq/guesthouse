//! Fence pages: counters in shared memory that guest, host and GPU all read.

use guesthouse_backend::{BackendError, MemoryId, SpaceId};

/// Fence pages take the front of the guest's IO space, as one block.
pub const FENCE_AREA: u64 = 16 << 20;
/// Each fence gets a page of its own.
pub const FENCE_PAGE: u64 = 4096;

/// A fence's guest physical address and GPU address.
pub type Fence = (u64, u64);

/// The block of fence pages, at one GPU address in every process. A process could move another's
/// fence, which is the one thing that crosses between address spaces, but they hold only counters.
///
/// The GPU reaches the block in pieces, each adopted and mapped when a fence in it is first handed
/// out. Most backends take the whole area as the one piece. One whose driver works through every
/// adopted page each time work is sent asks for smaller ones, and then only the pieces in use cost.
#[derive(Debug)]
pub struct FenceBlock {
    pub gpu_address: u64,
    piece: u64,
    /// Each piece's memory once it is adopted, by its place in the area.
    pieces: Vec<Option<MemoryId>>,
    /// The address spaces every adopted piece is mapped in, oldest first.
    pub spaces: Vec<SpaceId>,
}

/// A piece of the block the GPU reaches: where it starts in the area, its length and its memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FencePiece {
    pub offset: u64,
    pub length: u64,
    pub memory: MemoryId,
}

/// The piece size to use when a backend asks for `asked`: that, if it cuts the area into whole
/// pages, and the whole area otherwise.
pub fn piece_size(asked: Option<u64>) -> u64 {
    asked
        .filter(|&piece| piece != 0 && piece.is_multiple_of(FENCE_PAGE) && FENCE_AREA.is_multiple_of(piece))
        .unwrap_or(FENCE_AREA)
}

impl FenceBlock {
    /// A block at `gpu_address` in pieces of `piece` bytes, none adopted yet.
    pub fn new(gpu_address: u64, piece: u64) -> Self {
        let pieces = vec![None; (FENCE_AREA / piece) as usize];
        Self { gpu_address, piece, pieces, spaces: Vec::new() }
    }

    /// Where the piece holding the fence at `offset` into the area starts, and its length, if the
    /// GPU doesn't reach it yet.
    pub fn missing_piece(&self, offset: u64) -> Option<(u64, u64)> {
        let index = (offset / self.piece) as usize;
        match self.pieces.get(index) {
            Some(None) => Some((index as u64 * self.piece, self.piece)),
            _ => None,
        }
    }

    /// Notes that the piece starting at `offset` is adopted as `memory`.
    pub fn adopted(&mut self, offset: u64, memory: MemoryId) {
        if let Some(piece) = self.pieces.get_mut((offset / self.piece) as usize) {
            *piece = Some(memory);
        }
    }

    /// The pieces the GPU reaches, in the order they lie in the area.
    pub fn reached(&self) -> Vec<FencePiece> {
        let adopted = |(index, memory): (usize, &Option<MemoryId>)| {
            memory.map(|memory| FencePiece { offset: index as u64 * self.piece, length: self.piece, memory })
        };
        self.pieces.iter().enumerate().filter_map(adopted).collect()
    }
}

/// Which pages are in use.
#[derive(Debug, Default)]
pub struct FencePool {
    next_page: u64,
    pub block: Option<FenceBlock>,
    free: Vec<Fence>,
    free_paging: Vec<u64>,
}

impl FencePool {
    pub fn new() -> Self {
        Self::default()
    }

    /// A page never used before, as an offset into the fence area.
    pub fn fresh_page(&mut self) -> Result<u64, BackendError> {
        if (self.next_page + 1) * FENCE_PAGE > FENCE_AREA {
            return Err(BackendError::new("out of fence pages"));
        }
        let offset = self.next_page * FENCE_PAGE;
        self.next_page += 1;
        Ok(offset)
    }

    /// Takes back the page [`Self::fresh_page`] gave last, which nothing came to use.
    pub fn unused_fresh_page(&mut self, offset: u64) {
        if self.next_page > 0 && offset == (self.next_page - 1) * FENCE_PAGE {
            self.next_page -= 1;
        }
    }

    pub fn reuse(&mut self) -> Option<Fence> {
        self.free.pop()
    }

    pub fn give_back(&mut self, fence: Fence) {
        self.free.push(fence);
    }

    pub fn reuse_paging(&mut self) -> Option<u64> {
        self.free_paging.pop()
    }

    pub fn give_back_paging(&mut self, fence: u64) {
        self.free_paging.push(fence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_recently_freed_fence_comes_back_first() {
        let mut pool = FencePool::new();
        pool.give_back((1, 10));
        pool.give_back((2, 20));
        assert_eq!(pool.reuse(), Some((2, 20)));
    }

    #[test]
    fn paging_fences_are_a_separate_pool() {
        let mut pool = FencePool::new();
        pool.give_back_paging(5);
        assert_eq!(pool.reuse(), None);
        assert_eq!(pool.reuse_paging(), Some(5));
    }

    #[test]
    fn a_fresh_page_nothing_used_is_given_again() {
        let mut pool = FencePool::new();
        let first = pool.fresh_page().unwrap();
        let second = pool.fresh_page().unwrap();
        pool.unused_fresh_page(first);
        assert_eq!(pool.fresh_page().unwrap(), second + FENCE_PAGE, "only the last one goes back");
        pool.unused_fresh_page(second + FENCE_PAGE);
        assert_eq!(pool.fresh_page().unwrap(), second + FENCE_PAGE);
    }

    #[test]
    fn a_piece_has_to_cut_the_area_into_whole_pages() {
        assert_eq!(piece_size(None), FENCE_AREA);
        assert_eq!(piece_size(Some(1 << 20)), 1 << 20);
        assert_eq!(piece_size(Some(FENCE_PAGE)), FENCE_PAGE);
        for bad in [0, FENCE_PAGE / 2, 3 * FENCE_PAGE, FENCE_AREA * 2, (1 << 20) + 1] {
            assert_eq!(piece_size(Some(bad)), FENCE_AREA, "{bad:#x}");
        }
    }

    #[test]
    fn a_block_is_reached_piece_by_piece() {
        const PIECE: u64 = 1 << 20;
        let mut block = FenceBlock::new(0x4000_0000, PIECE);
        assert!(block.reached().is_empty());
        assert_eq!(block.missing_piece(0), Some((0, PIECE)));
        assert_eq!(block.missing_piece(3 * PIECE + FENCE_PAGE), Some((3 * PIECE, PIECE)));

        block.adopted(3 * PIECE, MemoryId(9));
        block.adopted(0, MemoryId(4));
        assert_eq!(block.missing_piece(PIECE - FENCE_PAGE), None);
        assert_eq!(block.missing_piece(3 * PIECE), None);
        assert_eq!(block.missing_piece(PIECE), Some((PIECE, PIECE)));
        assert_eq!(block.missing_piece(FENCE_AREA), None, "past the area there is nothing to adopt");
        let reached = block.reached();
        assert_eq!(reached.len(), 2);
        assert_eq!(reached[0], FencePiece { offset: 0, length: PIECE, memory: MemoryId(4) });
        assert_eq!(reached[1], FencePiece { offset: 3 * PIECE, length: PIECE, memory: MemoryId(9) });
    }

    #[test]
    fn a_whole_block_is_one_piece() {
        let mut block = FenceBlock::new(0, FENCE_AREA);
        assert_eq!(block.missing_piece(FENCE_AREA - FENCE_PAGE), Some((0, FENCE_AREA)));
        block.adopted(0, MemoryId(1));
        assert_eq!(block.missing_piece(FENCE_AREA - FENCE_PAGE), None);
    }

    #[test]
    fn the_area_runs_out() {
        let mut pool = FencePool::new();
        for _ in 0..FENCE_AREA / FENCE_PAGE {
            pool.fresh_page().unwrap();
        }
        assert!(pool.fresh_page().is_err());
    }
}
