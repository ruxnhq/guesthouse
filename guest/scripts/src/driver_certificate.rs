//! `Test-CodeSigningOnly` in the Windows graphics driver's install script: a certificate that names other
//! uses than signing code is not trusted. Certificates are made in memory and their public halves written to
//! files, so no certificate store or account's rights are involved.
#![cfg(windows)]

use crate::{Scratch, functions_from, quote, run, shell};
use std::path::Path;

fn script() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../drivers/windows-pvg/release/install.ps1")
}

fn answer(extra_use: Option<&str>) -> String {
    let Some(shell) = shell() else { return "True".to_owned() };
    let scratch = Scratch::new();
    let file = scratch.path("driver.cer");
    // 1.3.6.1.5.5.7.3.3 is code signing, and 1.3.6.1.5.5.7.3.1 is TLS server authentication. The certificate
    // is made in memory, so no store or account's rights come into it.
    let extra = extra_use
        .map(|oid| format!("; $uses.Add((New-Object System.Security.Cryptography.Oid '{oid}')) | Out-Null"))
        .unwrap_or_default();
    let said = run(
        shell,
        &format!(
            "$ErrorActionPreference = 'Stop'\n{}\
             $key = [System.Security.Cryptography.RSA]::Create(2048)\n\
             $request = New-Object System.Security.Cryptography.X509Certificates.CertificateRequest 'CN=guesthouse test only', $key, ([System.Security.Cryptography.HashAlgorithmName]::SHA256), ([System.Security.Cryptography.RSASignaturePadding]::Pkcs1)\n\
             $uses = New-Object System.Security.Cryptography.OidCollection\n\
             $uses.Add((New-Object System.Security.Cryptography.Oid '1.3.6.1.5.5.7.3.3')) | Out-Null\n\
             {extra_uses}\n\
             $request.CertificateExtensions.Add((New-Object System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension $uses, $false))\n\
             $made = $request.CreateSelfSigned([DateTimeOffset]::UtcNow.AddDays(-1), [DateTimeOffset]::UtcNow.AddDays(30))\n\
             [IO.File]::WriteAllBytes({}, $made.Export('Cert'))\n\
             Test-CodeSigningOnly {}",
            functions_from(&script(), &["Test-CodeSigningOnly"]),
            quote(&file),
            quote(&file),
            extra_uses = extra.trim_start_matches("; "),
        ),
    );
    assert_eq!(said.code, Some(0), "{said}");
    said.last().to_owned()
}

#[test]
fn a_certificate_for_signing_code_alone_is_trusted() {
    assert_eq!(answer(None), "True");
}

#[test]
fn a_certificate_that_also_serves_tls_is_not() {
    if shell().is_none() {
        return;
    }
    assert_eq!(answer(Some("1.3.6.1.5.5.7.3.1")), "False");
}
