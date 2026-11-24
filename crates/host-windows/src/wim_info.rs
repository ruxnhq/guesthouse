//! Reads DISM's list of the editions in a Windows image, to find the one asked for by name.

/// The editions DISM's `/Get-WimInfo` lists, as index and name, from its English output.
pub fn editions(listing: &str) -> Vec<(u32, String)> {
    let mut found = Vec::new();
    let mut index = None;
    for line in listing.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        match key.trim() {
            "Index" => index = value.trim().parse().ok(),
            "Name" => {
                if let Some(number) = index.take() {
                    found.push((number, value.trim().to_owned()));
                }
            }
            _ => {}
        }
    }
    found
}

/// The index of the edition with this name, ignoring case.
pub fn index_of(listing: &str, edition: &str) -> Option<u32> {
    editions(listing).into_iter().find(|(_, name)| name.eq_ignore_ascii_case(edition)).map(|(index, _)| index)
}

/// The edition a machine is given when none is named.
pub const DEFAULT_EDITION: &str = "Windows 11 Pro";

/// The edition to apply, as index and name: the one named, whatever its case, or with no name Pro
/// if the image has it and its first edition if not, so an image of Home alone still works.
pub fn choose(listing: &str, edition: &str) -> Result<(u32, String), String> {
    let editions = editions(listing);
    let wanted = edition.trim();
    let found = if wanted.is_empty() {
        let pro = editions.iter().find(|(_, name)| name.eq_ignore_ascii_case(DEFAULT_EDITION));
        pro.or(editions.first()).cloned()
    } else {
        editions.iter().find(|(_, name)| name.eq_ignore_ascii_case(wanted)).cloned()
    };
    found.ok_or_else(|| {
        let names: Vec<&str> = editions.iter().map(|(_, name)| name.as_str()).collect();
        if names.is_empty() {
            "DISM listed no editions in the image.".to_owned()
        } else {
            format!("The image has no {wanted}. It has: {}. Pick one of them.", names.join(", "))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "\
Deployment Image Servicing and Management tool
Version: 10.0.26100.1

Details for image : E:\\sources\\install.wim

Index : 1
Name : Windows 11 Home
Description : Windows 11 Home
Size : 19,000,000,000 bytes

Index : 6
Name : Windows 11 Pro
Description : Windows 11 Pro
Size : 19,500,000,000 bytes

The operation completed successfully.
";

    #[test]
    fn each_edition_is_read_with_its_index() {
        assert_eq!(editions(LISTING), [(1, "Windows 11 Home".to_owned()), (6, "Windows 11 Pro".to_owned())]);
    }

    #[test]
    fn an_edition_is_found_by_name_whatever_its_case() {
        assert_eq!(index_of(LISTING, "windows 11 pro"), Some(6));
        assert_eq!(index_of(LISTING, "Windows 11 Education"), None);
    }

    #[test]
    fn with_no_edition_named_pro_is_chosen_and_else_the_first() {
        assert_eq!(choose(LISTING, "").unwrap(), (6, "Windows 11 Pro".to_owned()));
        assert_eq!(choose(LISTING, " windows 11 home ").unwrap(), (1, "Windows 11 Home".to_owned()));
        let home_only = LISTING.replace("Windows 11 Pro", "Windows 11 Home Single Language");
        assert_eq!(choose(&home_only, "").unwrap(), (1, "Windows 11 Home".to_owned()));
        let refused = choose(LISTING, "Windows 11 Education").unwrap_err();
        assert!(refused.contains("Windows 11 Home, Windows 11 Pro"), "{refused}");
    }
}
