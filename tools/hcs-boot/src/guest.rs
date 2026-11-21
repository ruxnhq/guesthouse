//! The files `prepare` puts on a new guest's disk: an answer file that takes Windows past its first
//! questions with a local account, the script Windows runs when it has set itself up, and the report
//! that script schedules.

/// Run as SYSTEM at every start, it writes what the guest sees of its GPU to COM1.
pub const REPORT: &str = include_str!("../guest/report.ps1");
/// Where the report goes, from the root of the guest's Windows volume. `SetupComplete.cmd` names it
/// as `C:\Guesthouse\report.ps1`.
pub const REPORT_PATH: &str = r"Guesthouse\report.ps1";
pub const SETUP_COMPLETE: &str = include_str!("../guest/SetupComplete.cmd");
/// Windows runs this once its own setup is done.
pub const SETUP_COMPLETE_PATH: &str = r"Windows\Setup\Scripts\SetupComplete.cmd";
/// Windows reads the answer file here on the first start of an image applied with no Setup.
pub const ANSWER_FILE_PATH: &str = r"Windows\Panther\unattend.xml";

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

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_file_makes_the_named_administrator() {
        let file = answer_file("guest", "p&ss<word>");
        assert!(file.contains("<Name>guest</Name>"));
        assert!(file.contains("<Value>p&amp;ss&lt;word&gt;</Value>"));
        assert!(file.contains("<Group>Administrators</Group>"));
    }

    #[test]
    fn the_scripts_agree_on_where_the_report_is() {
        assert!(SETUP_COMPLETE.contains(&format!(r"C:\{REPORT_PATH}")));
        assert!(REPORT.contains("COM1"));
    }
}
