//! What another process may open: a resource shared by its global handle, or through a shared object.

use super::*;

/// A resource made to be shared by a global handle, and its one allocation.
pub(super) fn shared_by_global_handle(session: &mut TestSession) -> (u32, u32) {
    const CREATE_SHARED: u32 = 0x2;
    let reply = send(session, &create_message(CREATE_SHARED, &[private(0x1000)]));
    assert_eq!(status_of(&reply), SUCCESS);
    (u32_at(&reply, 4).unwrap(), u32_at(&reply, 0x10).unwrap())
}

fn open(session: &mut TestSession, shared: u32) -> Option<u32> {
    let mut request = message(Command::OpenResource, 0x28);
    set_u32(&mut request, 0x20, shared);
    set_u32(&mut request, 0x24, 1);
    let reply = send(session, &request);
    u32_at(&reply, 8).ok().filter(|&handle| handle != 0)
}

#[test]
fn a_resource_shared_by_its_global_handle_opens_by_it() {
    let mut session = session();
    let (resource, handle) = shared_by_global_handle(&mut session);
    assert_eq!(open(&mut session, resource), Some(handle));
}

#[test]
fn a_resource_not_made_to_be_shared_does_not_open_by_its_handle() {
    let mut session = session();
    let (resource, _) = allocate(&mut session, private(0x1000));
    assert_eq!(open(&mut session, resource), None);
    assert!(!session.resource_users.contains_key(&resource));
}

#[test]
fn a_resource_opens_through_a_shared_object() {
    let mut session = session();
    let (resource, handle) = allocate(&mut session, private(0x1000));
    let mut share = message(Command::CreateSharedObject, 0x20);
    set_u32(&mut share, 0x18, resource);
    let shared = u32_at(&send(&mut session, &share), 0).unwrap();
    assert_eq!(open(&mut session, shared), Some(handle));
}
