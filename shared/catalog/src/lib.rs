//! Where a guest's images come from, as data: each image's address, name, SHA-256 and size, and the
//! rule that picks an image from a published checksum list. A host fetches and checks the files its
//! own way; nothing here calls the system or the network, so every host can build on it.
//!
//! Each rule stays as the host that has it made it, and none stands in for another: today the Linux
//! host's Ubuntu disc ([`ubuntu_amd64`]), its Android image ([`android`]) and its macOS part
//! ([`macos_part`]).

pub mod android;
pub mod macos_part;
pub mod ubuntu_amd64;
