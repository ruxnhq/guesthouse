//! What the app's setup found, which it keeps as a JSON file in the data store.

use crate::store::store;
use guesthouse_backend::{DriverVersion, PciAddress, PciCard, Setup, driver_version_from_inf};
use guesthouse_backend_nvidia::inf::INF_NAME;
use guesthouse_backend_nvidia::startup::guest_branch_of;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const SETUP_FILE: &str = "setup/setup.json";
const NVIDIA_VENDOR: u16 = 0x10DE;
/// Where the kernel lists PCI devices, one folder each, named by address.
const PCI_DEVICES: &str = "/sys/bus/pci/devices";

/// INF files are Latin-1, and only the version line matters here.
fn guest_driver_version(driver_folder: &Path) -> Result<DriverVersion, String> {
    let path = driver_folder.join(INF_NAME);
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let text: String = bytes.iter().map(|&byte| char::from(byte)).collect();
    driver_version_from_inf(&text).ok_or_else(|| format!("{} has no driver version", path.display()))
}

fn parse_setup(text: &str) -> Result<Setup, String> {
    let setup: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let folder = setup["driver_folder"].as_str().ok_or("Setup has not unpacked the guest driver yet.")?;
    let driver_folder = PathBuf::from(folder);
    let guest_driver_version = guest_driver_version(&driver_folder)?;
    let number = |name: &str| setup["card"][name].as_u64();
    let card = number("device").zip(number("revision")).map(|(device, revision)| PciCard {
        vendor: NVIDIA_VENDOR,
        device: device as u16,
        revision: revision as u8,
    });
    // Setup writes down where its card is, as `pci`. One from before that has none.
    let card_address = setup["card"]["pci"].as_str().and_then(PciAddress::parse);
    Ok(Setup {
        guest_branch: guest_branch_of(guest_driver_version),
        guest_driver_version,
        driver_folder,
        card,
        card_address,
    })
}

/// Reads what setup found.
pub fn read_setup() -> Result<Setup, String> {
    let path = store().join(SETUP_FILE);
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("Setup has not been run ({}: {error}).", path.display()))?;
    parse_setup(&text)
}

/// What setup found, for a guest the app gave the NVIDIA card at `address`, which needn't be the card
/// setup chose. The guest's driver is setup's. The card is read from sysfs as setup reads its own, so
/// the backend opens this card, checks it found it, and tells the guest what it is.
pub fn on_card(setup: Setup, address: PciAddress) -> Result<Setup, String> {
    on_card_in(setup, address, Path::new(PCI_DEVICES))
}

fn on_card_in(setup: Setup, address: PciAddress, devices: &Path) -> Result<Setup, String> {
    let folder = devices.join(address.to_string());
    let number = |field: &str| {
        let text = fs::read_to_string(folder.join(field)).ok()?;
        u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
    };
    let gone = || {
        format!(
            "This guest was given the NVIDIA card at {address}, and there is no NVIDIA card there now. Choose \
             another card in the machine's settings."
        )
    };
    if number("vendor") != Some(u32::from(NVIDIA_VENDOR)) {
        return Err(gone());
    }
    let device = number("device").ok_or_else(gone)?;
    let revision = number("revision").unwrap_or(0);
    let card = PciCard { vendor: NVIDIA_VENDOR, device: device as u16, revision: revision as u8 };
    Ok(Setup { card: Some(card), card_address: Some(address), ..setup })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::process;

    #[test]
    fn the_version_the_guest_is_told_comes_from_the_inf() {
        let folder = env::temp_dir().join(format!("guesthouse-setup-{}", process::id()));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(INF_NAME), "[Version]\nDriverVer = 01/02/2026,32.0.16.1742\n").unwrap();
        let text = format!(
            "{{\"driver_folder\": {:?}, \"card\": {{\"device\": 11141, \"revision\": 161}}}}",
            folder.to_str().unwrap()
        );
        let setup = parse_setup(&text).unwrap();
        assert_eq!(setup.guest_driver_version.to_string(), "32.0.16.1742");
        assert_eq!(setup.guest_branch, "617");
        assert_eq!(setup.card, Some(PciCard { vendor: 0x10DE, device: 0x2B85, revision: 161 }));
        assert_eq!(setup.card_address, None, "a setup from before the address was written down");
        let text = text.replace("\"revision\": 161", "\"revision\": 161, \"pci\": \"0000:02:00.0\"");
        let setup = parse_setup(&text).unwrap();
        assert_eq!(setup.card_address.map(|address| address.to_string()).as_deref(), Some("0000:02:00.0"));
        fs::remove_dir_all(folder).unwrap();
    }

    fn setup_on_first_card() -> Setup {
        Setup {
            guest_driver_version: guesthouse_backend::DriverVersion { parts: [32, 0, 16, 1742] },
            guest_branch: "617".to_owned(),
            driver_folder: PathBuf::from("/driver"),
            card: Some(PciCard { vendor: NVIDIA_VENDOR, device: 0x2704, revision: 0xA1 }),
            card_address: PciAddress::parse("0000:01:00.0"),
        }
    }

    fn fake_device(devices: &Path, address: &str, vendor: &str, device: &str) {
        let folder = devices.join(address);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("vendor"), format!("{vendor}\n")).unwrap();
        fs::write(folder.join("device"), format!("{device}\n")).unwrap();
        fs::write(folder.join("revision"), "0xa1\n").unwrap();
    }

    #[test]
    fn a_guest_given_the_second_card_opens_it_and_tells_its_driver_that_card() {
        let devices = env::temp_dir().join(format!("guesthouse-devices-{}", process::id()));
        fake_device(&devices, "0000:01:00.0", "0x10de", "0x2704");
        fake_device(&devices, "0000:02:00.0", "0x10de", "0x21c4");
        let second = PciAddress::parse("0000:02:00.0").unwrap();
        let setup = on_card_in(setup_on_first_card(), second, &devices).unwrap();
        assert_eq!(setup.card_address, Some(second));
        assert_eq!(setup.card, Some(PciCard { vendor: 0x10DE, device: 0x21C4, revision: 0xA1 }));
        assert_eq!((setup.guest_branch.as_str(), setup.driver_folder), ("617", PathBuf::from("/driver")));
        fs::remove_dir_all(devices).unwrap();
    }

    #[test]
    fn a_card_that_is_gone_or_not_nvidias_is_refused_by_its_address() {
        let devices = env::temp_dir().join(format!("guesthouse-gone-{}", process::id()));
        fake_device(&devices, "0000:03:00.0", "0x1002", "0x744c");
        for address in ["0000:02:00.0", "0000:03:00.0"] {
            let card = PciAddress::parse(address).unwrap();
            let refused = on_card_in(setup_on_first_card(), card, &devices).unwrap_err();
            assert!(refused.contains(&format!("the NVIDIA card at {address}, and there is no")), "{refused}");
            assert!(refused.contains("Choose another card"), "{refused}");
        }
        fs::remove_dir_all(devices).unwrap();
    }

    #[test]
    fn no_driver_yet_is_said_plainly() {
        assert!(parse_setup("{}").unwrap_err().contains("not unpacked"));
    }
}
