//! Who made a Windows machine's GPU. A machine keeps this as a real value, so each place that has to
//! treat the makers differently is a `match` on [`GpuMaker`], and a new maker is a compile error at
//! every one of them until it's handled.

use serde::{Deserialize, Serialize};

/// A maker whose card a Windows guest's GPU-PV can run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuMaker {
    Nvidia,
    Amd,
    Intel,
}

impl GpuMaker {
    /// Every maker, for code that has to look at all of them.
    pub const ALL: [GpuMaker; 3] = [Self::Nvidia, Self::Amd, Self::Intel];

    /// The maker's PCI vendor number.
    pub fn vendor(self) -> u16 {
        match self {
            Self::Nvidia => 0x10DE,
            Self::Amd => 0x1002,
            Self::Intel => 0x8086,
        }
    }

    /// The name people know it by.
    pub fn name(self) -> &'static str {
        match self {
            Self::Nvidia => "NVIDIA",
            Self::Amd => "AMD",
            Self::Intel => "Intel",
        }
    }

    /// The maker whose PCI vendor number this is, if it's one of these.
    pub fn from_vendor(vendor: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|maker| maker.vendor() == vendor)
    }

    /// What a machine's file records for this maker's card at `pci`, if it records anything. No address
    /// is setup's NVIDIA card and an address alone is an AMD card, as they always were, so a file with
    /// either stays one an older app reads. An NVIDIA card named by its address is written down, and
    /// so is a maker that came later.
    pub fn recorded_for(self, pci: &str) -> Option<Self> {
        match self {
            Self::Nvidia => (!pci.is_empty()).then_some(self),
            Self::Amd => None,
            Self::Intel => Some(self),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_maker_is_found_by_its_vendor_number_and_by_no_other() {
        for maker in GpuMaker::ALL {
            assert_eq!(GpuMaker::from_vendor(maker.vendor()), Some(maker));
        }
        assert_eq!(GpuMaker::from_vendor(0x1414), None, "Microsoft's render device is not a card maker");
    }

    #[test]
    fn only_a_maker_the_address_does_not_tell_is_recorded() {
        let recorded = |pci: &str| -> Vec<GpuMaker> {
            GpuMaker::ALL.into_iter().filter_map(|maker| maker.recorded_for(pci)).collect()
        };
        assert_eq!(recorded(""), [GpuMaker::Intel], "no address is setup's NVIDIA card");
        assert_eq!(
            recorded("0000:02:00.0"),
            [GpuMaker::Nvidia, GpuMaker::Intel],
            "an address alone is AMD's"
        );
    }
}
