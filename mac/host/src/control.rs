// SPDX-License-Identifier: GPL-2.0-or-later
//! The agent's own use of a machine's QMP socket: one short conversation at a time. QEMU takes
//! one client per socket, so the agent never holds the connection; it comes, asks and leaves.

use crate::link::{CAPABILITIES, HANDSHAKE_TIMEOUT, WRITE_TIMEOUT, connect_long, read_line, refused};
use serde_json::{Value, json};
use std::io::{self, BufReader, Write};
use std::path::Path;

/// Connects, does the handshake, and runs `command` (a QMP command name such as
/// `system_powerdown`) if there is one. With `None` it only proves the socket answers.
pub fn talk(socket: &Path, command: Option<&str>) -> io::Result<()> {
    exchange(socket, command.map(|name| json!({ "execute": name }))).map(|_| ())
}

/// Presses and lets go of the key `qcode` names (`spc` for the space bar) in the guest, as QMP's
/// `send-key`: what a person at the keyboard would do, before any guest software runs.
pub fn send_key(socket: &Path, qcode: &str) -> io::Result<()> {
    let keys =
        json!({ "execute": "send-key", "arguments": { "keys": [{ "type": "qcode", "data": qcode }] } });
    exchange(socket, Some(keys)).map(|_| ())
}

/// The bytes the machine has written to its drive `hd`, from `query-blockstats`: none when QEMU
/// does not say.
pub fn bytes_written(socket: &Path) -> io::Result<Option<u64>> {
    let stats = exchange(socket, Some(json!({ "execute": "query-blockstats" })))?;
    Ok(stats.and_then(|value| written_to(&value, "hd")))
}

/// The `wr_bytes` of the drive called `device` in a `query-blockstats` answer.
fn written_to(stats: &Value, device: &str) -> Option<u64> {
    stats
        .as_array()?
        .iter()
        .find(|entry| entry.get("device").and_then(Value::as_str) == Some(device))?
        .get("stats")?
        .get("wr_bytes")?
        .as_u64()
}

/// One conversation: the greeting, the capabilities, and the one `request` if there is one;
/// the `return` it is answered with comes back.
fn exchange(socket: &Path, request: Option<Value>) -> io::Result<Option<Value>> {
    let stream = connect_long(socket)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    if !read_line(&mut reader)?.contains("\"QMP\"") {
        return Err(refused("no QMP greeting"));
    }
    (&stream).write_all(CAPABILITIES.as_bytes())?;
    answer(&mut reader)?;
    let Some(request) = request else {
        return Ok(None);
    };
    let quitting = request["execute"] == "quit";
    (&stream).write_all(format!("{request}\r\n").as_bytes())?;
    // `quit` may close the socket before it answers, which is as good as an answer.
    match answer(&mut reader) {
        Err(error) if quitting && error.kind() == io::ErrorKind::ConnectionRefused => Ok(None),
        other => other.map(Some),
    }
}

/// Reads until QEMU's `return` or `error`, skipping the events it sends meanwhile.
fn answer(reader: &mut impl io::BufRead) -> io::Result<Value> {
    loop {
        let line = read_line(reader)?;
        if line.contains("\"return\"") {
            let value: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            return Ok(value.get("return").cloned().unwrap_or(Value::Null));
        }
        if line.contains("\"error\"") {
            return Err(refused(line.trim()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Write};
    use std::os::unix::net::UnixListener;
    use std::thread;

    /// A QMP server that answers the one request it is sent with `reply`, and hands back what it
    /// was asked.
    fn serve(name: &str, reply: &'static str) -> (std::path::PathBuf, thread::JoinHandle<String>) {
        let socket = std::env::temp_dir().join(format!("gh-ctl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let mut reader = BufReader::new(stream);
            writeln!(writer, r#"{{"QMP": {{"version": {{}}, "capabilities": []}}}}"#).unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            writeln!(writer, r#"{{"return": {{}}}}"#).unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            writeln!(writer, "{reply}").unwrap();
            line
        });
        (socket, handle)
    }

    #[test]
    fn a_key_goes_as_send_key_with_its_qcode() {
        let (socket, asked) = serve("key", r#"{"return": {}}"#);
        send_key(&socket, "spc").unwrap();
        let request: Value = serde_json::from_str(&asked.join().unwrap()).unwrap();
        assert_eq!(request["execute"], "send-key");
        assert_eq!(request["arguments"]["keys"][0], json!({"type": "qcode", "data": "spc"}));
        let _ = std::fs::remove_file(socket);
    }

    #[test]
    fn what_the_disk_has_written_is_read_from_the_block_statistics() {
        let reply = r#"{"return": [{"device": "other", "stats": {"wr_bytes": 5}}, {"device": "hd", "stats": {"wr_bytes": 1457000000}}]}"#;
        let (socket, asked) = serve("stats", reply);
        assert_eq!(bytes_written(&socket).unwrap(), Some(1_457_000_000));
        assert!(asked.join().unwrap().contains("query-blockstats"));
        let _ = std::fs::remove_file(socket);
        assert_eq!(written_to(&json!([]), "hd"), None);
    }
}
