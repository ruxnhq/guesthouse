//! The card's name as AMD gives it, from libdrm's list of AMD cards, the one Linux names them by.
//! Windows shows it in Device Manager, and programs that ask the adapter what it is get it too.
//! Without it they saw "AMD GPU 0x1114". The list is the host's, so a card newer than this program
//! is named as soon as the host's libdrm knows it.

use std::fs;

/// Where distributions keep libdrm's list, and NixOS's, which its Guesthouse module links into the
/// system's folders.
const NAME_LISTS: [&str; 2] =
    ["/usr/share/libdrm/amdgpu.ids", "/run/current-system/sw/share/libdrm/amdgpu.ids"];

/// The name for a card, by its device and PCI revision, if this PC's list has one.
pub fn card_name(device: u16, revision: u8) -> Option<String> {
    let list = NAME_LISTS.iter().find_map(|path| fs::read_to_string(path).ok())?;
    name_in(&list, device, revision)
}

/// One line of the list is `device, revision, name` in hexadecimal, as `1114, C2, AMD Radeon 860M
/// Graphics`. A revision missing from the list still has a name if the device has only one: the same
/// chip is sold under more names by revision, and a guess could give it the wrong one.
fn name_in(list: &str, device: u16, revision: u8) -> Option<String> {
    let entries: Vec<(u8, &str)> = list
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.splitn(3, ',');
            let listed = u16::from_str_radix(fields.next()?.trim(), 16).ok()?;
            let revision = u8::from_str_radix(fields.next()?.trim(), 16).ok()?;
            let name = fields.next()?.trim();
            (listed == device && !name.is_empty()).then_some((revision, name))
        })
        .collect();
    if let Some((_, name)) = entries.iter().find(|(listed, _)| *listed == revision) {
        return Some((*name).to_owned());
    }
    let first = entries.first()?.1;
    entries.iter().all(|(_, name)| *name == first).then(|| first.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "# List of AMDGPU IDs\n#\n1.0.0\n\
        1114,\tC2,\tAMD Radeon 860M Graphics\n\
        1114,\tC3,\tAMD Radeon 840M Graphics\n\
        1114,\tD2,\tAMD Radeon 860M Graphics\n\
        150E,\tC1,\tAMD Radeon 890M Graphics\n\
        150E,\tD1,\tAMD Radeon 890M Graphics\n";

    #[test]
    fn a_card_is_named_by_its_device_and_revision() {
        assert_eq!(name_in(LIST, 0x1114, 0xC2).as_deref(), Some("AMD Radeon 860M Graphics"));
        assert_eq!(name_in(LIST, 0x1114, 0xC3).as_deref(), Some("AMD Radeon 840M Graphics"));
    }

    #[test]
    fn an_unlisted_revision_is_named_only_when_the_device_has_one_name() {
        assert_eq!(name_in(LIST, 0x150E, 0xE5).as_deref(), Some("AMD Radeon 890M Graphics"));
        assert_eq!(name_in(LIST, 0x1114, 0xE5), None, "860M or 840M, so no guess");
        assert_eq!(name_in(LIST, 0x7FFF, 0xC0), None);
    }
}
