//! Contexts the backend refuses.

use super::*;
use guesthouse_backend_standin::NODE_WITHOUT_CHANNEL;

#[test]
fn a_context_on_a_node_with_no_channel_is_refused_with_a_zero_handle() {
    let mut session = session();
    assert_eq!(create_context(&mut session, NODE_WITHOUT_CHANNEL), 0);
    assert!(session.context_nodes.is_empty());
}

#[test]
fn a_context_whose_private_data_is_refused_gets_a_zero_handle() {
    let mut session = session();
    session.backend_mut().refuse_private_data = true;
    assert_eq!(create_context(&mut session, 0), 0);
    assert!(session.context_nodes.is_empty());
}

#[test]
fn a_refusal_makes_no_handle() {
    let mut session = session();
    create_context(&mut session, NODE_WITHOUT_CHANNEL);
    let first = create_context(&mut session, 0);
    let mut fresh = self::session();
    assert_eq!(create_context(&mut fresh, 0), first);
}
