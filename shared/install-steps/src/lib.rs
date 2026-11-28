//! The plain half of a guest's install, as each host goes through it: the steps it says and their
//! words, how long each wait is and how often it looks, the rule that a disk which stops growing
//! means the install is stuck, and every sentence. Each host's are a module of their own, as each
//! host installs its guests its own way.
//!
//! Only the Linux host goes through an install in steps it says as it waits, [`linux_host`]. The
//! Windows host's service makes a machine in one go (DISM applies Windows to a new disk, or Ubuntu's
//! installer is readied on one) and deletes a machine it couldn't make, so it has nothing to wait on
//! or to go on from, and no module.
//!
//! Starting the VM, asking the guest, reading the serial log and the disk, and making the discs stay
//! with each host.

pub mod linux_host;
