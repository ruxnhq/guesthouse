//! shared/remote-view/guest/screen.ps1's choice of whether remoteman's installer runs again, and what
//! it's given when it does. The script works in C:\Guesthouse\screen and calls Windows' services, so
//! these load only its functions for that choice from the real file and run them on a folder that
//! stands in for the guest's screen folder, as the host fills it.

use crate::{Said, Scratch, quote, run, shell};
use std::fs;
use std::path::{Path, PathBuf};

const VIEWER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PAIRED: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The functions the choice is made with.
const FUNCTIONS: [&str; 6] = [
    "Get-InstallNote",
    "Test-NoteChanged",
    "Get-SetupArguments",
    "Test-NamesDevice",
    "Get-ScreenHosts",
    "Limit-ScreenRules",
];
/// How the script before viewer.txt made its note, word for word, with its trust list as `$trusted`.
const NOTE_BEFORE: &str = "((Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash, (Get-FileHash -LiteralPath $trusted -Algorithm SHA256).Hash, $encoder) -join ' '";

fn screen_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shared/remote-view/guest/screen.ps1")
}

/// A guest's screen folder as the host writes it before a boot.
struct Folder {
    shell: &'static str,
    path: PathBuf,
    _scratch: Scratch,
}

impl Folder {
    fn new(shell: &'static str) -> Self {
        let scratch = Scratch::new();
        let path = scratch.path("screen");
        fs::create_dir_all(&path).unwrap();
        let folder = Self { shell, path, _scratch: scratch };
        folder.write("remoteman-host-setup.exe", "remoteman 0.1.9");
        folder.write("encoder.txt", "auto");
        // As the host writes them: the viewer's line alone, and the guest's whole list.
        folder.write("viewer.txt", &format!("{VIEWER} Guesthouse\n"));
        folder.write("trusted", &format!("{VIEWER} Guesthouse\n"));
        folder
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.path.join(name), text).unwrap();
    }

    /// Runs `command` with the script's functions loaded and `$folder` set to this folder.
    fn run(&self, command: &str) -> Said {
        let names = FUNCTIONS.map(|name| format!("'{name}'")).join(", ");
        run(
            self.shell,
            &format!(
                "$ErrorActionPreference = 'Stop'
                $script = [System.Management.Automation.Language.Parser]::ParseFile({}, [ref]$null, [ref]$null)
                foreach ($name in {names}) {{
                    $found = $script.Find({{ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }}, $true)
                    . ([scriptblock]::Create($found.Extent.Text))
                }}
                $folder = {}
                {command}",
                quote(&screen_script()),
                quote(&self.path),
            ),
        )
    }

    /// Notes an install as the script does once the installer has run.
    fn install(&self) {
        let said = self.run(
            "Set-Content -LiteralPath (Join-Path $folder 'installed.txt') -Value (Get-InstallNote $folder) -Encoding ascii",
        );
        assert_eq!(said.code, Some(0), "{said}");
    }

    /// Whether the script would run the installer, with its service there.
    fn installs(&self) -> bool {
        let said = self.run("Write-Output \"changed $(Test-NoteChanged $folder (Get-InstallNote $folder))\"");
        match said.last() {
            "changed True" => true,
            "changed False" => false,
            _ => panic!("no answer{said}"),
        }
    }
}

#[test]
fn nothing_changed_installs_nothing() {
    let Some(shell) = shell() else { return };
    let folder = Folder::new(shell);
    assert!(folder.installs(), "never installed");
    folder.install();
    assert!(!folder.installs());
}

/// A guest installed when the trust list was the viewer's line alone, and its note made of it, isn't
/// installed again by the script that notes the viewer's line instead.
#[test]
fn a_guest_installed_before_keeps_its_note() {
    let Some(shell) = shell() else { return };
    let folder = Folder::new(shell);
    let said = folder.run(&format!(
        "$setup = Join-Path $folder 'remoteman-host-setup.exe'
        $trusted = Join-Path $folder 'trusted'
        $encoder = (Get-Content -LiteralPath (Join-Path $folder 'encoder.txt') -Raw).Trim()
        Set-Content -LiteralPath (Join-Path $folder 'installed.txt') -Value ({NOTE_BEFORE}) -Encoding ascii"
    ));
    assert_eq!(said.code, Some(0), "{said}");
    assert!(!folder.installs());
}

