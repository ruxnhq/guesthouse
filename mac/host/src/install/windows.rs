// SPDX-License-Identifier: GPL-2.0-or-later
//! Windows 11 for ARM: where the installer comes from, the answers its setup is given, and what
//! the first logon runs.
//!
//! **The installer is chosen by the person.** Microsoft's download page for Windows 11 on ARM makes
//! its links in a browser session, for a day, behind a check that a person is there. The other
//! hosts (`docs/windows-host.md` of the public repo, the Linux app) take a Windows ISO the person
//! has, and so does this: Guesthouse opens Microsoft's own page and the person saves the file
//! (`INSTALLER_PAGE`). Automating the page would mean answering its check as a browser, which is
//! not done.

use super::driver;
use guesthouse_account_core::common::{MOST_WINDOWS_NAME_CHARS, WINDOWS_RESERVED};
use guesthouse_answers_core::linux_host::editions::edition_key;
use guesthouse_answers_core::windows_host::escape;
use guesthouse_basics::text::base64_encode;
use std::path::{Path, PathBuf};

/// Microsoft's own page for the ARM installer, which the app opens for the person.
pub const INSTALLER_PAGE: &str = "https://www.microsoft.com/software-download/windows11arm64";

/// Smaller than this is not a Windows installer, whatever it is called.
const MIN_INSTALLER_BYTES: u64 = 2_000_000_000;

/// A file the person chose, checked for what can be checked without opening it.
pub fn check_installer(path: &Path) -> Result<PathBuf, String> {
    if !path.is_file() {
        return Err(format!("{} is not there.", path.display()));
    }
    let is_iso = path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("iso"));
    if !is_iso {
        return Err("Choose the Windows 11 installer, a file ending in .iso.".into());
    }
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size < MIN_INSTALLER_BYTES {
        return Err("That file is too small to be a Windows installer.".into());
    }
    Ok(path.to_path_buf())
}

/// The guest's account, named after the Mac's user: letters, digits and dashes of the short name,
/// as many as Windows takes, and never one it keeps for itself.
pub fn account_name(mac_user: &str) -> String {
    let mut name: String = mac_user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(MOST_WINDOWS_NAME_CHARS)
        .collect();
    if name.is_empty() || !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        name = format!("user{name}").chars().take(MOST_WINDOWS_NAME_CHARS).collect();
    }
    if WINDOWS_RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
        name.push('1');
    }
    name
}

/// The Mac's user, by its short name.
pub fn mac_user() -> String {
    std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_default()
}

/// What the guest says on its serial port, which the host reads from the machine's `serial.log`.
pub mod marker {
    /// Every line the guest's own scripts say begins so; the firmware's lines do not.
    pub const GUEST_SAYS: &str = "guesthouse: ";
    pub const SYSTEM_INSTALLED: &str = "guesthouse: system installed";
    pub const DRIVER_INSTALLED: &str = "guesthouse: driver installed";
    /// Followed by the code Windows gave.
    pub const DRIVER_FAILED: &str = "guesthouse: driver failed";
    pub const FIRST_START: &str = "guesthouse: first start ok";
}

/// The label of the small disk the driver's files are on.
pub const STICK_LABEL: &str = "RUXN";

/// What the answer file is made from.
pub struct Answers<'a> {
    /// The account's name, from `account_name`.
    pub account: &'a str,
    /// The account's password, from `password::check`: letters, digits and dashes, so it needs no
    /// quoting where it is written.
    pub password: &'a str,
    pub computer_name: &'a str,
    /// The edition to install, by its name in the installer ("Windows 11 Pro").
    pub edition: &'a str,
    /// The language Setup shows its own pages in and the system is installed in, when it is
    /// known to be on the installer (`en-US`). With none, the installer's own default stands.
    pub ui_language: Option<&'a str>,
}

/// The locale Setup and the installed system start with: the same as the Linux host's answer file
/// gives, which never leaves a language page or a keyboard page to be answered by hand.
const LOCALE: &str = "en-US";

/// The component that settles Setup's language and keyboard pages, as the shared answer file for
/// the Linux host has it. Its UI language is the one thing left to the installer when none is known:
/// naming one the installer does not carry makes Setup stop, and a single-language installer
/// shows no language page.
fn international_core(ui_language: Option<&str>) -> String {
    let ui = ui_language.map_or(String::new(), |language| {
        let language = escape(language);
        format!("<SetupUILanguage><UILanguage>{language}</UILanguage></SetupUILanguage><UILanguage>{language}</UILanguage>")
    });
    format!(
        r#"<component name="Microsoft-Windows-International-Core-WinPE" processorArchitecture="arm64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      {ui}<InputLocale>{LOCALE}</InputLocale><SystemLocale>{LOCALE}</SystemLocale><UserLocale>{LOCALE}</UserLocale>
    </component>
    "#
    )
}

/// Setup's `<ProductKey>`: the generic key Microsoft publishes for the edition, from the shared
/// table (`edition_key`). It only tells Setup which edition to install, so the product key page
/// is not shown; Windows stays unactivated. An edition the table does not have gets none.
fn product_key(edition: &str) -> String {
    edition_key(edition).map_or(String::new(), |key| {
        format!("<ProductKey><Key>{key}</Key><WillShowUI>OnError</WillShowUI></ProductKey>")
    })
}

