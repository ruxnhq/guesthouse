//! The real guest behind [`crate::ssh`]'s relay: a Hyper-V socket to the machine's own runtime ID.

use crate::events::Sink;
use crate::ssh::{Dial, GUEST_PORT, GuestSocket, Listener};
use guesthouse_hcs::hvsock::{HvStream, parse_guid};
use guesthouse_hcs::system::ComputeSystem;
use std::io;
use std::sync::Arc;
use std::time::Duration;

/// How long a connection to the guest may take before it's given up on.
const CONNECT: Duration = Duration::from_secs(5);

impl GuestSocket for HvStream {
    fn try_clone(&self) -> io::Result<Box<dyn GuestSocket>> {
        Ok(Box::new(self.clone()))
    }

    fn close(&self) {
        HvStream::close(self);
    }
}

/// The guest with this runtime ID, on the vsock port its sshd is behind.
struct Machine {
    runtime_id: u128,
}

impl Dial for Machine {
    fn dial(&self) -> io::Result<Box<dyn GuestSocket>> {
        Ok(Box::new(HvStream::connect(self.runtime_id, GUEST_PORT, CONNECT)?))
    }
}

/// Starts listening on `127.0.0.1:port` for the running machine, and says what came of it. A
/// machine without SSH is no reason not to run it, so a failure is told and the machine goes on.
pub fn listen(port: u16, running: &ComputeSystem, sink: &Arc<dyn Sink>) -> Option<Listener> {
    let runtime_id = match running.properties() {
        Ok(properties) => properties.runtime_id,
        Err(error) => {
            sink.problem(format!(
                "There is no SSH over a Hyper-V socket this boot, as its ID can't be read: {error}"
            ));
            return None;
        }
    };
    let Some(runtime_id) = parse_guid(&runtime_id) else {
        sink.problem(format!(
            "There is no SSH over a Hyper-V socket this boot, as HCS gave the machine the runtime ID {runtime_id:?}."
        ));
        return None;
    };
    match Listener::start(port, Arc::new(Machine { runtime_id }), Arc::clone(sink)) {
        Ok(listener) => {
            sink.note(format!(
                "SSH to it, with no network needed: {}, once the guest is up.",
                guesthouse_control::ssh_line(port, "USER")
            ));
            Some(listener)
        }
        Err(error) => {
            sink.problem(format!("There is no SSH over a Hyper-V socket this boot, as port {port} can't be listened on: {error}"));
            None
        }
    }
}
