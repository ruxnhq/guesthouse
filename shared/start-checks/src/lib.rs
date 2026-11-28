//! Why a machine can't start, judged from facts a host reads first: plain data in and sentences out,
//! in the order a host says them, the first being what a refused start says. Each host's checks are a
//! module of their own, as each host judges a start its own way. Only the Linux host judges a start
//! from facts today, [`linux_host`]; the Windows host refuses a start only for what its service is
//! doing, and checks no facts before it.
//!
//! Reading the facts stays with each host, as they come from the PC: its memory, cores and cards, the
//! files a machine names, and what else runs.

pub mod linux_host;
