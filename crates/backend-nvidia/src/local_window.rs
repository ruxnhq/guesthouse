//! The window a shader's local addresses sit in, as the guest's own driver sets it.
//!
//! A compute launch that uses local memory needs a pool on its channel, and the pool is reached
//! through this window. Some queues set both themselves. Others, such as an OpenCL queue, set
//! neither and fail their first launch that needs local memory. The window is the same for every
//! queue of a guest, so the first one any queue sets is kept, and the pools given here use it.

use crate::pushbuffer::Header;
use guesthouse_backend::SpaceId;
use std::collections::HashMap;

/// The compute class is the second object on a graphics channel, so it is subchannel 1.
pub(crate) const COMPUTE_SUBCHANNEL: u32 = 1;
/// The compute class's methods for the pool and its window, from the open class headers.
pub(crate) const SET_LOCAL_MEMORY_ADDRESS: u32 = 0x790;
pub(crate) const SET_LOCAL_MEMORY_WINDOW: u32 = 0x7B0;
const SET_LOCAL_MEMORY_WINDOW_LOWER: u32 = SET_LOCAL_MEMORY_WINDOW + 4;
const SET_LOCAL_MEMORY_ADDRESS_LOWER: u32 = SET_LOCAL_MEMORY_ADDRESS + 4;
/// The pool's size for each unit.
pub(crate) const SET_LOCAL_MEMORY_SIZE: u32 = 0x2E4;
const SET_LOCAL_MEMORY_SIZE_LOWER: u32 = SET_LOCAL_MEMORY_SIZE + 4;
/// How many units the pool is split between, at most.
pub(crate) const SET_LOCAL_MEMORY_UNITS: u32 = 0x2EC;

const MODE_INCREASING: u32 = 1;
const MODE_NON_INCREASING: u32 = 3;
const MODE_IMMEDIATE: u32 = 4;
const MODE_INCREASE_ONCE: u32 = 5;
const METHOD_STEP: u32 = 4;

/// The window, once a queue has set it, and what each queue has said so far.
#[derive(Debug, Default)]
pub(crate) struct LocalWindow {
    window: Option<u64>,
    queues: HashMap<u32, LocalMemorySeen>,
}

impl LocalWindow {
    pub(crate) fn window(&self) -> Option<u64> {
        self.window
    }

    /// Reads one buffer of a queue's commands. Once the window is known, nothing more is read.
    pub(crate) fn read(&mut self, queue: u32, words: &[u32]) {
        if self.window.is_some() {
            return;
        }
        let seen = self.queues.entry(queue).or_default();
        seen.read(words);
        if let Some(window) = seen.window {
            log::info!("queue {queue:#x} set the shader local window to {window:#x}");
            self.window = Some(window);
            // Only the queues with their own pool are of any use from here.
            self.queues.retain(|_, seen| seen.sets_own_pool);
        }
    }

    /// Whether the queue was seen pointing its channel at its own pool before the window was known.
    pub(crate) fn sets_own_pool(&self, queue: u32) -> bool {
        self.queues.get(&queue).is_some_and(|seen| seen.sets_own_pool)
    }

    pub(crate) fn forget(&mut self, queue: u32) {
        self.queues.remove(&queue);
    }
}

/// What one queue's commands said about local memory.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalMemorySeen {
    /// The window, once both of its halves have been seen.
    pub(crate) window: Option<u64>,
    /// Whether the queue points its channel at a pool of its own.
    pub(crate) sets_own_pool: bool,
    upper: Option<u32>,
    lower: Option<u32>,
    address: [Option<u32>; 2],
    per_unit: [Option<u32>; 2],
    units: Option<u32>,
    /// A pool method came in the last read.
    touched: bool,
}

/// A shader local-memory pool, as the methods name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GuestPool {
    pub(crate) address: u64,
    pub(crate) per_unit: u64,
    pub(crate) units: u32,
    pub(crate) window: u64,
}

