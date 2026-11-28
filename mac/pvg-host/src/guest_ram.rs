// SPDX-License-Identifier: GPL-2.0-or-later
//! The guest's memory, from the files QEMU hands over. With `pvg-sys`'s own, this is where the
//! process's `unsafe` is.
//!
//! QEMU's RAM is a shared memory object that QEMU made and nobody can open by name. Its forwarding
//! device sends the file of each range over the socket, and each is mapped here, shared, so that
//! this process and the guest see the same pages.

use crate::wire::MESSAGE_BYTES;
use guesthouse_pvg_sys::profile::PAGE_BYTES;
use guesthouse_pvg_sys::{GuestMemory, Range};
use std::io::{self, Read};
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::ptr;

/// Room for the one file a message may bring, in the form `recvmsg` wants: a header and an `int`.
const CONTROL_ROOM: usize = 64;

/// One range as QEMU names it.
pub struct Shared {
    pub guest_address: u64,
    pub length: u64,
    pub file_offset: u64,
    pub file: OwnedFd,
}

/// The mappings behind a [`GuestMemory`], unmapped when the last user of it has gone.
struct Mappings(Vec<(*mut libc::c_void, usize)>);

// SAFETY: the mappings are this value's alone, and it does nothing with them but unmap them.
unsafe impl Send for Mappings {}
// SAFETY: as above.
unsafe impl Sync for Mappings {}

impl Drop for Mappings {
    fn drop(&mut self) {
        for &(start, length) in &self.0 {
            // SAFETY: each pair came from `mmap` in `map`, and is unmapped once, here.
            unsafe { libc::munmap(start, length) };
        }
    }
}

/// Reads one message, and the file that came with it if one did.
pub fn receive(stream: &mut UnixStream) -> io::Result<([u8; MESSAGE_BYTES], Option<OwnedFd>)> {
    let mut message = [0u8; MESSAGE_BYTES];
    let mut control = [0u64; CONTROL_ROOM / 8];
    let mut part = libc::iovec { iov_base: message.as_mut_ptr().cast(), iov_len: MESSAGE_BYTES };
    // SAFETY: all zeroes is a valid `msghdr`, and the fields that matter are set below.
    let mut header: libc::msghdr = unsafe { mem::zeroed() };
    header.msg_iov = &raw mut part;
    header.msg_iovlen = 1;
    header.msg_control = control.as_mut_ptr().cast();
    header.msg_controllen = CONTROL_ROOM as _;

    let got = loop {
        // SAFETY: the header names one buffer of `MESSAGE_BYTES` bytes and a control buffer of
        // `CONTROL_ROOM`, both alive through the call.
        let got = unsafe { libc::recvmsg(stream.as_raw_fd(), &raw mut header, 0) };
        if got >= 0 {
            break got as usize;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    if got == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }

    let mut file = None;
    // SAFETY: the kernel filled the control buffer the header names, and these calls walk it as
    // the header's length bounds it.
    let mut entry = unsafe { libc::CMSG_FIRSTHDR(&raw const header) };
    while !entry.is_null() {
        // SAFETY: `entry` is a control message inside the buffer, by the walk above.
        let (level, kind, length) =
            unsafe { ((*entry).cmsg_level, (*entry).cmsg_type, (*entry).cmsg_len as usize) };
        // SAFETY: only the length is computed, from a constant.
        let one_file = unsafe { libc::CMSG_LEN(mem::size_of::<libc::c_int>() as u32) } as usize;
        if level == libc::SOL_SOCKET && kind == libc::SCM_RIGHTS && length >= one_file {
            // SAFETY: a rights message of at least this length holds one descriptor, which may not
            // be aligned, and which is this process's to own from now on.
            let descriptor = unsafe { ptr::read_unaligned(libc::CMSG_DATA(entry).cast::<libc::c_int>()) };
            // SAFETY: the descriptor is open and nothing else holds it.
            let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
            // SAFETY: setting a flag on a descriptor this process owns.
            unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
            file = file.or(Some(owned));
        }
        // SAFETY: the walk stays inside the control buffer the header names.
        entry = unsafe { libc::CMSG_NXTHDR(&raw const header, entry) };
    }
    // A stream may hand over less than a whole message at once. The file comes with the first byte.
    stream.read_exact(&mut message[got..])?;
    Ok((message, file))
}

/// Maps every range, and gives them back as guest memory that unmaps itself.
pub fn map(shared: Vec<Shared>) -> io::Result<GuestMemory> {
    let on_a_page = |value: u64| value.is_multiple_of(PAGE_BYTES);
    let mut mappings = Mappings(Vec::new());
    let mut ranges = Vec::new();
    for range in shared {
        let length = usize::try_from(range.length).ok().filter(|&length| length != 0);
        let offset = libc::off_t::try_from(range.file_offset).ok();
        let (Some(length), Some(offset)) = (length, offset) else {
            return Err(io::Error::other("a range of guest memory with no size, or one beyond all files"));
        };
        if !on_a_page(range.guest_address) || !on_a_page(range.length) || !on_a_page(range.file_offset) {
            return Err(io::Error::other(format!(
                "the range at {:#x} doesn't lie on the device's pages",
                range.guest_address
            )));
        }
        // SAFETY: a new shared mapping of a file QEMU handed over, at an address of the kernel's
        // choosing, touches nothing that exists. A range past the file's end fails at first use
        // and not here, and QEMU, which made the file, is trusted with its size.
        let start = unsafe {
            libc::mmap(
                ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                range.file.as_raw_fd(),
                offset,
            )
        };
        if start == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        mappings.0.push((start, length));
        ranges.push(Range {
            guest_address: range.guest_address,
            length: range.length,
            host_address: start.cast(),
        });
    }
    // SAFETY: each range was mapped just now, readable and writable, for its whole length, stays
    // mapped for as long as `mappings` lives, and no reference into it exists here.
    unsafe { GuestMemory::from_ranges(ranges, Box::new(mappings)) }.map_err(io::Error::other)
}
