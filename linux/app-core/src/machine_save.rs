//! Saving the app's copy of a machine: only what the copy changed goes onto the file as it is now, so a
//! change made meanwhile by the command line or a task stays.

use crate::context::Context;
use crate::failure::Result;
use crate::machine::{Machine, SettingsChanges, load};

/// Every field changed from `before` to `after`.
pub(crate) fn field_changes(before: &Machine, after: &Machine) -> SettingsChanges {
    let as_map = |machine: &Machine| match serde_json::to_value(machine) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let (before, after) = (as_map(before), as_map(after));
    let mut changes: SettingsChanges = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, value)| (key.clone(), Some(value.clone())))
        .collect();
    changes.extend(before.keys().filter(|key| !after.contains_key(*key)).map(|key| (key.clone(), None)));
    changes
}

impl Machine {
    /// `saved`, which is the machine as its file has it now, with what changed from `known` to this
    /// copy. `known` is the file as this copy was last read from it or saved to it, so a field this copy
    /// didn't change takes whatever the command line or a task has saved since.
    pub fn caught_up(&self, known: &Machine, saved: &Machine) -> Result<Machine> {
        saved.with_changes(&field_changes(known, self))
    }

    /// Saves only what changed from `known` to this copy, onto the machine as its file has it now, and
    /// returns the machine as it was saved. See [`Machine::caught_up`].
    pub fn save_changes(&self, context: &Context, known: &Machine) -> Result<Machine> {
        let saved = load(context, &self.id)?;
        let changed = self.caught_up(known, &saved)?;
        changed.save(context)?;
        load(context, &self.id)
    }
}
