//! The private data a guest sends is parsed in plain functions over byte slices, and a daemon that
//! panics in one stops its VM. This feeds random and mutated bytes to each of them and checks that
//! every call returns.
//!
//! `FUZZ_ROUNDS` runs it for longer, and `FUZZ_SEED` picks where it starts.

#![cfg(target_os = "linux")]

use guesthouse_backend_amd::protocol::{allocation, command_data, context, escape, hw_queue, video_escape};
use std::env;

fn setting(name: &str, default: u64) -> u64 {
    env::var(name).ok().and_then(|text| text.parse().ok()).unwrap_or(default)
}

/// A deterministic xorshift, so a failure can be reproduced from the seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() as usize) % bound.max(1)
    }

    /// A dword that is often a size, a count or an edge value.
    fn dword(&mut self) -> u32 {
        const EDGES: [u32; 14] = [
            0,
            1,
            2,
            3,
            8,
            0x40,
            0x80,
            0x114,
            0x120,
            0x1_0000,
            0x2_0000,
            0x7FFF_FFFF,
            0xFFFF_FFFF,
            0xFFFF_FFF8,
        ];
        match self.below(4) {
            0 => EDGES[self.below(EDGES.len())],
            1 => self.next() as u32 % 0x400,
            _ => self.next() as u32,
        }
    }

    /// Up to `max` bytes, as random bytes or as random dwords.
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let len = self.below(max + 1);
        if self.below(2) == 0 {
            return (0..len).map(|_| self.next() as u8).collect();
        }
        let mut out = Vec::with_capacity(len + 4);
        while out.len() < len {
            out.extend_from_slice(&self.dword().to_le_bytes());
        }
        out.truncate(len);
        out
    }

    /// A few dwords, bytes or a cut taken out of `data`.
    fn mutate(&mut self, data: &mut Vec<u8>) {
        for _ in 0..1 + self.below(4) {
            match self.below(4) {
                0 if data.len() >= 4 => {
                    let at = self.below(data.len() - 3);
                    data[at..at + 4].copy_from_slice(&self.dword().to_le_bytes());
                }
                1 if !data.is_empty() => {
                    let at = self.below(data.len());
                    data[at] = self.next() as u8;
                }
                2 => data.truncate(self.below(data.len() + 1)),
                _ => {
                    let extra = self.below(64);
                    data.extend((0..extra).map(|_| self.next() as u8));
                }
            }
        }
    }
}

fn put(data: &mut [u8], at: usize, value: u32) {
    if let Some(slot) = data.get_mut(at..at + 4) {
        slot.copy_from_slice(&value.to_le_bytes());
    }
}

/// Bytes shaped like private data that starts with a size and a few small fields, so the checks
/// past the first one are reached.
fn shaped(rng: &mut Rng, max: usize) -> Vec<u8> {
    let mut data = rng.bytes(max);
    if data.len() >= 8 {
        let claimed = if rng.below(2) == 0 { data.len() as u32 } else { rng.dword() };
        put(&mut data, 0, claimed);
    }
    data
}

fn run_allocation(data: &[u8]) {
    let _ = allocation::parse(data);
    let _ = allocation::shape(data);
}

/// A submission with a count and records whose lengths are random, and some that are right.
fn submission(rng: &mut Rng) -> Vec<u8> {
    let mut data = vec![0u8; 8];
    put(&mut data, 0, 1 + rng.below(12) as u32);
    let records = rng.below(6);
    put(&mut data, 4, if rng.below(4) == 0 { rng.dword() } else { records as u32 });
    for _ in 0..records {
        let length = if rng.below(3) == 0 { rng.dword() as usize % 0x80 } else { 8 + rng.below(0x60) };
        let mut record = rng.bytes(length);
        record.resize(length.max(8), 0);
        put(&mut record, 0, if rng.below(2) == 0 { rng.below(0x40) as u32 } else { rng.dword() });
        let len = record.len() as u32;
        put(&mut record, 4, if rng.below(4) == 0 { rng.dword() } else { len });
        data.extend_from_slice(&record);
    }
    data
}

fn run_submission(data: &[u8], dma_buffer: u64) {
    if command_data::check(data, dma_buffer).is_ok() {
        let _ = command_data::command_buffers(data);
    }
}

