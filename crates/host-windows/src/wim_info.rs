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
}
