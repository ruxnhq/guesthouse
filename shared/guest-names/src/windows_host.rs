//! The Windows host's rules for a guest's names: what a shared folder is called in the guest.

/// What the folder in a machine's settings at `slot` is called, to HCS and in the guest: `shared`
/// for the first, as when there could be only one, then `shared2` and on.
pub fn share_name(slot: usize) -> String {
    match slot {
        0 => "shared".to_owned(),
        _ => format!("shared{}", slot + 1),
    }
}

#[cfg(test)]
mod tests;
