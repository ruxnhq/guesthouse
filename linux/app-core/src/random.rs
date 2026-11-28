//! Random bytes for MAC addresses, tokens and snapshot tags.

use guesthouse_basics::random::RandomSource;
use std::fs::File;
use std::io::Read;

const RANDOM_DEVICE: &str = "/dev/urandom";

/// Panics if the kernel's random device can't be read, which only a broken system does.
pub fn random_bytes<const SIZE: usize>() -> [u8; SIZE] {
    let mut bytes = [0; SIZE];
    File::open(RANDOM_DEVICE).and_then(|mut device| device.read_exact(&mut bytes)).expect("no random device");
    bytes
}

pub fn random_hex<const SIZE: usize>() -> String {
    random_bytes::<SIZE>().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The kernel's random device as a source, for code that takes its randomness as a value.
pub struct KernelRandom;

impl RandomSource for KernelRandom {
    /// Panics if the kernel's random device can't be read, as [`random_bytes`] does.
    fn fill(&self, bytes: &mut [u8]) {
        File::open(RANDOM_DEVICE).and_then(|mut device| device.read_exact(bytes)).expect("no random device");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_fills_what_it_is_given() {
        let (mut first, mut second) = ([0u8; 32], [0u8; 32]);
        KernelRandom.fill(&mut first);
        KernelRandom.fill(&mut second);
        assert_ne!(first, [0; 32]);
        assert_ne!(first, second);
        KernelRandom.fill(&mut []);
    }
}
