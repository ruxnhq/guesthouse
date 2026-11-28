//! Fid numbers: the client picks them, and the server only remembers them.

/// Hands out fids and takes them back for reuse.
#[derive(Debug, Default)]
pub struct FidPool {
    next: u32,
    free: Vec<u32>,
}

impl FidPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// A fid that is not in use: a freed one if there is one, else the next new number. It never
    /// gives `NOFID` in practice.
    pub fn alloc(&mut self) -> u32 {
        if let Some(fid) = self.free.pop() {
            return fid;
        }
        let fid = self.next;
        // Nothing holds 2^32 fids at once, so the numbers never reach NOFID.
        self.next = self.next.saturating_add(1);
        fid
    }

    /// Takes a fid back. One that was never given, or is already free, is ignored.
    pub fn release(&mut self, fid: u32) {
        if fid < self.next && !self.free.contains(&fid) {
            self.free.push(fid);
        }
    }
}
