# Guesthouse for macOS

A Mac app that runs a Windows or Ubuntu virtual machine on Apple silicon, with the machine drawing on the Mac's own GPU. It is a SwiftUI app on a small Rust core. A background agent owns the running machines, so they carry on when the app quits. It shares its rules with the Linux and Windows hosts through the crates in `crates/`, and the Mac's part of it is `mac/host`.

I'm saying the awkward parts first, because you should know them before you build it.

## What you should know

- The graphics process, `guesthouse-pvg-host`, uses three classes of Apple's ParavirtualizedGraphics framework that Apple doesn't declare. QEMU's patch that lists them does the same. They are not a public interface, so a macOS update can break them, and then a machine has no screen until the host is changed.
- Guesthouse isn't made, endorsed or supported by Apple. macOS and Apple silicon are theirs.
- It is not on the App Store and won't be. The App Store's sandbox would not let it do this. You build it, or you take a build signed with a Developer ID.
- It needs macOS 27 and a Mac with Apple silicon.

## What works

- Making a machine from a disk image, or installing Windows 11 for ARM from an ISO you give it, or Ubuntu, which it downloads and checks against Ubuntu's checksums.
- Starting, shutting down and turning off a machine, with the machine's screen in a window, the keyboard and the mouse.
- The Windows graphics driver and the Ubuntu one, installed at the end of an install. The Windows driver is test signed, so a Windows guest runs with test signing on.
- Viewing a machine from another device, with pairing. It needs `rmhost`, which is in `mac/rmhost` and built with `sh mac/rmhost/build.sh`.
- Sending a bug report, with a preview of what is in it first.

## What doesn't work yet

- No signed release. Nothing here has been notarised, and the driver package isn't published, so an install from a clean Mac stops at the driver step and offers to go on without it.
- Windows needs an installer you download yourself. Microsoft's page can't be automated.
- Snapshots, clipboard, shared folders and sound are not built.
- The screen window doesn't pass every key to the guest. Command-Tab, Mission Control, and the Globe key stay with the Mac.
- It has been tried on one Mac and one kind of GPU. Expect rough edges.

## Build

You need Xcode 27, XcodeGen and Rust with the `aarch64-apple-darwin` target. The graphics process and QEMU with the `pvg-fwd` device are in `mac/pvg-host` and `qemu/mac/`, and `docs/mac-host.md` says how to build them. The nightly build carries them inside the app. An app you build yourself finds them through its Settings or an environment variable, or in `Contents/Helpers` if you put them there with `mac/nightly.sh --helpers`.

```sh
./build.sh
```

That runs `build-core.sh` (the Rust core, `mac/host`, built with cargo from the repo's root), `xcodegen generate` and `xcodebuild`. The app ends up in `build/dd/Build/Products/Release/Guesthouse.app`, signed ad hoc, or with your certificate if you set `GUESTHOUSE_SIGN_IDENTITY`.

The core's tests run without a guest:

```sh
cargo test -p guesthouse-host-mac
```

## Where things are

- `mac/host/` is the Rust core: machines, the agent, installs, reports, the rules of the app's pages. Swift only draws and calls it through `mac/host/include/guesthouse_core.h`.
- `App/` is the SwiftUI app, `Agent/` the agent's program, `Shared/` what both use, `Launchd/` the agent's launchd file.
- `Tests/` are the Swift tests of the app's wording. `rmframes/` is how the graphics process hands the screen to the app as IOSurfaces, MIT. `pvgfb/` is the UEFI driver that gives a guest's firmware its screen on the graphics device, MIT; the helpers carry it as `pvgfb.rom`.

If you add logic that doesn't touch the system, put it in a shared crate if one has it, or shape it like the other hosts' version first.

## Licence

GPL-2.0-or-later, see the repo's `LICENSE`. `rmframes/` and `pvgfb/` are MIT. What I took from other projects is in `THIRD-PARTY.md` in the repo's root.
