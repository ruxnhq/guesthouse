//! The building blocks of a guest's unattended install, as each host has them: the XML escape, the
//! Windows edition table and the reader of a WIM's editions, the time zone table, the fills that put
//! an account into an answer file, and the templates they fill.
//!
//! A wrong byte here makes Windows Setup or Ubuntu's installer fail an hour in, or puts a password
//! somewhere new, so nothing was unified when these moved: the hosts' answer files do different jobs
//! (Setup from a disc, an image applied offline), their escapes differ on the apostrophe, and each
//! host's pieces are a module of their own, [`linux_host`] and [`windows_host`], with none standing in
//! for another.
//!
//! Making a password, reading the host's time zone, reading a disc and writing the files stay with
//! each host, as they draw on the system.

pub mod linux_host;
pub mod windows_host;
