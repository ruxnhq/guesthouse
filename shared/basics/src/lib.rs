//! Small pieces every host shares that belong to no one subject, each too small for a crate of its
//! own. Each is a value in and a value out, or a plain file, with no calls into a system or a
//! hypervisor, so it builds and is tested on every host.
//!
//! A piece moves here from a host as it was, and keeps working at its old path there.

pub mod capped_line;
pub mod clock;
pub mod json_file;
pub mod json_lines;
pub mod random;
pub mod text;
pub mod words;
