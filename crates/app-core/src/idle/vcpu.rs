//! How much the guest's vCPUs ran, from the host. The kernel counts the time a process spent running a
//! guest in `guest_time`, in the process's `stat`. That is the vCPU threads' time and not QEMU's own
//! (its main loop, the screen, the disks), which is why it is used and not the process's CPU time:
//! QEMU is started with `-name gpupv` and no thread names, so the vCPU threads can't be told by name
//! in `/proc/PID/task`. It is a count, so a rate needs two looks.

use super::{Signal, Unread};
use crate::context::Context;
use crate::process_stat::{self, Seen};
use guesthouse_jail::launch::start_time;
use std::time::Duration;

/// The field's place after the program's name: `stat`'s field 43, where the state, field 3, is the first.
const GUEST_TIME_FIELD: usize = 43 - 3;
/// What the kernel counts the time in. It is fixed for programs, whatever the kernel is built with.
const TICKS_PER_SECOND: f64 = 100.0;
/// A look that came sooner than this after the first says too little.
pub const LEAST_WINDOW: Duration = Duration::from_secs(2);

/// The guest time in ticks, if this kernel's `stat` has the field.
pub fn guest_ticks(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(GUEST_TIME_FIELD)?.parse().ok()
}

/// The first look at a VM's QEMU.
pub struct First {
    pid: u32,
    started: u64,
    ticks: u64,
    at: Duration,
}

/// Looks at the QEMU of a VM, or says why it can't.
pub fn first_look(context: &Context, pid: u32) -> Signal<First> {
    let (ticks, started) = look(context, pid)?;
    Ok(First { pid, started, ticks, at: context.clock.now() })
}

impl First {
    /// When the first look was, by the context's clock.
    pub fn at(&self) -> Duration {
        self.at
    }

    /// How many cores the vCPUs ran, on average, since the first look, and for how many seconds.
    pub fn since(&self, context: &Context) -> Signal<(f64, f64)> {
        let (ticks, started) = look(context, self.pid)?;
        if started != self.started {
            return Err(Unread::new("the VM's QEMU ended and the system gave its pid to another process"));
        }
        let seconds = context.clock.now().saturating_sub(self.at).as_secs_f64();
        if seconds <= 0.0 {
            return Err(Unread::new("no time passed between two looks at the VM"));
        }
        Ok((ticks.saturating_sub(self.ticks) as f64 / TICKS_PER_SECOND / seconds, seconds))
    }
}

/// The guest time, and the start time that says which process it is.
fn look(context: &Context, pid: u32) -> Signal<(u64, u64)> {
    let stat = process_stat::read(context, pid).filter(|_| Seen::now(context, pid).is_some());
    let Some(stat) = stat else { return Err(Unread::new("the VM's QEMU is not running")) };
    match (guest_ticks(&stat), start_time(&stat)) {
        (Some(ticks), Some(started)) => Ok((ticks, started)),
        _ => Err(Unread::new("this kernel does not count the time a process runs a guest")),
    }
}

#[cfg(test)]
pub mod stand_in {
    use crate::context::Context;
    use std::fs;

    /// A QEMU's `stat` with this much guest time, as a kernel writes all 52 fields, in a process that
    /// started at this tick. The name has a bracket in it, as a program's can.
    pub fn write_guest_stat(context: &Context, pid: u32, started: u64, guest_ticks: u64) {
        let folder = context.processes.join(pid.to_string());
        fs::create_dir_all(&folder).unwrap();
        let mut fields = vec!["0".to_owned(); 50];
        fields[0] = "S".to_owned();
        fields[19] = started.to_string();
        fields[GUEST_FIELD_INDEX] = guest_ticks.to_string();
        fs::write(folder.join("stat"), format!("{pid} (qemu (x)) {}", fields.join(" "))).unwrap();
    }

    const GUEST_FIELD_INDEX: usize = super::GUEST_TIME_FIELD;
}

#[cfg(test)]
mod tests {
    use super::stand_in::write_guest_stat;
    use super::*;
    use crate::clock::Clock;
    use crate::clock::stand_in::Ticking;
    use crate::testing::{Scratch, scratch_context};
    use std::sync::Arc;

    #[test]
    fn the_guest_time_is_field_43_after_the_name() {
        // A real kernel's line, with the program's name holding a bracket and a space.
        let stat = "4242 (qemu (x) y) S 1 4242 4242 0 -1 4194560 100 0 0 0 10 20 0 0 20 0 9 0 777 1000 500 \
                    18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 55 0 0 0 0 0 0 0 0 0";
        assert_eq!(guest_ticks(stat), Some(55));
        assert_eq!(start_time(stat), Some(777));
        assert_eq!(guest_ticks("4242 (qemu) S 1 2 3"), None, "an older kernel has no such field");
    }

    #[test]
    fn the_rate_is_the_cores_the_vcpus_ran_over_the_time_between_two_looks() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let clock = Arc::new(Ticking::default());
        context.clock = clock.clone();
        write_guest_stat(&context, 500, 7, 1000);
        let first = first_look(&context, 500).unwrap();
        clock.sleep(Duration::from_secs(10));
        // 200 ticks are two seconds of guest time, in ten.
        write_guest_stat(&context, 500, 7, 1000 + 200);
        let (cores, seconds) = first.since(&context).unwrap();
        assert!((cores - 0.2).abs() < 1e-9 && (seconds - 10.0).abs() < 1e-9, "{cores} over {seconds}");
        write_guest_stat(&context, 500, 7, 1000 + 2000);
        assert!((first.since(&context).unwrap().0 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_vm_that_is_gone_or_replaced_is_not_read() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let clock = Arc::new(Ticking::default());
        context.clock = clock.clone();
        assert!(first_look(&context, 500).is_err(), "nothing runs under this pid");
        write_guest_stat(&context, 500, 7, 1000);
        let first = first_look(&context, 500).unwrap();
        clock.sleep(Duration::from_secs(5));
        write_guest_stat(&context, 500, 8, 5);
        assert!(first.since(&context).is_err(), "another process has the pid");
        std::fs::remove_dir_all(context.processes.join("500")).unwrap();
        assert!(first.since(&context).is_err());
    }

    #[test]
    fn two_looks_with_no_time_between_say_nothing() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.clock = Arc::new(Ticking::default());
        write_guest_stat(&context, 500, 7, 1000);
        assert!(first_look(&context, 500).unwrap().since(&context).is_err());
    }
}