impl LocalMemorySeen {
    /// Reads one buffer of commands. A window split across buffers of the same queue still counts.
    pub(crate) fn read(&mut self, words: &[u32]) {
        self.touched = false;
        let mut at = 0;
        while at < words.len() {
            let header = Header::parse(words[at]);
            if header.subchannel == COMPUTE_SUBCHANNEL {
                for (method, value) in method_writes(header, &words[at + 1..]) {
                    self.note(method, value);
                }
            }
            at += header.stride();
        }
    }

    fn note(&mut self, method: u32, value: u32) {
        match method {
            SET_LOCAL_MEMORY_ADDRESS => self.sets_own_pool = true,
            SET_LOCAL_MEMORY_WINDOW => self.upper = Some(value),
            SET_LOCAL_MEMORY_WINDOW_LOWER => self.lower = Some(value),
            _ => {}
        }
        match method {
            SET_LOCAL_MEMORY_ADDRESS => self.address[0] = Some(value),
            SET_LOCAL_MEMORY_ADDRESS_LOWER => self.address[1] = Some(value),
            SET_LOCAL_MEMORY_SIZE => self.per_unit[0] = Some(value),
            SET_LOCAL_MEMORY_SIZE_LOWER => self.per_unit[1] = Some(value),
            SET_LOCAL_MEMORY_UNITS => self.units = Some(value),
            SET_LOCAL_MEMORY_WINDOW | SET_LOCAL_MEMORY_WINDOW_LOWER => {}
            _ => return,
        }
        self.touched = true;
        if let (Some(upper), Some(lower)) = (self.upper, self.lower) {
            let window = u64::from(upper) << 32 | u64::from(lower);
            self.window = (window != 0).then_some(window);
        }
    }
}

impl LocalMemorySeen {
    /// The whole pool, once every part of it has come and none is zero. A zero is a driver
    /// clearing it, which isn't a pool to pass on.
    pub(crate) fn pool(&self) -> Option<GuestPool> {
        let long = |[upper, lower]: [Option<u32>; 2]| Some(u64::from(upper?) << 32 | u64::from(lower?));
        let pool = GuestPool {
            address: long(self.address)?,
            per_unit: long(self.per_unit)?,
            units: self.units?,
            window: self.window?,
        };
        (pool.address != 0 && pool.per_unit != 0 && pool.units != 0).then_some(pool)
    }
}

/// The pools the guest's compute drivers set themselves, passed on to the context's other graphics
/// channels. On Windows a context's channels share one graphics context, so the driver sets its
/// pool on one channel and counts on the rest seeing it. Here each channel is its own, and one
/// left on our small pool runs the guest's bigger per-unit size past its end.
#[derive(Debug, Default)]
pub(crate) struct PoolMirror {
    seen: HashMap<u32, LocalMemorySeen>,
    /// The newest pool for each address space and kind of context, and how many there have been.
    pools: HashMap<(SpaceId, u32), (GuestPool, u32)>,
    /// Which of those each queue runs with.
    given: HashMap<u32, u32>,
}

impl PoolMirror {
    /// Reads a submission's commands, which come before it runs. `key` is its address space and
    /// what its context is for.
    pub(crate) fn read(&mut self, key: (SpaceId, u32), queue: u32, words: &[u32]) {
        let seen = self.seen.entry(queue).or_default();
        seen.read(words);
        if !seen.touched {
            return;
        }
        let Some(pool) = seen.pool() else { return };
        let generation = match self.pools.get(&key) {
            Some((known, generation)) if *known == pool => *generation,
            known => {
                let generation = known.map_or(1, |(_, generation)| generation + 1);
                log::info!(
                    "queue {queue:#x} set its own shader local pool {pool:x?}, which goes to the rest"
                );
                self.pools.insert(key, (pool, generation));
                generation
            }
        };
        self.given.insert(queue, generation);
    }

    /// The pool this queue has to be pointed at ahead of its next submission, if it isn't yet.
    pub(crate) fn owed(&mut self, key: (SpaceId, u32), queue: u32) -> Option<GuestPool> {
        let (pool, generation) = *self.pools.get(&key)?;
        let given = self.given.entry(queue).or_default();
        (*given < generation).then(|| {
            *given = generation;
            pool
        })
    }

