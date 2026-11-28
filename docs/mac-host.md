# The Mac host

This is the Mac app: `mac/` is the SwiftUI app, `mac/host/` its Rust core. It runs Windows 11 for ARM or Ubuntu on Apple silicon, with the guest drawing on the Mac's GPU.

It is a preview, and some of it isn't here yet. Here is what the app needs that this repo doesn't hold:

- A Windows guest driver and its package, signed for testing. The package isn't published, so an install stops at its driver step and offers to go on without it.
- The Ubuntu guest's driver package. It's built from `guest/drivers/linux-pvg/` and our Mesa branch in [ruxnlabs/mesa](https://github.com/ruxnlabs/mesa), and published on files.ruxn.com by `.github/workflows/linux-package.yml`. Until a version is up, an Ubuntu install stops at its driver step and offers to go on without it.

The graphics process and the QEMU it needs are here: `mac/pvg-host`, `mac/pvg-sys` and `qemu/mac/`, below. The nightly build carries them, and rmhost: QEMU with its firmware and libraries, the graphics process and rmhost are inside the app (`Contents/Helpers`, `Contents/Frameworks` and `Contents/Resources/qemu`), each signed with the hardened runtime, QEMU with the hypervisor entitlement and nothing else. `Contents/Resources/NOTICE.txt` says what is in the app, under what licence and where its source is. The app finds them there first, and otherwise through its Settings or an environment variable (`GUESTHOUSE_QEMU`, `GUESTHOUSE_PVG_QEMU`, `GUESTHOUSE_PVG_HOST`, `GUESTHOUSE_RMHOST`).

The graphics process uses three classes of Apple's ParavirtualizedGraphics framework that Apple doesn't declare: `PGMemoryMapDescriptor`, `PGIOSurfaceHostDeviceDescriptor` and `PGIOSurfaceHostDevice`. The two mapper classes are declared as QEMU's own `apple-gfx-mmio.m` declares them, and the memory map, `PGMemoryMapDescriptor` and the property that takes it, as the patch on QEMU's list adds it. It also calls `reset` on `PGIOSurfaceHostDevice`, which no header or patch names. None of that is a public interface, so a macOS update can break it. Guesthouse isn't affiliated with Apple, and the app isn't distributed through the App Store.

## How the pieces fit

Four programs run for one machine.

- The app, `mac/`, shows the machines and their screens. It doesn't run a machine itself.
- The agent, `mac/Agent` on the core in `mac/host`, owns the running machines, so they carry on when the app quits. It starts QEMU and the graphics process for each machine and talks to both over sockets.
- `guesthouse-pvg-host`, the graphics process in `mac/pvg-host`, one per machine. It holds the machine's graphics device, which is macOS's own, from the ParavirtualizedGraphics framework. That device was written for Apple's own guest driver and trusts it in places: a few things a guest can say end the process the device is in. So nothing the guest says reaches the device before the guard in `mac/pvg-sys` has read it: every register write, and every command in the guest's rings. If the guard refuses something, the device never sees it. If the device dies anyway, it's this process that goes, not QEMU or the agent. It hands the guest's picture to the app as IOSurfaces, through `rmframes`, with no copy on the way.
- QEMU, with the `pvg-fwd` device from `qemu/mac/`. The machine's RAM is shared memory. `pvg-fwd` shows the guest a PCI device and forwards its register reads and writes over a socket to the graphics process, which maps the same RAM. The interrupts come back the same way.

The guard knows one macOS build at a time. The device's registers and commands aren't published, so `mac/pvg-sys/src/profile.rs` holds what was seen on each build it knows, and on a build it doesn't know the graphics process won't start. Today that's macOS 27.2.1 (26B131) only. A new macOS build needs a look at the device and a profile before machines on it have a screen.

To build the two programs for the app:

```sh
brew install glib pixman ninja pkgconf
sh qemu/mac/build.sh
sh mac/build-pvg-host.sh
```

That leaves QEMU, the libraries it needs, its firmware and licences, and the graphics process in `target/mac-helpers/`, laid out for `mac/nightly.sh --helpers` (below). [qemu/mac/README.md](../qemu/mac/README.md) says what the patches are.

The graphics process and the guard are s3k000's work.

See `mac/README.md` for how to build it.

## Nightly builds

A nightly build of the Mac app is built from `main` each night, if `main` has changed, by `.github/workflows/mac-nightly.yml` on a GitHub-hosted Mac (the `xcode-27` image, which has Xcode 27 and macOS 27). `mac/nightly.sh` does the building: a Release build, every program in it signed with the hardened runtime, a signed disk image, notarisation and stapling, then the image, its `.sha256` and `guesthouse-macos-nightly.json` (version, commit, date, file, size, sha256 and the lowest macOS). The app's Settings can look for it: Updates, Look for nightly builds, which is off unless you turn it on.

None has been published yet. Until the first one is on files.ruxn.com there is nothing to download, and the app's switch finds nothing.

The workflow needs these before its first run:

- An environment called `mac-nightly`, limited to `main`, with five secrets: `MAC_SIGN_P12_BASE64` and `MAC_SIGN_P12_PASSWORD` (the Developer ID Application certificate and its key), `MAC_NOTARY_KEY`, `MAC_NOTARY_KEY_ID` and `MAC_NOTARY_ISSUER_ID` (an App Store Connect key that notarises). `mac/nightly.sh` never prints them and puts the certificate in a keychain of its own that it deletes.
- The Worker behind files.ruxn.com taking `mac-nightly.yml` for the project `guesthouse-nightly`, whose files are at `/p/guesthouse-nightly/`, as it takes `release-files.yml` for `/p/guesthouse/`. It takes only `Guesthouse-nightly-YYYYMMDD-<8 hex>.dmg` and its `.dmg.sha256` (written once), and `Guesthouse-nightly.dmg`, its `.sha256` and `guesthouse-macos-nightly.json` (which are replaced). The upload has no secret: `scripts/files-publish.sh` sends the job's GitHub OIDC token.
- For the 14-day cleanup (`mac-nightly-cleanup.yml`), the repository variable `FILES_BUCKET` and an environment `mac-nightly-cleanup` with the bucket's keys. Without them the cleanup does nothing.

A nightly is the release's major and minor with the date as the patch (`0.1.20261128`), so it is newer than the release it follows and older than the next one.

## Helpers inside the app

`mac/nightly.sh --helpers FOLDER` puts the programs the app runs inside the app, each signed with the hardened runtime and its own entitlements (`mac/Nightly/entitlements/NAME.entitlements`, none if there is no file). The folder is laid out as the app wants it:

- the programs (`guesthouse-pvg-host`, `qemu-system-aarch64`, `rmhost`) go to `Contents/Helpers`, where the app looks for them first;
- `Resources/` goes to `Contents/Resources`: QEMU's firmware (`edk2-aarch64-code.fd`, `edk2-aarch64-vars.fd`) and the graphics device's option ROM (`pvgfb.rom`) in `Resources/qemu`, which a QEMU in the app finds beside itself;
- `Frameworks/` goes to `Contents/Frameworks`: libraries QEMU needs.

`sh mac/rmhost/build.sh --helpers FOLDER` puts `rmhost` in such a folder. QEMU needs the hypervisor entitlement and has its file; the graphics process needs no entitlement, so it has none. `sh qemu/mac/build.sh` and `sh mac/build-pvg-host.sh` put QEMU, its libraries and firmware, and the graphics process in such a folder too.
