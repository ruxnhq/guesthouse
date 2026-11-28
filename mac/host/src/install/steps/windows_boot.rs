// SPDX-License-Identifier: GPL-2.0-or-later
//! Windows' own parts of the steps that a Windows installer disc needs from the host: the key
//! that boots it, and the word about a disk that has gone quiet.
//!
//! A Windows disc boots only if a key is pressed while the firmware says "Press any key to boot
//! from CD or DVD", a countdown of a few seconds, and times out otherwise, which leaves a
//! machine with nothing to boot and an install waiting for hours. The Linux host presses the
//! space bar for the same reason. Here the key goes in over QMP, for as long as the prompt
//! stands, which the machine's serial log says: the prompt appears in it when the firmware
//! shows it, and the log goes on with the time-out when it is not answered.

use super::RealSteps;
use crate::install::runner::StepError;
use crate::install::windows::marker;

/// What the firmware says on the serial port.
const PROMPT: &str = "Press any key to boot from CD or DVD";
const TIMED_OUT: &str = "Time out";
const NOTHING_TO_BOOT: &str = "BdsDxe: No bootable option";
/// The key pressed, as QMP names it.
const KEY: &str = "spc";
/// How often the key is pressed, and the log looked at, while the prompt stands.
const KEY_EVERY_MS: u64 = 500;
/// How often the log is looked at while there is nothing to answer.
const LOOK_EVERY_MS: u64 = 200;
/// How long a prompt is answered: its countdown is a few seconds, so this is the whole of it.
const PROMPT_STANDS_MS: u64 = 8_000;
/// How long, after the prompt has been answered or has timed out, a second one is waited for
/// (the firmware shows it again for the next boot option).
const SECOND_PROMPT_MS: u64 = 8_000;
/// How long the firmware gets to show the first prompt, from the machine's start.
const FIRST_PROMPT_MS: u64 = 90_000;

/// The disk is called quiet when it has written nothing for this long.
pub(super) const QUIET_AFTER_MS: u64 = 5 * 60_000;

impl RealSteps<'_> {
    /// Presses the key while the firmware's prompt stands, for each prompt it shows, and stops when
    /// the installer is running (it has said something on the serial port), when no prompt comes,
    /// or when the firmware has nothing to boot, which is said as an error and not waited out.
    pub(super) fn pass_boot_prompt(&mut self) -> Result<(), StepError> {
        let id = self.setup.id.clone();
        let began = self.control.now_ms();
        let mut prompts = 0;
        // When the last prompt first showed, or the last time-out was seen: what the waits count from.
        let mut since = began;
        loop {
            let now = self.control.now_ms();
            let serial = self.control.serial(&id);
            if serial.contains(marker::GUEST_SAYS) {
                return Ok(());
            }
            if serial.contains(NOTHING_TO_BOOT) {
                return Err(StepError::coded(
                    "no_boot",
                    "The installer disc did not start: the firmware found nothing to boot from. Try again.",
                ));
            }
            let shown = serial.matches(PROMPT).count();
            if shown > prompts {
                prompts = shown;
                since = now;
            }
            let after_prompt = serial.rsplit_once(PROMPT).map(|(_, rest)| rest);
            let timed_out = after_prompt.is_some_and(|rest| rest.contains(TIMED_OUT));
            if prompts > 0 && !timed_out && now - since < PROMPT_STANDS_MS {
                self.control.press_key(&id, KEY).map_err(StepError::new)?;
                self.control.sleep_ms(KEY_EVERY_MS);
                continue;
            }
            let waited = now - since;
            let gave_up = if prompts == 0 {
                waited >= FIRST_PROMPT_MS
            } else {
                waited >= PROMPT_STANDS_MS + SECOND_PROMPT_MS
            };
            if gave_up {
                return Ok(());
            }
            self.control.sleep_ms(LOOK_EVERY_MS);
        }
    }

    /// For how many minutes the disk has written nothing, once that is as long as `QUIET_AFTER_MS`;
    /// none while it is being written to, or when the machine cannot say. `last` is the bytes and
    /// the time of the last change seen, kept by the caller between looks.
    pub(super) fn quiet_minutes(&mut self, last: &mut Option<(u64, u64)>) -> Option<u64> {
        let written = self.control.disk_written(&self.setup.id)?;
        let now = self.control.now_ms();
        match last {
            Some((bytes, _)) if *bytes != written => *last = Some((written, now)),
            None => *last = Some((written, now)),
            Some(_) => {}
        }
        let (_, changed) = (*last)?;
        let quiet = now - changed;
        (quiet >= QUIET_AFTER_MS).then_some(quiet / 60_000)
    }
}
