use super::*;
use std::sync::Mutex;

/// A source that counts up, as a test hands one to code that needs randomness.
struct Counting(Mutex<u8>);

impl RandomSource for Counting {
    fn fill(&self, bytes: &mut [u8]) {
        let mut next = self.0.lock().unwrap();
        for byte in bytes {
            *byte = *next;
            *next = next.wrapping_add(1);
        }
    }
}

#[test]
fn code_that_needs_randomness_takes_any_source() {
    fn tag(random: &dyn RandomSource) -> String {
        let mut bytes = [0; 4];
        random.fill(&mut bytes);
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    let source = Counting(Mutex::new(0xfe));
    assert_eq!(tag(&source), "feff0001");
    assert_eq!(tag(&source), "02030405");
}
