// SPDX-License-Identifier: GPL-2.0-or-later
//! The words on the socket between QEMU's forwarding device and this process.
//!
//! Every message is 32 bytes: two 32-bit numbers and three of 64 bits, little-endian. A range of
//! guest memory comes with its file as ancillary data. QEMU's end is its `pvg-fwd` device, which
//! has the same numbers.

pub const MESSAGE_BYTES: usize = 32;
/// What both ends have to agree on before anything else.
pub const PROTOCOL: u32 = 1;
/// More ranges than any machine's RAM comes in.
pub const MOST_RANGES: u64 = 64;

/// Which of the device's two sets of registers a read or a write is for, as the guest sees them:
/// by the number of the PCI function's BAR.
pub const BAR_DEVICE: u32 = 0;
pub const BAR_MAPPER: u32 = 2;

// From QEMU.
const HELLO: u32 = 1;
const RANGE: u32 = 2;
const READ: u32 = 3;
const WRITE: u32 = 4;
const RESET: u32 = 5;
// To QEMU.
const REPLY: u32 = 0x80;
const INTERRUPT: u32 = 0x81;
const READY: u32 = 0x82;

/// Where a register access goes, and how wide the guest made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    pub bar: u32,
    pub size: u32,
    pub offset: u64,
}

/// What QEMU says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// The first message: the protocol QEMU speaks, and how many ranges of guest memory follow.
    Hello {
        protocol: u32,
        ranges: u64,
    },
    /// One range of guest memory. Its file comes with the message.
    Range {
        guest_address: u64,
        length: u64,
        file_offset: u64,
    },
    Read(Access),
    Write(Access, u64),
    Reset,
}

/// What this process says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The device is made: how many bytes of registers the device has, and the mapper.
    Ready { device_bytes: u64, mapper_bytes: u64 },
    /// A request is done. For a read, the value.
    Reply(u64),
    /// The device raised an interrupt: 0 is the graphics device's vector, 1 the mapper's.
    Interrupt(u32),
}

fn words(bytes: &[u8; MESSAGE_BYTES]) -> (u32, u32, u64, u64, u64) {
    let word32 = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
    let word64 = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"));
    (word32(0), word32(4), word64(8), word64(16), word64(24))
}

fn bytes(kind: u32, arg: u32, a: u64, b: u64, c: u64) -> [u8; MESSAGE_BYTES] {
    let mut bytes = [0; MESSAGE_BYTES];
    bytes[0..4].copy_from_slice(&kind.to_le_bytes());
    bytes[4..8].copy_from_slice(&arg.to_le_bytes());
    bytes[8..16].copy_from_slice(&a.to_le_bytes());
    bytes[16..24].copy_from_slice(&b.to_le_bytes());
    bytes[24..32].copy_from_slice(&c.to_le_bytes());
    bytes
}

/// A register access's BAR is the lowest byte of its word, and its size the next.
fn access(arg: u32, offset: u64) -> Access {
    Access { bar: arg & 0xFF, size: arg >> 8 & 0xFF, offset }
}

impl Request {
    /// What a message says, or `None` for a kind this process doesn't know.
    pub fn read(message: &[u8; MESSAGE_BYTES]) -> Option<Self> {
        let (kind, arg, a, b, c) = words(message);
        Some(match kind {
            HELLO => Self::Hello { protocol: arg, ranges: a },
            RANGE => Self::Range { guest_address: a, length: b, file_offset: c },
            READ => Self::Read(access(arg, a)),
            WRITE => Self::Write(access(arg, a), b),
            RESET => Self::Reset,
            _ => return None,
        })
    }

    /// The message for it, which is what QEMU's end writes.
    #[cfg(test)]
    pub fn write(&self) -> [u8; MESSAGE_BYTES] {
        let arg = |access: &Access| access.bar | access.size << 8;
        match self {
            Self::Hello { protocol, ranges } => bytes(HELLO, *protocol, *ranges, 0, 0),
            Self::Range { guest_address, length, file_offset } => {
                bytes(RANGE, 0, *guest_address, *length, *file_offset)
            }
            Self::Read(access) => bytes(READ, arg(access), access.offset, 0, 0),
            Self::Write(access, value) => bytes(WRITE, arg(access), access.offset, *value, 0),
            Self::Reset => bytes(RESET, 0, 0, 0, 0),
        }
    }
}

impl Answer {
    pub fn write(&self) -> [u8; MESSAGE_BYTES] {
        match self {
            Self::Ready { device_bytes, mapper_bytes } => bytes(READY, 0, *device_bytes, *mapper_bytes, 0),
            Self::Reply(value) => bytes(REPLY, 0, 0, *value, 0),
            Self::Interrupt(vector) => bytes(INTERRUPT, *vector, 0, 0, 0),
        }
    }

    /// What QEMU's end makes of a message.
    #[cfg(test)]
    pub fn read(message: &[u8; MESSAGE_BYTES]) -> Option<Self> {
        let (kind, arg, a, b, _) = words(message);
        Some(match kind {
            READY => Self::Ready { device_bytes: a, mapper_bytes: b },
            REPLY => Self::Reply(b),
            INTERRUPT => Self::Interrupt(arg),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_reads_back_as_it_was_written() {
        let access = Access { bar: BAR_MAPPER, size: 8, offset: 0x1018 };
        let requests = [
            Request::Hello { protocol: PROTOCOL, ranges: 2 },
            Request::Range { guest_address: 0x4000_0000, length: 1 << 32, file_offset: 0x4000 },
            Request::Read(access),
            Request::Write(access, u64::MAX - 1),
            Request::Reset,
        ];
        for request in requests {
            assert_eq!(Request::read(&request.write()), Some(request));
        }
        let answers = [
            Answer::Ready { device_bytes: 0x4000, mapper_bytes: 0x1_0000 },
            Answer::Reply(7),
            Answer::Interrupt(1),
        ];
        for answer in answers {
            assert_eq!(Answer::read(&answer.write()), Some(answer));
        }
    }

    /// The numbers are QEMU's too, so they are held here as bytes and not only against themselves.
    #[test]
    fn a_write_lies_on_the_wire_as_qemu_s_end_lays_it() {
        let message = Request::Write(Access { bar: 2, size: 8, offset: 0x1008 }, 0x400).write();
        let mut wanted = [0u8; MESSAGE_BYTES];
        wanted[0] = 4;
        wanted[4] = 2;
        wanted[5] = 8;
        wanted[8..10].copy_from_slice(&[0x08, 0x10]);
        wanted[16..18].copy_from_slice(&[0x00, 0x04]);
        assert_eq!(message, wanted);
        assert_eq!(Answer::Interrupt(1).write()[..8], [0x81, 0, 0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn a_kind_nobody_sends_is_no_message() {
        let mut message = Request::Reset.write();
        message[0] = 0x7F;
        assert_eq!(Request::read(&message), None);
        assert_eq!(Answer::read(&message), None);
    }
}