fn say(text: &str) -> String {
    format!(
        "$p = New-Object System.IO.Ports.SerialPort COM1,115200; $p.Open(); $p.WriteLine('{text}'); $p.Close()"
    )
}

/// Standard base64 of the text as UTF-16, for PowerShell's `-EncodedCommand`, so no quoting reaches
/// the command line.
fn base64_utf16(text: &str) -> String {
    let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64_encode(&bytes)
}

/// What the first logon runs: test signing, the driver from the stick, and what happened said on
/// the serial port. A reboot follows, which is the first start under the driver.
fn first_logon_script() -> String {
    format!(
        "{installed}; \
         $d = (Get-Volume -FileSystemLabel {label}).DriveLetter; \
         if (-not $d) {{ {no_stick}; exit 1 }}; \
         certutil -addstore -f Root \"${{d}}:\\{cer}\"; \
         certutil -addstore -f TrustedPublisher \"${{d}}:\\{cer}\"; \
         bcdedit /set testsigning on; \
         & powershell -NoProfile -ExecutionPolicy Bypass -File \"${{d}}:\\{script}\"; \
         if ($LASTEXITCODE -eq 0) {{ {ok} }} else {{ $c = '{{0:x8}}' -f $LASTEXITCODE; $p = New-Object System.IO.Ports.SerialPort COM1,115200; $p.Open(); $p.WriteLine('{failed} 0x' + $c); $p.Close() }}; \
         Set-ItemProperty -Path 'HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\RunOnce' -Name GuesthouseFirstStart -Value 'powershell -NoProfile -EncodedCommand {first}'; \
         shutdown /r /t 5",
        installed = say(marker::SYSTEM_INSTALLED),
        label = STICK_LABEL,
        cer = driver::CERTIFICATE,
        script = driver::SCRIPT,
        no_stick = say(&format!("{} 0xstick", marker::DRIVER_FAILED)),
        ok = say(marker::DRIVER_INSTALLED),
        failed = marker::DRIVER_FAILED,
        first = base64_utf16(&say(marker::FIRST_START)),
    )
}

/// The answer file Windows setup reads from the disc it is given: the disk laid out for UEFI, the
/// checks for a TPM and Secure Boot waived (the machine has neither), one administrator, with the password it was given,
/// who signs in by itself, and the first logon's commands.
pub fn autounattend(answers: &Answers) -> String {
    let account = escape(answers.account);
    let script = escape(&first_logon_script());
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend">
  <settings pass="windowsPE">
    {international}<component name="Microsoft-Windows-Setup" processorArchitecture="arm64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <RunSynchronous>
        <RunSynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>1</Order>
          <Path>reg add HKLM\SYSTEM\Setup\LabConfig /v BypassTPMCheck /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>2</Order>
          <Path>reg add HKLM\SYSTEM\Setup\LabConfig /v BypassSecureBootCheck /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>3</Order>
          <Path>reg add HKLM\SYSTEM\Setup\LabConfig /v BypassRAMCheck /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>
      </RunSynchronous>
      <DiskConfiguration>
        <Disk wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <DiskID>0</DiskID>
          <WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State"><Order>1</Order><Type>EFI</Type><Size>260</Size></CreatePartition>
            <CreatePartition wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State"><Order>2</Order><Type>MSR</Type><Size>16</Size></CreatePartition>
            <CreatePartition wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State"><Order>3</Order><Type>Primary</Type><Extend>true</Extend></CreatePartition>
          </CreatePartitions>
        </Disk>
      </DiskConfiguration>
      <ImageInstall>
        <OSImage>
          <InstallTo><DiskID>0</DiskID><PartitionID>3</PartitionID></InstallTo>
          <InstallFrom><MetaData wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State"><Key>/IMAGE/NAME</Key><Value>{edition}</Value></MetaData></InstallFrom>
        </OSImage>
      </ImageInstall>
      <UserData>{key}<AcceptEula>true</AcceptEula></UserData>
    </component>
  </settings>
  <settings pass="specialize">
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="arm64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <ComputerName>{computer}</ComputerName>
    </component>
  </settings>
  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="arm64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <OOBE>
        <HideEULAPage>true</HideEULAPage>
        <HideOEMRegistrationScreen>true</HideOEMRegistrationScreen>
        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>
        <HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>
        <ProtectYourPC>3</ProtectYourPC>
      </OOBE>
      <UserAccounts>
        <LocalAccounts>
          <LocalAccount wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
            <Name>{account}</Name>
            <Group>Administrators</Group>
            <Password><Value>{password}</Value><PlainText>true</PlainText></Password>
          </LocalAccount>
        </LocalAccounts>
      </UserAccounts>
      <AutoLogon>
        <Enabled>true</Enabled>
        <LogonCount>3</LogonCount>
        <Username>{account}</Username>
        <Password><Value>{password}</Value><PlainText>true</PlainText></Password>
      </AutoLogon>
      <FirstLogonCommands>
        <SynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>1</Order>
          <CommandLine>powershell -NoProfile -ExecutionPolicy Bypass -Command "{script}"</CommandLine>
        </SynchronousCommand>
      </FirstLogonCommands>
    </component>
  </settings>
</unattend>
"#,
        international = international_core(answers.ui_language),
        key = product_key(answers.edition),
        password = escape(answers.password),
        edition = escape(answers.edition),
        computer = escape(answers.computer_name),
    )
}

#[cfg(test)]
mod tests;
