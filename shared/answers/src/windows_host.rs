//! The Windows host's answer-file pieces, as they were in `host-windows`:
//!
//! - [`answer_file`], a Windows guest's `unattend.xml` for the oobeSystem pass, on an image applied
//!   offline with DISM, so no partitions, edition, key or time zone;
//! - [`escape`], its XML escape, which leaves the apostrophe as it is;
//! - [`ubuntu`], the line an Ubuntu guest's `install.sh` makes its administrator from.
//!
//! An Ubuntu guest's `user-data` and `meta-data` are files of the host's own that go on its files
//! disk as they are, so they stay in `host-windows` with the guest's scripts.

pub mod ubuntu;

const COMPONENT: &str = r#"processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS""#;

/// An answer file for the oobeSystem pass: English (US), every first-start question skipped, and one
/// local administrator.
pub fn answer_file(user: &str, password: &str) -> String {
    let (user, password) = (escape(user), escape(password));
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-International-Core" {COMPONENT}>
      <InputLocale>en-US</InputLocale><SystemLocale>en-US</SystemLocale><UILanguage>en-US</UILanguage><UserLocale>en-US</UserLocale>
    </component>
    <component name="Microsoft-Windows-Shell-Setup" {COMPONENT}>
      <OOBE>
        <HideEULAPage>true</HideEULAPage>
        <HideLocalAccountScreen>true</HideLocalAccountScreen>
        <HideOEMRegistrationScreen>true</HideOEMRegistrationScreen>
        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>
        <HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>
        <ProtectYourPC>3</ProtectYourPC>
      </OOBE>
      <UserAccounts>
        <LocalAccounts>
          <LocalAccount wcm:action="add">
            <Name>{user}</Name>
            <Group>Administrators</Group>
            <Password><Value>{password}</Value><PlainText>true</PlainText></Password>
          </LocalAccount>
        </LocalAccounts>
      </UserAccounts>
    </component>
  </settings>
</unattend>
"#
    )
}

/// The text as it goes inside an XML element or attribute's double quotes. The apostrophe stays as
/// it is.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
#[path = "windows_host/tests.rs"]
mod tests;