#[test]
fn a_device_paired_since_installs_nothing() {
    let Some(shell) = shell() else { return };
    let folder = Folder::new(shell);
    folder.install();
    folder.write("trusted", &format!("{VIEWER} Guesthouse\n{PAIRED} Kitchen tablet\n"));
    assert!(!folder.installs());
}

#[test]
fn another_installer_viewer_or_encoder_installs_again() {
    let Some(shell) = shell() else { return };
    for (name, text) in [
        ("remoteman-host-setup.exe", "remoteman 0.2.0"),
        ("viewer.txt", &format!("{PAIRED} Guesthouse\n")),
        ("encoder.txt", "d3d12"),
    ] {
        let folder = Folder::new(shell);
        folder.install();
        folder.write(name, text);
        assert!(folder.installs(), "{name}");
    }
}

/// The installer replaces the guest's whole list, so it's given the host's merged list, trusted, and
/// never the viewer's line alone.
#[test]
fn the_installer_is_given_the_whole_list() {
    let Some(shell) = shell() else { return };
    let folder = Folder::new(shell);
    let said =
        folder.run("Get-SetupArguments $folder 'auto' 'C:\\setup.log' | ForEach-Object { Write-Output $_ }");
    let trusted = folder.path.join("trusted");
    let expected = [
        "--silent".to_string(),
        "--trust".to_string(),
        format!("\"{}\"", trusted.display()),
        "--encoder".to_string(),
        "auto".to_string(),
        "--no-pair".to_string(),
        "--log".to_string(),
        "\"C:\\setup.log\"".to_string(),
    ];
    assert_eq!(said.lines(), expected, "{said}");
}

/// Lines as remoteman 0.1.9's host writes them to its log, by tracing's own format: those that name a
/// device or a file it sent are never passed on, and the rest, which say why the host failed to
/// start, are.
#[test]
fn host_log_lines_that_name_a_device_are_left_out() {
    let Some(shell) = shell() else { return };
    let folder = Folder::new(shell);
    let at = "2026-11-27T08:38:37.123456Z  INFO remoteman_host_windows";
    let named = [
        format!(
            "{at}::session: client name=Zoë's phone screen=Some(Display {{ width: 2752 }}) decoders=[Av1] cursor_apart=true"
        ),
        format!(
            "{at}::session: this computer is locked: the device is asked for the password device=\"Zoë's phone\""
        ),
        format!(
            "{at}::session: this computer was locked: unlocked for a paired device; it locks again when the device leaves device=\"Tab\""
        ),
        format!("{at}::session: a wrong password for unlocking this computer device=\"Tab\" tries=2"),
        format!("{at}::session: a device came while sharing is off: turned away name=Kitchen tablet"),
        format!(
            "{at}::control: a device's permission changed by the owner device=\"Tab\" what=\"clipboard\" on=false"
        ),
        format!(
            "{at}::files: a file from the device file=C:\\Users\\guesthouse\\Downloads\\report.pdf bytes=1024"
        ),
        format!("{at}: a new device: asking on the desktop device=\"Tab\" code=\"123 456\""),
        format!("{at}: the owner answered device=\"Tab\" let_in=true in_time=true"),
    ];
    let kept = [
        format!("{at}: listening on 0.0.0.0:7791; this host's fingerprint is 7e96"),
        format!("{at}: pairing: never opened by itself (--no-pair); the trust list's devices get in"),
        format!("{at}::pipeline: encoder ready (NVENC) mode=Av1 width=2752 height=2064"),
        format!("{at}::mirror: the screen's duplication failed: access denied device=\"\\\\.\\DISPLAY1\""),
        "2026-11-27T08:38:38.000000Z ERROR remoteman_host_windows::pipeline: the picture stopped: no encoder"
            .to_owned(),
    ];
    let lines: Vec<String> =
        named.iter().chain(&kept).map(|line| format!("'{}'", line.replace('\'', "''"))).collect();
    let said = folder.run(&format!(
        "foreach ($line in @({})) {{ if (-not (Test-NamesDevice $line)) {{ Write-Output \"kept $line\" }} }}",
        lines.join(", ")
    ));
    assert_eq!(said.code, Some(0), "{said}");
    let passed: Vec<&str> = said.lines().into_iter().filter_map(|line| line.strip_prefix("kept ")).collect();
    assert_eq!(passed, kept.iter().map(String::as_str).collect::<Vec<_>>(), "{said}");
}

