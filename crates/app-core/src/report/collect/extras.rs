//! The place for anything a report could hold only with the tester's say-so for that one thing, such
//! as a screenshot of the guest or a recording. There are none yet.
//!
//! An extra must be asked for by name, in its own words, every time, and be off unless the tester
//! turns it on. It goes in `Request` as a choice of its own and is added here, and the dialog and
//! `guesthouse-report` each need a way to ask. Memory dumps, core files and disk images are never
//! extras: they don't go in a report at all.

use super::{Collection, Request};
use crate::context::Context;

pub fn collect(_context: &Context, _request: &Request, _found: &mut Collection) {}