    pub(crate) fn forget(&mut self, queue: u32) {
        self.seen.remove(&queue);
        self.given.remove(&queue);
    }

    /// An address space has gone.
    pub(crate) fn forget_space(&mut self, space: SpaceId) {
        self.pools.retain(|(known, _), _| *known != space);
    }
}

/// Each method a header writes, with its value, as far as the words after it reach.
fn method_writes(header: Header, data: &[u32]) -> Vec<(u32, u32)> {
    if header.mode == MODE_IMMEDIATE {
        return vec![(header.method, header.count)];
    }
    let method_of = |step: u32| match header.mode {
        MODE_INCREASING => Some(header.method + step * METHOD_STEP),
        MODE_INCREASE_ONCE => Some(header.method + step.min(1) * METHOD_STEP),
        MODE_NON_INCREASING => Some(header.method),
        _ => None,
    };
    let count = (header.count as usize).min(data.len());
    data[..count].iter().zip(0..).filter_map(|(&value, step)| Some((method_of(step)?, value))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pushbuffer::increasing;

    const WINDOW: u64 = 0x1234_5600_0000;

    fn header(mode: u32, subchannel: u32, method: u32, count: u32) -> u32 {
        mode << 29 | count << 16 | subchannel << 13 | method >> 2
    }

    fn seen(words: &[u32]) -> LocalMemorySeen {
        let mut seen = LocalMemorySeen::default();
        seen.read(words);
        seen
    }

    #[test]
    fn a_window_set_in_one_header_is_read() {
        let words = [increasing(1, SET_LOCAL_MEMORY_WINDOW, 2), (WINDOW >> 32) as u32, WINDOW as u32];
        assert_eq!(seen(&words).window, Some(WINDOW));
    }

    #[test]
    fn a_window_set_a_half_at_a_time_is_read() {
        let words = [
            increasing(1, 0x2E4, 1),
            7,
            increasing(1, SET_LOCAL_MEMORY_WINDOW, 1),
            (WINDOW >> 32) as u32,
            header(MODE_IMMEDIATE, 1, 0x0, 3),
            increasing(1, SET_LOCAL_MEMORY_WINDOW_LOWER, 1),
            WINDOW as u32,
        ];
        assert_eq!(seen(&words).window, Some(WINDOW));
    }

    #[test]
    fn halves_in_two_buffers_make_one_window() {
        let mut seen = LocalMemorySeen::default();
        seen.read(&[increasing(1, SET_LOCAL_MEMORY_WINDOW, 1), (WINDOW >> 32) as u32]);
        assert_eq!(seen.window, None);
        seen.read(&[increasing(1, SET_LOCAL_MEMORY_WINDOW_LOWER, 1), WINDOW as u32]);
        assert_eq!(seen.window, Some(WINDOW));
    }

    #[test]
    fn an_increase_once_header_reaches_the_lower_half() {
        let words =
            [header(MODE_INCREASE_ONCE, 1, SET_LOCAL_MEMORY_WINDOW, 2), (WINDOW >> 32) as u32, WINDOW as u32];
        assert_eq!(seen(&words).window, Some(WINDOW));
    }

    #[test]
    fn the_same_method_on_another_subchannel_is_not_the_window() {
        let words = [increasing(0, SET_LOCAL_MEMORY_WINDOW, 2), (WINDOW >> 32) as u32, WINDOW as u32];
        assert_eq!(seen(&words), LocalMemorySeen::default());
    }

    #[test]
    fn a_window_of_nothing_is_no_window() {
        assert_eq!(seen(&[increasing(1, SET_LOCAL_MEMORY_WINDOW, 2), 0, 0]).window, None);
    }

    #[test]
    fn a_header_cut_short_reads_only_what_is_there() {
        assert_eq!(seen(&[increasing(1, SET_LOCAL_MEMORY_WINDOW, 2), 0x1234]).window, None);
    }

    #[test]
    fn the_first_window_set_is_kept_and_nothing_is_read_after() {
        let mut local = LocalWindow::default();
        local.read(3, &[increasing(1, SET_LOCAL_MEMORY_ADDRESS, 2), 1, 0]);
        local.read(4, &[increasing(1, 0x2E4, 1), 1]);
        assert_eq!(local.window(), None);
        local.read(5, &[increasing(1, SET_LOCAL_MEMORY_WINDOW, 2), (WINDOW >> 32) as u32, WINDOW as u32]);
        assert_eq!(local.window(), Some(WINDOW));
        local.read(6, &[increasing(1, SET_LOCAL_MEMORY_WINDOW, 2), 1, 0]);
        local.read(6, &[increasing(1, SET_LOCAL_MEMORY_ADDRESS, 2), 1, 0]);
        assert_eq!(local.window(), Some(WINDOW));
        assert!(local.sets_own_pool(3));
        assert!(!local.sets_own_pool(4) && !local.sets_own_pool(6));
        local.forget(3);
        assert!(!local.sets_own_pool(3));
    }

    #[test]
    fn a_queue_that_names_its_own_pool_is_noticed() {
        let words = [increasing(1, SET_LOCAL_MEMORY_ADDRESS, 2), 1, 0];
        let seen = seen(&words);
        assert!(seen.sets_own_pool);
        assert_eq!(seen.window, None);
    }

    const SPACE: SpaceId = SpaceId(1);
    const CUDA: (SpaceId, u32) = (SPACE, 5);

    fn pool_words(address: u64, per_unit: u64, units: u32) -> Vec<u32> {
        vec![
            increasing(1, SET_LOCAL_MEMORY_ADDRESS, 2),
            (address >> 32) as u32,
            address as u32,
            increasing(1, SET_LOCAL_MEMORY_SIZE, 2),
            (per_unit >> 32) as u32,
            per_unit as u32,
            increasing(1, SET_LOCAL_MEMORY_UNITS, 1),
            units,
            increasing(1, SET_LOCAL_MEMORY_WINDOW, 2),
            (WINDOW >> 32) as u32,
            WINDOW as u32,
        ]
    }

    #[test]
    fn a_whole_pool_is_read() {
        let pool = seen(&pool_words(0x5_0580_0000, 0x3F_0000, 0x55)).pool();
        assert_eq!(
            pool,
            Some(GuestPool { address: 0x5_0580_0000, per_unit: 0x3F_0000, units: 0x55, window: WINDOW })
        );
    }

    #[test]
    fn a_cleared_pool_is_no_pool() {
        assert_eq!(seen(&pool_words(0, 0, 0x100)).pool(), None);
    }

    #[test]
    fn a_pool_goes_to_the_other_queues_once_and_not_back_to_its_own() {
        let mut mirror = PoolMirror::default();
        assert_eq!(mirror.owed(CUDA, 2), None);
        mirror.read(CUDA, 1, &pool_words(0x5_0580_0000, 0x3F_0000, 0x55));
        assert_eq!(mirror.owed(CUDA, 1), None);
        assert_eq!(mirror.owed(CUDA, 2).map(|pool| pool.address), Some(0x5_0580_0000));
        assert_eq!(mirror.owed(CUDA, 2), None);
        assert_eq!(mirror.owed((SPACE, 3), 3), None, "another kind of context keeps its own");
        assert_eq!(mirror.owed((SpaceId(2), 5), 3), None, "another process keeps its own");
    }

    #[test]
    fn a_pool_that_grows_goes_out_again() {
        let mut mirror = PoolMirror::default();
        mirror.read(CUDA, 1, &pool_words(0x5_0580_0000, 0x3F_0000, 0x55));
        mirror.owed(CUDA, 2);
        mirror.read(CUDA, 1, &[0]);
        assert_eq!(mirror.owed(CUDA, 2), None, "commands without pool methods change nothing");
        mirror.read(CUDA, 1, &pool_words(0x5_1AA0_0000, 0xD0_0000, 0x55));
        assert_eq!(mirror.owed(CUDA, 2).map(|pool| pool.per_unit), Some(0xD0_0000));
        mirror.forget_space(SPACE);
        mirror.forget(2);
        assert_eq!(mirror.owed(CUDA, 2), None);
    }
}