/// An escape with a header the guest's driver could send, with or without its wrapper.
fn escape_bytes(rng: &mut Rng) -> Vec<u8> {
    let mut data = rng.bytes(0x300);
    data.resize(data.len().max(escape::HEADER_SIZE + 8), 0);
    let size = if rng.below(4) == 0 { rng.dword() } else { data.len() as u32 };
    put(&mut data, 0, size);
    put(&mut data, 4, if rng.below(4) == 0 { rng.dword() } else { 0x0001_0000 });
    let service = [1u32, 2, 3, 5, 6, 0, 9][rng.below(7)];
    let code = service << 24 | (rng.below(0x100) as u32) << 8 | [3u32, 3, 1, 2, 0xFF][rng.below(5)];
    put(&mut data, 8, if rng.below(8) == 0 { rng.dword() } else { code });
    if rng.below(2) == 0 {
        let mut wrapped = vec![0u8; escape::WRAPPER_SIZE];
        put(&mut wrapped, 0, escape::WRAPPER_KIND);
        put(&mut wrapped, 4, escape::WRAPPER_VERSION);
        wrapped.extend_from_slice(&data);
        data = wrapped;
    }
    data
}

fn run_escape(data: &[u8]) {
    let _ = escape::framing(data);
    let _ = escape::payload(data);
    let mut answered = data.to_vec();
    let _ = escape::answer(&mut answered);
    // An escape nothing in the module answers goes on to the video escapes, as the backend does.
    let mut answered = data.to_vec();
    let _ = escape::answer_with(&mut answered, |code, payload| {
        let Some(asked) = video_escape::read(code, payload) else { return false };
        match asked {
            video_escape::Asked::Start => video_escape::give_session(payload, 1),
            video_escape::Asked::Throughput { .. } => video_escape::give_throughput(payload, 1, 2),
            video_escape::Asked::End { .. } => {}
        }
        true
    });
}

fn run_video_payload(code: escape::EscapeCode, payload: &[u8]) {
    let mut payload = payload.to_vec();
    if let Some(asked) = video_escape::read(code, &payload) {
        match asked {
            video_escape::Asked::Start => video_escape::give_session(&mut payload, 7),
            video_escape::Asked::Throughput { .. } => video_escape::give_throughput(&mut payload, 3, 9),
            video_escape::Asked::End { .. } => {}
        }
    }
}

#[test]
fn no_private_data_parser_panics_on_random_bytes() {
    let rounds = setting("FUZZ_ROUNDS", 200_000);
    let seed = setting("FUZZ_SEED", 0x9E37_79B9_7F4A_7C15);
    eprintln!("private data fuzz: {rounds} rounds, seed {seed}");
    let mut rng = Rng(seed | 1);
    let mut kept: Vec<Vec<u8>> = Vec::new();
    for round in 0..rounds {
        let max = [0x20, 0x130, 0x400, 0x2000][rng.below(4)];
        let mut data = if round % 3 == 0 && !kept.is_empty() {
            let mut data = kept[rng.below(kept.len())].clone();
            rng.mutate(&mut data);
            data
        } else {
            shaped(&mut rng, max)
        };
        if round % 8 == 0 && data.len() >= 0x80 {
            // Wrapped as the guest's other driver generation sends it.
            let mut wrapped = vec![0u8; 0x40];
            put(&mut wrapped, 0, data.len() as u32);
            wrapped.append(&mut data);
            data = wrapped;
        }
        run_allocation(&data);
        let _ = context::flag_words(&data);
        let _ = context::check(&data);
        let _ = hw_queue::check(&data);
        run_submission(&data, rng.next() & 1);
        run_escape(&data);
        let sub = submission(&mut rng);
        run_submission(&sub, rng.next() & 1);
        let mut sub_mut = sub.clone();
        rng.mutate(&mut sub_mut);
        run_submission(&sub_mut, 0);
        let esc = escape_bytes(&mut rng);
        run_escape(&esc);
        let mut esc_mut = esc.clone();
        rng.mutate(&mut esc_mut);
        run_escape(&esc_mut);
        let code = escape::EscapeCode {
            service: 3,
            subfunction: [0x4D, 0x4E, 0xAC, 0x10][rng.below(4)] as u32,
            client: 3,
        };
        let mut payload = rng.bytes(0x100);
        if payload.len() >= 0x40 && rng.below(4) != 0 {
            put(&mut payload, 0, 0x40);
        }
        run_video_payload(code, &payload);
        if round % 16 == 0 {
            kept.push(data);
            kept.push(sub);
            kept.push(esc);
            if kept.len() > 64 {
                kept.drain(..16);
            }
        }
    }
}
