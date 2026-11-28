# SPDX-License-Identifier: GPL-2.0-or-later
#
# DRAFT. Not run in a guest yet: every line is unproven until it is.
#
# Installs the Guesthouse graphics driver in a Windows guest, from the folder this script is in (the
# release package, as the Mac app puts it on the stick). It takes ALREADY-SIGNED files and signs
# nothing: no key is needed or present. It
#   1. refuses a folder with a private key in it,
#   2. trusts the public certificate in the machine's Root and TrustedPublisher stores,
#   3. turns test signing on if it is not on,
#   4. installs the driver with pnputil,
#   5. says how it went on the first serial port, in the words the Mac app waits for,
#   6. restarts the machine if asked to (-Restart).
# It exits 0 when the driver is in (3010, which asks for a restart, counts), and otherwise with
# Windows' own error code, which the Mac app shows as "Windows turned the driver away with error".
param([switch]$Restart)

$Here = (Split-Path -Parent $MyInvocation.MyCommand.Path).TrimEnd('\')
$Certificate = Join-Path $Here 'ruxn-driver.cer'
$Inf = Join-Path $Here 'ruxnwddm-full.inf'
$RebootNeeded = 3010
$NotTrusted = 0x800b0109
$PrivateKey = 0xe0000001
$NoCertificate = 0xe0000002

# What the Mac app waits for on the serial port (mac/host/src/install/windows.rs, `marker`).
function Say([string]$Text) {
	Write-Information $Text -InformationAction Continue
	try {
		$port = New-Object System.IO.Ports.SerialPort COM1, 115200
		$port.Open()
		$port.WriteLine($Text)
		$port.Close()
	} catch { }
}

function Stop-With([uint32]$Code) {
	Say ('guesthouse: driver failed 0x{0:x8}' -f $Code)
	exit ([int]$Code)
}

# 1. No private key, whoever put it here.
if (Get-ChildItem -Path $Here -File | Where-Object { $_.Extension -in '.pfx', '.p12', '.pvk', '.key', '.snk' }) {
	Stop-With $PrivateKey
}
if (-not (Test-Path $Certificate)) { Stop-With $NoCertificate }

# 2. Trust the public certificate, in the two stores the driver's signature is checked against.
foreach ($store in 'Root', 'TrustedPublisher') {
	Import-Certificate -FilePath $Certificate -CertStoreLocation "Cert:\LocalMachine\$store" | Out-Null
}

# The catalog must now check out, or Windows would turn the driver away; say so before it does.
foreach ($name in 'ruxnwddm-full.cat', 'ruxnwddm.sys') {
	$signature = Get-AuthenticodeSignature (Join-Path $Here $name)
	if ($signature.Status -ne 'Valid') { Stop-With $NotTrusted }
}

# 3. Test signing, which the certificate needs to load a kernel driver in this guest. It takes
# effect at the next start, which is the restart that follows.
$current = (bcdedit /enum '{current}') -join "`n"
if ($current -notmatch 'testsigning\s+Yes') {
	bcdedit /set testsigning on | Out-Null
	if ($LASTEXITCODE -ne 0) { Stop-With ([uint32]$LASTEXITCODE) }
}

# 4. The driver.
pnputil /add-driver $Inf /install
$code = $LASTEXITCODE
if ($code -ne 0 -and $code -ne $RebootNeeded) { Stop-With ([uint32]$code) }

# 5 and 6.
Say 'guesthouse: driver installed'
if ($Restart) { Restart-Computer -Force }
exit 0
