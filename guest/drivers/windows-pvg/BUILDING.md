# Building the Windows graphics driver

Everything here is cross-built on an arm64 Mac. Nothing needs Visual Studio, only the package's catalog is made on Windows, and nothing uses Xcode's Metal compiler. The kernel driver is built with clang and lld-link against Microsoft's driver kit. The user-mode parts are built with llvm-mingw.

## What you need

All of it goes in one folder, called `ROOT` below. None of it is in this repo.

| Where in `ROOT` | What | From |
|---|---|---|
| `.tools/llvm-mingw` | llvm-mingw 20261117, the macOS universal build | [its releases](https://github.com/mstorsjo/llvm-mingw/releases) |
| `.tools/wdk/microsoft.windows.wdk.arm64` | Microsoft's driver kit for arm64, 10.0.28000 | the `Microsoft.Windows.WDK.ARM64` package on nuget.org, unzipped |
| `.tools/wdk/microsoft.windows.sdk.cpp`, `.tools/wdk/microsoft.windows.sdk.cpp.arm64` | the Windows SDK's headers and arm64 libraries, 10.0.28000 | the `Microsoft.Windows.SDK.CPP` and `Microsoft.Windows.SDK.CPP.arm64` packages, unzipped |
| `.tools/src/dxmt` | DXMT at commit `faa73e4a0686e432ce5d3bb99a02b5363d76885c` | `git clone https://github.com/3Shain/dxmt` |
| `.venv/bin/meson`, `.venv/bin/cmake` | meson and cmake | `python3 -m venv ROOT/.venv && ROOT/.venv/bin/pip install meson cmake` |

Also `ninja`, `python3` and `osslsigncode`, for example from Homebrew, and `lld-link` (`LLD_LINK` names it if it isn't `/opt/homebrew/bin/lld-link`).

Microsoft's packages are under Microsoft's licence, which you accept when you fetch them. Don't commit them or pass them on.

## Steps

1. DXMT's shader compiler, airconv, and the LLVM 15 it needs, for the Mac and for Windows on arm64. This takes a while the first time, and fetches LLVM 15.0.7's source and checks its SHA-256:

        sh dxmt-win/airconv-build.sh ROOT

2. The kernel driver, the user-mode driver and the tools:

        sh ruxnwddm/build.sh ROOT

3. `winemetal.dll`, DXMT's Direct3D 11 and DXGI under our names (`ruxnd3d11.dll`, `ruxndxgi.dll`), and the test programs:

        sh dxmt-win/build.sh ROOT

The output is in `ROOT/build/ruxnwddm` and `ROOT/build/dxmt-win`.

## Signing

Windows only loads a kernel driver that's signed. For a test build, make your own test certificate and set `SIGN_CERT` and `SIGN_KEY` to its PEM certificate and key before step 2. Without them the driver is built and not signed. The user-mode DLLs don't need a signature of their own, but the package's catalog, `ruxnwddm-full.cat`, covers every file in it and must be signed with the same certificate. The catalog is made on Windows, with PowerShell's `New-FileCatalog -CatalogVersion 2.0` over the package's folder, and signed with `Set-AuthenticodeSignature` (or with `Inf2Cat` and `signtool` from the driver kit).

Never put the private key in the package or in this repo. `release/package.sh` refuses a folder that has one.

## The package

With the files signed, `release/package.sh SIGNED_DIR OUT_DIR` lays out the package as the Mac app's installer expects it: each file under the name it's published by, a `.sha256` beside each, and `guesthouse-windows-driver-latest.json`. `SIGNED_DIR` holds `ruxnwddm.sys`, `ruxnwddm-full.inf`, `ruxnwddm-full.cat`, `ruxnumd.dll`, `winemetal.dll`, `ruxndxgi.dll`, `ruxnd3d11.dll` and the public certificate as `ruxn-driver.cer`.
