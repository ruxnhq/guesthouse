// SPDX-License-Identifier: GPL-2.0-or-later
//! The agent's own use of a machine's QMP socket: one short conversation at a time. QEMU takes
//! one client per socket, so the agent never holds the connection; it comes, asks and leaves.

use crate::link::{CAPABILITIES, HANDSHAKE_TIMEOUT, WRITE_TIMEOUT, connect_long, read_line, refused};
use std::io::{self, BufReader, Write};
use std::path::Path;

/// Connects, does the handshake, and runs `command` (a QMP command name such as
/// `system_powerdown`) if there is one. With `None` it only proves the socket answers.
pub fn talk(socket: &Path, command: Option<&str>) -> io::Result<()> {
    let stream = connect_long(socket)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    if !read_line(&mut reader)?.contains("\"QMP\"") {
        return Err(refused("no QMP greeting"));
    }
    (&stream).write_all(CAPABILITIES.as_bytes())?;
    answer(&mut reader)?;
    if let Some(command) = command {
        let request = format!("{{\"execute\":\"{command}\"}}\r\n");
        (&stream).write_all(request.as_bytes())?;
        // `quit` may close the socket before it answers, which is as good as an answer.
        match answer(&mut reader) {
            Err(error) if command == "quit" && error.kind() == io::ErrorKind::ConnectionRefused => {}
            other => other?,
        }
    }
    Ok(())
}

/// Reads until QEMU's `return` or `error`, skipping the events it sends meanwhile.
fn answer(reader: &mut impl io::BufRead) -> io::Result<()> {
    loop {
        let line = read_line(reader)?;
        if line.contains("\"return\"") {
            return Ok(());
        }
        if line.contains("\"error\"") {
            return Err(refused(line.trim()));
        }
    }
}
