# Releases the Windows host, on Windows, from a clean checkout of main whose windows\version.txt
# has been raised to the new version:
#   powershell -ExecutionPolicy Bypass -File scripts\windows-release.ps1 [-DryRun]
# 1. checks the tree is clean, and that neither the tag windows-vVERSION nor the installer of this
#    version is published yet, as a published file never changes;
# 2. builds the app, then the installer with scripts\windows-setup.ps1, into
#    target\windows-release\VERSION\, and checks the installer against its .sha256;
# 3. writes guesthouse-windows-latest.json beside them, which the app reads to offer the update;
#    It also gets the source of the WinFsp the installer carries, which windows\winfsp.json pins, and
#    checks it against the pin: WinFsp is under the GPLv3, which asks whoever passes on its installer
#    to offer its source as well;
# 4. tags the commit windows-vVERSION, pushes the tag, and makes a GitHub release with the installer,
#    its .sha256 and WinFsp's source, which isn't marked latest, as that's the Linux host's;
# 5. publishes WinFsp's source unless it's there already, then the installer, its .sha256 and, last of
#    all, guesthouse-windows-latest.json to the file drop (the R2 bucket ruxn-files, at
#    pub/guesthouse/, served as files.ruxn.com/p/guesthouse/) with the cf CLI, and downloads the
#    installer back to check it.
# -DryRun does 1 to 3 and publishes nothing. RELEASING.md says what comes before and after.
# -BuildOnly is -DryRun for release-windows.yml, which runs from the tag once it's pushed: the tag being
# on GitHub already is what it expects, so only that check is left out.
# It needs git, cargo, gh and cf on PATH, each signed in.
param([switch]$DryRun, [switch]$BuildOnly)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
Set-Location $root

$base = 'https://files.ruxn.com/p/guesthouse'
$bucket = 'ruxn-files'
$prefix = 'pub/guesthouse'
$repo = 'ruxnlabs/guesthouse'

# Runs a native program and stops if it fails.
function Invoke-Native([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed ($LASTEXITCODE)" }
}

function Get-Sha256([string]$Path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant() }

# The curl Windows has. Windows PowerShell's own web requests drop the connection on a large download.
$curl = Join-Path $env:SystemRoot 'System32\curl.exe'

# Whether a file is published already at the file drop.
function Test-Published([string]$Name) {
    & $curl --head --fail --silent --proto =https --output NUL "$base/$Name"
    return $LASTEXITCODE -eq 0
}

$version = (Get-Content -Raw -LiteralPath 'windows\version.txt').Trim()
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "windows\version.txt says '$version', which isn't three numbers." }
$tag = "windows-v$version"
$installer = "guesthouse-setup-$version.exe"
Write-Output "Releasing the Windows host $version, as $tag."

# 1. Nothing uncommitted, and nothing of this version out already.
$dirty = & git status --porcelain
if ($dirty) { throw "The tree has uncommitted changes. A release is built from a commit:`n$($dirty -join "`n")" }
$commit = (& git rev-parse HEAD).Trim()
if (-not $BuildOnly -and (& git ls-remote --tags origin "refs/tags/$tag")) { throw "The tag $tag is on GitHub already. Raise windows\version.txt." }
if (Test-Published $installer) { throw "$base/$installer is published already, and a published file never changes. Raise windows\version.txt." }

# 2. The app, then the installer with it.
$out = Join-Path $root "target\windows-release\$version"
New-Item -ItemType Directory -Force $out | Out-Null
Invoke-Native 'the app build' { cargo build --release --locked -p guesthouse-app-windows }
Invoke-Native 'the installer build' {
    powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'windows-setup.ps1') -Version $version -Out $out
}
$setup = Join-Path $out $installer
$sums = "$setup.sha256"
$sha = Get-Sha256 $setup
$said = ((Get-Content -Raw -LiteralPath $sums) -split '\s+')[0]
if ($said -ne $sha) { throw "$sums says $said, and the installer's SHA-256 is $sha." }

# WinFsp's source, as the pin names it, beside the installer.
$winfsp = (Get-Content -Raw -LiteralPath 'windows\winfsp.json' | ConvertFrom-Json).source
$winfspSource = Join-Path $out $winfsp.file
Invoke-Native "downloading WinFsp's source" {
    & $curl --fail --silent --location --proto =https --output $winfspSource $winfsp.url
}
if ((Get-Sha256 $winfspSource) -ne $winfsp.sha256.ToLowerInvariant()) {
    throw "$($winfsp.url) isn't the WinFsp source windows\winfsp.json pins ($($winfsp.sha256))."
}

# 3. What the app reads to offer the update.
$latest = Join-Path $out 'guesthouse-windows-latest.json'
[ordered]@{
    version   = $version
    installer = "$base/$installer"
    sha256    = $sha
    notes     = "https://github.com/$repo/releases/tag/$tag"
} | ConvertTo-Json | Set-Content -LiteralPath $latest -Encoding ascii
Write-Output "Built $setup ($sha) from $commit."

if ($DryRun -or $BuildOnly) {
    Write-Output "Nothing was tagged or published. The files are in $out."
    exit 0
}

# 4. The tag and the GitHub release, which isn't the latest: that's the Linux host's.
Invoke-Native 'the tag' { git tag -a $tag -m "Windows host $version" $commit }
Invoke-Native 'pushing the tag' { git push origin $tag }
Invoke-Native 'the GitHub release' {
    gh release create $tag $setup $sums $winfspSource --repo $repo --verify-tag --latest=false --title "Windows $version" `
        --notes "The Windows host's installer, $installer. Its SHA-256 is $sha. It's also at $base/$installer. It carries WinFsp, under the GPLv3, whose source is $($winfsp.file), here and at $base/$($winfsp.file)."
}

# 5. The file drop: WinFsp's source first, once, as a published file never changes; the latest file
# last, so the app never offers an installer that isn't there.
if (-not (Test-Published $winfsp.file)) {
    Invoke-Native "publishing $($winfsp.file)" {
        cf r2 objects put "$prefix/$($winfsp.file)" --bucket-name $bucket --file $winfspSource --content-type application/gzip
    }
}
foreach ($file in @($setup, $sums, $latest)) {
    $name = Split-Path -Leaf $file
    Invoke-Native "publishing $name" {
        cf r2 objects put "$prefix/$name" --bucket-name $bucket --file $file --content-type application/octet-stream
    }
}
$back = Join-Path $out 'downloaded.exe'
Invoke-Native 'downloading the installer back' { & $curl --fail --silent --location --proto =https --output $back "$base/$installer" }
if ((Get-Sha256 $back) -ne $sha) { throw "The installer downloaded from $base/$installer isn't the one built." }
Remove-Item -LiteralPath $back

Write-Output "Released the Windows host ${version}: $base/$installer"
Write-Output "Next, in a pull request: the Windows version in site\public\index.html, then cf deploy public from site."
