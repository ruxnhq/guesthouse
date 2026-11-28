//! Bug reports, the part with no host in it, so every host builds on one implementation: what the
//! report server and the hosts agree on (`contract`), the kinds of problem (`kind`), the
//! facts that sort a report (`summary`), and how it's posted with `curl` and the server's answer
//! read (`send`, `answer`), times
//! written in UTC (`clock`) a long file cut to its end (`trim`), the
//! note that says what a bundle holds (`manifest`), which host made it (`host`), the link that opens an issue about a sent
//! report (`issue`), the Windows host's scrubber (`scrub`), which a host can add its own pass to, and
//! what to say before a thin report is sent (`warning`).
//!
//! Each host gathers its own files and facts, packs them and runs `curl` its own way. Where the
//! hosts still differ in what they do with a report, each host's way is kept as it is, and the
//! difference is said where it lives.
//!
//! Nothing here calls the system, the network or a hypervisor, so every host can build on it.

pub mod answer;
pub mod clock;
pub mod contract;
pub mod host;
pub mod issue;
pub mod kind;
pub mod manifest;
pub mod scrub;
pub mod send;
pub mod summary;
pub mod trim;
pub mod warning;
