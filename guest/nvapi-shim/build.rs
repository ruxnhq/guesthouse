//! A version resource for the DLL, saying it is Guesthouse's, so nobody takes it for NVIDIA's. It
//! needs a resource compiler: MinGW's windres for the GNU target, which is how the DLL is built. A
//! build without one still works, without the resource.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu")
    {
        return;
    }
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let numbers: Vec<u16> = version.split(['.', '-']).filter_map(|part| part.parse().ok()).take(3).collect();
    let [major, minor, patch] = [0, 1, 2].map(|index| numbers.get(index).copied().unwrap_or(0));
    let script = format!(
        r#"1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x2
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Guesthouse"
      VALUE "FileDescription", "Guesthouse NVAPI wrapper, which loads NVIDIA's nvapi64.dll"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "nvapi64"
      VALUE "OriginalFilename", "nvapi64.dll"
      VALUE "ProductName", "Guesthouse"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let (source, object) = (out.join("version.rc"), out.join("version.o"));
    fs::write(&source, script).expect("OUT_DIR is writable");
    let windres = env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".to_owned());
    let made = Command::new(&windres).arg(&source).arg("-O").arg("coff").arg("-o").arg(&object).status();
    match made {
        Ok(status) if status.success() => println!("cargo:rustc-link-arg-cdylib={}", object.display()),
        _ => println!("cargo:warning=no version resource: {windres} didn't run"),
    }
}
