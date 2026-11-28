//! Guesthouse's Android image, as the Linux host fetches it: its name, its SHA-256 and where it is.

/// Guesthouse's Android image (LineageOS for VMs with its setup app, microG and ARM64 translation, built
/// from `android/` in Guesthouse's source), as a compressed qcow2 of its system disk. A new one gets a new
/// name, so one fetched before is never taken for it.
pub const ANDROID_IMAGE_NAME: &str = "guesthouse-android-17-2026-11-23.qcow2";
pub const ANDROID_IMAGE_SHA256: &str = "d547bda57a11624e201bd6c4ea50f3e8da3f63997d5a2ffb575220050ec10917";
pub const ANDROID_IMAGE_HOST: &str = "https://files.ruxn.com/p/guesthouse/";
