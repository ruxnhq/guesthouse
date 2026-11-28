//! The ports other machines hold as the service collects them today, held before it moves: every
//! other machine's port is taken, and a machine's own isn't.

use super::remote_view::{info, share, windows_machine};
use super::*;

#[test]
fn every_other_machines_port_is_taken_and_its_own_is_not() {
    let setup = setup("view-ports", Pretend::default());
    let service = &setup.service;
    for name in ["one", "two", "three"] {
        windows_machine(&setup, name);
    }
    assert_eq!(info(share(service, "one", Some(true))).port, Some(7792));
    assert_eq!(info(share(service, "two", Some(true))).port, Some(7793));
    assert_eq!(info(share(service, "one", Some(false))).port, Some(7792));
    assert_eq!(
        info(share(service, "three", Some(true))).port,
        Some(7794),
        "one's port stays its own while off"
    );
    assert_eq!(info(share(service, "one", Some(true))).port, Some(7792));
}
