//! Copies by the CPU in steps. The CPU reads the card's memory slowly, so a large copy, an 8K frame
//! or a big texture, goes a few megabytes at a time: one step when it starts, and one each time the
//! copy is looked at after that, so no step holds the daemon's thread up for long and the copy is
//! whole at its end.

use super::CardWork;
use super::copies::{Area, Rows};
use guesthouse_backend::{BackendError, SpaceId};

/// The most bytes the CPU copies or turns in one step.
pub const STEP_BYTES: u64 = 4 << 20;

/// What is left of a copy by the CPU: rows to copy, then rows whose red and blue change over.
#[derive(Debug)]
pub(super) struct CpuCopy {
    space: SpaceId,
    rows: Option<Rows>,
    turn: Option<Area>,
}

/// How many rows of `row_bytes` fit in what is left of a step: at least one.
fn rows_per_step(row_bytes: u64, budget: u64) -> u64 {
    (budget / row_bytes.max(1)).max(1)
}

fn too_long() -> BackendError {
    BackendError::new("a copy's rows run past the end of the addresses")
}

impl CardWork {
    /// Starts a copy by the CPU of `rows`, then a change of colours in `turn`, and takes its first
    /// step. `None` while there is more to do, which [`CardWork::step_cpu_copy`] does.
    pub fn start_cpu_copy(
        &mut self,
        space: SpaceId,
        rows: Option<Rows>,
        turn: Option<Area>,
    ) -> Option<Result<(), BackendError>> {
        self.blits.cpu = Some(CpuCopy { space, rows, turn });
        self.step_cpu_copy()
    }

    /// Whether a copy by the CPU is under way.
    pub fn cpu_copy_running(&self) -> bool {
        self.blits.cpu.is_some()
    }

    /// Takes the next step of the copy by the CPU under way. `None` while there is more to do,
    /// then how it went.
    pub fn step_cpu_copy(&mut self) -> Option<Result<(), BackendError>> {
        let mut job = self.blits.cpu.take()?;
        let mut budget = STEP_BYTES;
        while budget > 0 {
            let stepped = match (job.rows.as_mut(), job.turn.as_mut()) {
                (Some(rows), _) => self.copy_step(job.space, rows, &mut budget),
                (None, Some(area)) => self.turn_step(job.space, area, &mut budget),
                (None, None) => return Some(Ok(())),
            };
            match stepped {
                Err(error) => return Some(Err(error)),
                Ok(true) if job.rows.is_some() => job.rows = None,
                Ok(true) => job.turn = None,
                Ok(false) => {}
            }
        }
        if job.rows.is_none() && job.turn.is_none() {
            return Some(Ok(()));
        }
        self.blits.cpu = Some(job);
        None
    }

    /// Copies the next rows that fit in `budget`, takes their bytes from it, and moves `rows` past
    /// them. True once none are left.
    fn copy_step(&mut self, space: SpaceId, rows: &mut Rows, budget: &mut u64) -> Result<bool, BackendError> {
        let count = rows_per_step(rows.row_bytes, *budget).min(rows.rows);
        self.copy_rows(space, &Rows { rows: count, ..*rows })?;
        *budget = budget.saturating_sub(count.saturating_mul(rows.row_bytes).max(1));
        rows.source = count
            .checked_mul(rows.source_pitch)
            .and_then(|on| rows.source.checked_add(on))
            .ok_or_else(too_long)?;
        rows.target = count
            .checked_mul(rows.target_pitch)
            .and_then(|on| rows.target.checked_add(on))
            .ok_or_else(too_long)?;
        rows.rows -= count;
        Ok(rows.rows == 0)
    }

    /// Turns the colours of the next rows that fit in `budget`, takes their bytes from it, and
    /// moves `area` past them. True once none are left.
    fn turn_step(&mut self, space: SpaceId, area: &mut Area, budget: &mut u64) -> Result<bool, BackendError> {
        let count = rows_per_step(area.row_bytes, *budget).min(area.rows);
        self.turn_colours(space, &Area { rows: count, ..*area })?;
        *budget = budget.saturating_sub(count.saturating_mul(area.row_bytes).max(1));
        area.address =
            count.checked_mul(area.pitch).and_then(|on| area.address.checked_add(on)).ok_or_else(too_long)?;
        area.rows -= count;
        Ok(area.rows == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::Stage;
    use crate::work::testing::{self, map_request};
    use crate::xe::plan::BufferUse;
    use guesthouse_backend::GPU_RANGES;

    const SOURCE_AT: u64 = 0x1000_0000;
    const TARGET_AT: u64 = 0x5000_0000;

    #[test]
    fn a_copy_past_a_frames_worth_goes_in_steps_and_arrives_whole() {
        const ROW: u64 = 4096;
        const SIZE: u64 = 72 << 20;
        let (mut work, _xe) = testing::work(Stage::Everything);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let (from, _) = work.allocate(SIZE, BufferUse::Card).unwrap();
        let (to, _) = work.allocate(SIZE, BufferUse::Card).unwrap();
        work.map(&map_request(space, from, SOURCE_AT, 0, SIZE)).unwrap();
        work.map(&map_request(space, to, TARGET_AT, 0, SIZE)).unwrap();
        let last_row = (SIZE / ROW - 1) * ROW;
        assert!(work.cpu_view(from).unwrap().write(last_row, &[7; 4]));
        let rows = Rows {
            source: SOURCE_AT,
            target: TARGET_AT,
            row_bytes: ROW,
            rows: SIZE / ROW,
            source_pitch: ROW,
            target_pitch: ROW,
        };
        let area = Area { address: TARGET_AT, row_bytes: ROW, rows: SIZE / ROW, pitch: ROW };
        let mut steps = 1;
        let mut ended = work.start_cpu_copy(space, Some(rows), Some(area));
        while ended.is_none() {
            assert!(work.cpu_copy_running());
            ended = work.step_cpu_copy();
            steps += 1;
        }
        ended.unwrap().expect("the whole copy");
        assert_eq!(steps, (2 * SIZE).div_ceil(STEP_BYTES), "each step does at most its share");
        let mut read = [0u8; 4];
        assert!(work.cpu_view(to).unwrap().read(last_row, &mut read));
        assert_eq!(read, [7, 7, 7, 7], "its last row arrived, turned the same both ways");
        assert!(!work.cpu_copy_running());
    }

    #[test]
    fn a_row_wider_than_a_step_still_goes_a_row_at_a_time() {
        assert_eq!(rows_per_step(STEP_BYTES * 2, STEP_BYTES), 1);
        assert_eq!(rows_per_step(4096, 4096 * 3), 3);
    }
}
