//! `Test-CodeSigningOnly` in the Windows graphics driver's install script: a certificate that names other
//! uses than signing code is not trusted. Certificates are made here, in a scratch store, and their public
//! halves written to files.
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
    // 1.3.6.1.5.5.7.3.3 is code signing, and 1.3.6.1.5.5.7.3.1 is TLS server authentication.
    let uses = match extra_use {
        Some(extra) => format!("'2.5.29.37={{text}}1.3.6.1.5.5.7.3.3,{extra}'"),
        None => "'2.5.29.37={text}1.3.6.1.5.5.7.3.3'".to_owned(),
    };
    let said = run(
        shell,
        &format!(
            "$ErrorActionPreference = 'Stop'\n{}$made = New-SelfSignedCertificate -Subject 'CN=guesthouse test only' -CertStoreLocation Cert:\\CurrentUser\\My -KeyExportPolicy NonExportable -TextExtension @({uses})\n\
             try {{ [IO.File]::WriteAllBytes({}, $made.Export('Cert')); Test-CodeSigningOnly {} }} finally {{ Remove-Item \"Cert:\\CurrentUser\\My\\$($made.Thumbprint)\" -DeleteKey }}",
            functions_from(&script(), &["Test-CodeSigningOnly"]),
            quote(&file),
            quote(&file)
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