/// Windows Firewall's two rules for the screen's port, as functions that stand in for its cmdlets
/// (a function is found before a cmdlet of the same name): `$rules` holds each rule's remote
/// addresses by its name.
const FIREWALL: &str = "
    $rules = @{ 'remoteman host' = 'Any'; 'Guesthouse screen' = 'Any' }
    function Get-NetFirewallRule { [CmdletBinding()] param([string]$DisplayName) if ($rules.ContainsKey($DisplayName)) { $DisplayName } }
    function Set-NetFirewallRule { [CmdletBinding()] param([string]$DisplayName, [string[]]$RemoteAddress) $rules[$DisplayName] = ($RemoteAddress -join ' ') }
    function Show-Rules { foreach ($name in @('remoteman host', 'Guesthouse screen')) { Write-Output \"$name=$($rules[$name])\" } }
";

#[test]
fn the_screens_port_is_for_this_pcs_addresses_and_on_the_shared_network_its_gateway() {
    let Some(shell) = shell() else { return };
    let folder = Folder::new(shell);
    let hosts = |text: Option<&str>| {
        match text {
            Some(text) => folder.write("hosts.txt", text),
            None => {
                let _ = fs::remove_file(folder.path.join("hosts.txt"));
            }
        }
        let said = folder
            .run("Write-Output \"hosts $(@(Get-ScreenHosts $folder @('172.30.0.1', '0.0.0.0')) -join ',')\"");
        assert_eq!(said.code, Some(0), "{said}");
        said.last().strip_prefix("hosts").unwrap_or_default().trim().to_owned()
    };
    assert_eq!(hosts(Some("shared\n192.168.1.20\n")), "192.168.1.20,172.30.0.1");
    assert_eq!(
        hosts(Some("bridged\n192.168.1.20\n100.64.1.2\n")),
        "192.168.1.20,100.64.1.2",
        "never the router"
    );
    assert_eq!(hosts(Some("shared\n172.30.0.1\nnot an address; calc\n")), "172.30.0.1");
    assert_eq!(hosts(Some("tunnel\n192.168.1.20\n")), "", "a network it doesn't know leaves the rules");
    assert_eq!(hosts(Some("")), "");
    assert_eq!(hosts(None), "", "a host from before this leaves the rules as they are");
}

#[test]
fn both_rules_are_narrowed_and_remotemans_again_after_its_installer_opened_it() {
    let Some(shell) = shell() else { return };
    let folder = Folder::new(shell);
    let said = folder.run(&format!(
        "{FIREWALL}
        Limit-ScreenRules @() @('remoteman host', 'Guesthouse screen')
        Show-Rules
        Limit-ScreenRules @('192.168.1.20', '172.30.0.1') @('remoteman host', 'Guesthouse screen', 'not there')
        Show-Rules
        $rules['remoteman host'] = 'Any'
        Limit-ScreenRules @('192.168.1.20', '172.30.0.1') @('remoteman host', 'Guesthouse screen')
        Show-Rules"
    ));
    assert_eq!(said.code, Some(0), "{said}");
    assert_eq!(
        said.lines(),
        [
            "remoteman host=Any",
            "Guesthouse screen=Any",
            "remoteman host=192.168.1.20 172.30.0.1",
            "Guesthouse screen=192.168.1.20 172.30.0.1",
            "remoteman host=192.168.1.20 172.30.0.1",
            "Guesthouse screen=192.168.1.20 172.30.0.1"
        ],
        "{said}"
    );
}

/// Runs the script's Set-SignIn on a stand-in for Winlogon's key, with `$values` its values to begin
/// with, and with Windows taking the LSA secret or not. Nothing reads or writes the registry or the
/// LSA: the commands it calls are stood in for, and the key's name is no key at all. It says the
/// answer, the values after, one per line in order, and the secret Windows was given.
pub(crate) fn sign_in(shell: &str, values: &str, user: &str, secret: &str, takes: bool) -> Said {
    let takes = if takes { "$true" } else { "$false" };
    run(
        shell,
        &format!(
            "$ErrorActionPreference = 'Stop'
            {}
            $values = @{{ {values} }}
            $given = @{{}}
            function Get-ItemProperty {{ param($LiteralPath, $Name, $ErrorAction)
                if ($LiteralPath -ne 'stand-in') {{ throw \"asked $LiteralPath\" }}
                if ($values.ContainsKey($Name)) {{ [pscustomobject]@{{ $Name = $values[$Name] }} }} }}
            function Set-ItemProperty {{ param($LiteralPath, $Name, $Value, $Type)
                if ($LiteralPath -ne 'stand-in') {{ throw \"asked $LiteralPath\" }}
                $values[$Name] = $Value }}
            function Remove-ItemProperty {{ param($LiteralPath, $Name, $ErrorAction)
                if ($LiteralPath -ne 'stand-in') {{ throw \"asked $LiteralPath\" }}
                $values.Remove($Name) }}
            function Save-LogonSecret {{ param($Secret) $given['secret'] = $Secret; {takes} }}
            \"answer $(Set-SignIn 'stand-in' '{user}' '{secret}')\"
            foreach ($name in @($values.Keys | Sort-Object)) {{ \"$name=$($values[$name])\" }}
            \"secret $($given['secret'])\"",
            crate::functions_from(&screen_script(), &["Set-SignIn"])
                + &crate::functions_from(&crate::guest_script("logon-secret.ps1"), &["Move-LeftPassword"])
        ),
    )
}

#[test]
fn signing_in_by_itself_keeps_the_password_out_of_the_registry() {
    let Some(shell) = shell() else { return };
    let said = sign_in(shell, "", "guest", "1234abcd", true);
    assert_eq!(said.code, Some(0), "{said}");
    let computer =
        said.lines().iter().find_map(|line| line.strip_prefix("DefaultDomainName=")).unwrap_or("").to_owned();
    assert_eq!(
        said.lines(),
        [
            "answer kept".to_owned(),
            "AutoAdminLogon=1".to_owned(),
            format!("DefaultDomainName={computer}"),
            "DefaultUserName=guest".to_owned(),
            "secret 1234abcd".to_owned(),
        ],
        "{said}"
    );
}

/// A guest set up before has the password in the registry: it moves to the secret, and the rest of
/// what Windows signs in with stays as it was.
#[test]
fn a_password_left_in_the_registry_moves_to_the_secret() {
    let Some(shell) = shell() else { return };
    let said = sign_in(
        shell,
        "AutoAdminLogon = '1'; DefaultUserName = 'guest'; DefaultPassword = 'old1234'",
        "",
        "",
        true,
    );
    assert_eq!(said.code, Some(0), "{said}");
    assert_eq!(
        said.lines(),
        ["answer kept", "AutoAdminLogon=1", "DefaultUserName=guest", "secret old1234"],
        "{said}"
    );
}

/// When Windows doesn't take the secret, the password still doesn't stay in the registry, and Windows
/// isn't told to sign in by itself with no password.
#[test]
fn a_secret_windows_refuses_leaves_no_password_behind() {
    let Some(shell) = shell() else { return };
    let said = sign_in(shell, "DefaultPassword = 'old1234'", "guest", "1234abcd", false);
    assert_eq!(said.code, Some(0), "{said}");
    assert_eq!(said.lines(), ["answer refused", "secret 1234abcd"], "{said}");
}

#[test]
fn with_no_password_anywhere_nothing_changes() {
    let Some(shell) = shell() else { return };
    let said = sign_in(shell, "AutoAdminLogon = '0'", "", "", true);
    assert_eq!(said.code, Some(0), "{said}");
    assert_eq!(said.lines(), ["answer none", "AutoAdminLogon=0", "secret"], "{said}");
}
