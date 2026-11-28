// SPDX-License-Identifier: GPL-2.0-or-later
//! Shared by the stand-ins: they end when the program that started them does. A test that
//! fails early leaves its machines running, and a stand-in that waited for ever would keep
//! the pipes and sockets it inherited open, and with them whatever is waiting on those.

use std::process::exit;
use std::thread;
use std::time::Duration;

/// How often the stand-in looks for its parent.
const LOOK: Duration = Duration::from_millis(250);

/// Starts a thread that ends this process once its parent has gone.
pub fn end_with_parent() {
    // SAFETY: getppid has no arguments and no effects.
    let parent = unsafe { libc::getppid() };
    thread::spawn(move || {
        loop {
            thread::sleep(LOOK);
            // SAFETY: as above.
            if unsafe { libc::getppid() } != parent {
                exit(0);
            }
        }
    });
}
