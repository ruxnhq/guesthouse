# SPDX-License-Identifier: GPL-2.0-or-later
<#
.SYNOPSIS
Signs the Windows graphics driver with the test certificate and makes its catalog.

.DESCRIPTION
Run by .github/workflows/windows-pvg.yml on a Windows runner, in the job that has the certificate, and
nowhere else. The certificate is a PKCS#12 bundle, base64 in $env:PFX, with its password in
$env:PFX_PASSWORD. It signs ruxnwddm.sys and ruxnumd.dll in the folder with SHA-256, makes
ruxnwddm-full.cat over every file there (so after the signing, which changes those two) and signs it
with SHA-256 too, and puts the certificate's public half beside them as ruxn-driver.cer. The key
never leaves this script's temporary folder, which it removes.

The bundle may be made with AES and a SHA-256 MAC, which an older Windows or .NET does not read. If
the first read fails, openssl, which the runner has, unpacks it and makes one in the old format.

.PARAMETER Folder
The folder holding what the build made: ruxnwddm.sys, ruxnumd.dll, ruxnwddm-full.inf and the
user-mode driver's DLLs.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$Folder
)

$ErrorActionPreference = 'Stop'
$signed = 'ruxnwddm.sys', 'ruxnumd.dll'
$catalog = Join-Path $Folder 'ruxnwddm-full.cat'

function Get-Signer {
    param([string]$Path, [string]$Work)
    $flags = [Security.Cryptography.X509Certificates.X509KeyStorageFlags]::Exportable
    try {
        return [Security.Cryptography.X509Certificates.X509Certificate2]::new($Path, $env:PFX_PASSWORD, $flags)
    }
    catch {
        Write-Output 'The bundle is not read as it is; making one in the old format with openssl.'
    }
    $command = Get-Command openssl -ErrorAction SilentlyContinue
    $openssl = if ($command) { $command.Source } else { Join-Path $env:ProgramFiles 'Git\usr\bin\openssl.exe' }
    $pem = Join-Path $Work 'test.pem'
    $old = Join-Path $Work 'old-signer'
    & $openssl pkcs12 -in $Path -passin env:PFX_PASSWORD -nodes -out $pem
    if ($LASTEXITCODE -ne 0) { throw 'openssl could not read the bundle.' }
    & $openssl pkcs12 -export -in $pem -out $old -passout env:PFX_PASSWORD -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1
    if ($LASTEXITCODE -ne 0) { throw 'openssl could not make the old-format bundle.' }
    return [Security.Cryptography.X509Certificates.X509Certificate2]::new($old, $env:PFX_PASSWORD, $flags)
}

# A self-signed certificate that is not trusted on the runner gives UnknownError, with the signature
# in the file all the same. Anything else is a failure.
function Assert-Signed {
    param($Result, [string]$What)
    if ($Result.Status -ne 'Valid' -and $Result.Status -ne 'UnknownError') {
        throw "$What was not signed: $($Result.StatusMessage)"
    }
}

$work = Join-Path ([IO.Path]::GetTempPath()) ('sign-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    $pfx = Join-Path $work 'signer-in'
    [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:PFX))
    $signer = Get-Signer -Path $pfx -Work $work
    foreach ($name in $signed) {
        $result = Set-AuthenticodeSignature -FilePath (Join-Path $Folder $name) -Certificate $signer -HashAlgorithm SHA256
        Assert-Signed -Result $result -What $name
    }
    if (Test-Path $catalog) { Remove-Item $catalog }
    New-FileCatalog -Path $Folder -CatalogFilePath $catalog -CatalogVersion 2.0 | Out-Null
    $result = Set-AuthenticodeSignature -FilePath $catalog -Certificate $signer -HashAlgorithm SHA256
    Assert-Signed -Result $result -What 'the catalog'
    # The public half only, as the installer trusts it in the guest.
    [IO.File]::WriteAllBytes((Join-Path $Folder 'ruxn-driver.cer'), $signer.Export('Cert'))
    Write-Output "Signed $($signed -join ', '), made and signed the catalog, and wrote ruxn-driver.cer."
}
finally {
    Remove-Item -Recurse -Force $work
}
