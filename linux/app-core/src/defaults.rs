//! What a new machine starts as, for the app's dialog and the command line alike.

use crate::account::default_user;
use crate::iso::Iso;
use crate::machine::Kind;
use std::path::Path;

/// What a machine that is only a plain VM gets no part in.
pub const NO_GPU: &str = "This machine gets no GPU acceleration. It runs on the CPU, with a basic display.";

/// Sizes to start from. A plain machine is usually something small being tried out.
pub struct Sizes {
    pub cores: u32,
    pub ram: u64,
    pub disk: u32,
}

const WINDOWS_SIZES: Sizes = Sizes { cores: 8, ram: 16, disk: 128 };
const PLAIN_SIZES: Sizes = Sizes { cores: 4, ram: 4, disk: 32 };
const UBUNTU_SIZES: Sizes = Sizes { cores: 8, ram: 8, disk: 80 };
/// What Microsoft asks for Windows 11, for a Windows disc that's installed by hand.
const PLAIN_WINDOWS_SIZES: Sizes = Sizes { cores: 4, ram: 4, disk: 64 };

pub fn sizes_for(kind: Kind) -> Sizes {
    match kind {
        Kind::Windows => WINDOWS_SIZES,
        Kind::Linux => UBUNTU_SIZES,
        Kind::Plain => PLAIN_SIZES,
    }
}

/// Sizes for what this disc makes here. A Windows disc that's only a plain VM still has to pass
/// Windows' own setup.
pub fn sizes_for_disc(iso: &Iso) -> Sizes {
    if iso.kind == Kind::Plain && iso.windows_disc { PLAIN_WINDOWS_SIZES } else { sizes_for(iso.kind) }
}

pub fn suggested_name(kind: Kind, path: &str) -> String {
    match kind {
        Kind::Windows => "Windows 11".to_owned(),
        Kind::Linux => "Ubuntu".to_owned(),
        Kind::Plain => {
            Path::new(path).file_stem().map_or_else(String::new, |name| name.to_string_lossy().into_owned())
        }
    }
}

/// The name as the machine keeps it: empty for the default, so a machine made with the default reads
/// the same as one from before the choice.
pub fn account_name(kind: Kind, typed: &str) -> String {
    if typed == default_user(kind) { String::new() } else { typed.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_account_is_kept_as_no_choice() {
        assert_eq!(account_name(Kind::Windows, "gpupv"), "");
        assert_eq!(account_name(Kind::Linux, "desk"), "");
        assert_eq!(account_name(Kind::Windows, "ana"), "ana");
        assert_eq!(account_name(Kind::Linux, "gpupv"), "gpupv");
    }

    #[test]
    fn a_windows_disc_installed_by_hand_gets_the_disk_windows_asks_for() {
        let disc =
            |kind, windows_disc| Iso { kind, title: String::new(), edition: String::new(), windows_disc };
        assert_eq!(sizes_for_disc(&disc(Kind::Plain, true)).disk, 64);
        assert_eq!(sizes_for_disc(&disc(Kind::Plain, false)).disk, PLAIN_SIZES.disk);
        assert_eq!(sizes_for_disc(&disc(Kind::Windows, true)).disk, WINDOWS_SIZES.disk);
    }

    #[test]
    fn each_kind_of_disc_has_its_name() {
        assert_eq!(suggested_name(Kind::Windows, "/discs/win.iso"), "Windows 11");
        assert_eq!(suggested_name(Kind::Linux, "/discs/server.iso"), "Ubuntu");
        assert_eq!(suggested_name(Kind::Plain, "/discs/tiny-linux.iso"), "tiny-linux");
    }
}
