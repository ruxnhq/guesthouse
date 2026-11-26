//! A backend that serves canned [`ShareInfo`] values, for the page's pictures and for trying it with no
//! guest. A few calls change what it serves, so the buttons do something, but nothing is real.

use super::Backend;
use guesthouse_remote_view_core::share::{CloseReason, ShareInfo, ShareState};
use std::sync::Mutex;

struct Inner {
    allowed: bool,
    info: ShareInfo,
}

pub struct Fixture(Mutex<Inner>);

impl Fixture {
    pub fn new(allowed: bool, info: ShareInfo) -> Self {
        Self(Mutex::new(Inner { allowed, info }))
    }

    fn change(&self, how: impl FnOnce(&mut Inner)) -> Result<ShareInfo, String> {
        let mut inner = self.0.lock().map_err(|_| "The fixture broke.".to_owned())?;
        how(&mut inner);
        Ok(inner.info.clone())
    }
}

impl Backend for Fixture {
    fn allowed(&self) -> bool {
        self.0.lock().is_ok_and(|inner| inner.allowed)
    }

    fn info(&self) -> Result<ShareInfo, String> {
        self.change(|_| {})
    }

    fn share(&self, on: bool, pairing: bool, seconds: Option<u64>) -> Result<ShareInfo, String> {
        self.change(|inner| {
            inner.allowed = on;
            inner.info.state = if on { ShareState::Listening } else { ShareState::Off };
            inner.info.pairing = on && pairing;
            inner.info.pairing_until_ms =
                (on && pairing).then(|| super::now_ms() + seconds.unwrap_or(120) * 1000);
        })
    }

    fn pair_close(&self) -> Result<ShareInfo, String> {
        self.change(|inner| {
            inner.info.pairing = false;
            inner.info.pairing_until_ms = None;
            inner.info.closed_reason = Some(CloseReason::Asked);
        })
    }

    fn pair_accept(&self) -> Result<ShareInfo, String> {
        self.change(|inner| {
            inner.info.pending = None;
            inner.info.closed_reason = Some(CloseReason::Paired);
        })
    }

    fn pair_decline(&self) -> Result<ShareInfo, String> {
        self.change(|inner| {
            inner.info.pending = None;
            inner.info.closed_reason = Some(CloseReason::Declined);
        })
    }

    fn device_allow(&self, fingerprint: &str, what: &str, on: bool) -> Result<ShareInfo, String> {
        self.change(|inner| {
            for device in inner.info.devices.iter_mut().filter(|device| device.fingerprint == fingerprint) {
                match what {
                    "input" => device.permissions.input = on,
                    "clipboard" => device.permissions.clipboard = on,
                    "files" => device.permissions.files = on,
                    "sound" => device.permissions.sound = on,
                    "mic" => device.permissions.mic = on,
                    "camera" => device.permissions.camera = on,
                    _ => {}
                }
            }
        })
    }

    fn device_revoke(&self, fingerprint: &str) -> Result<ShareInfo, String> {
        self.change(|inner| inner.info.devices.retain(|device| device.fingerprint != fingerprint))
    }

    fn device_drop(&self, fingerprint: &str) -> Result<ShareInfo, String> {
        self.change(|inner| {
            for device in inner.info.devices.iter_mut().filter(|device| device.fingerprint == fingerprint) {
                device.connected = false;
            }
        })
    }
}
