# Windows graphics driver for the Mac host

This is the graphics driver for a Windows 11 for ARM guest on the Mac host. Windows gets a real GPU adapter, and what it draws is drawn by the Mac's own GPU, with no copy of the frame on the CPU.

It's early. It's test signed, so the guest runs with test signing on, and it has been tried on one Mac. Expect rough edges, and please send a report when something draws wrong.

## What you should know

- The device this driver talks to is the paravirtualized graphics device that macOS gives a virtual machine. The host side uses three classes of macOS's ParavirtualizedGraphics framework that aren't declared, as QEMU's patch that lists them does. A macOS update can break them, and then the guest has no screen until the host is changed. See `docs/mac-host.md`.
- Guesthouse isn't made, endorsed or supported by the maker of macOS, and the Mac app isn't on the App Store.
- The driver is for Windows on arm64 only.
- It isn't signed by Microsoft. The package is signed with a test certificate, which the Mac app's installer trusts in the guest and then turns test signing on.

## What works

- The desktop. Windows composes it on this adapter, not in software, and flips each frame to the screen.
- Edge on the GPU, with its sandbox on: canvas, compositing, WebGL and video say "Hardware accelerated" on `edge://gpu`. A WebGL scene runs at about 50 frames a second, and a video plays alongside it.
- Direct3D 11 at feature level 11.0, and 10.0 and 10.1. Draws, render targets, clears, sampling, compute with raw buffers, block-compressed textures (BC1 to BC7), texture arrays, cubes and 3D textures, and multisampling at 4x.

## What doesn't work yet

- No tessellation. Hull and domain shaders are refused when a program makes them.
- Geometry shaders go through, but nothing has tested one yet.
- Unordered access views other than a raw buffer, typed and structured buffers, and indirect dispatch are offered but thin. Unordered access views in a pixel shader are dropped.
- Multisampling is 4x only. 2x, 8x and 16x aren't offered.
- The driver offers no video decoder, so video is decoded on the CPU.
- A process can hold about 1 GiB of GPU memory and 16,384 objects. A program that wants more gets out-of-memory.
- Some programs fail to make a swap chain with "Failed to find an output". I'm looking at that now.
- No Direct3D 12, no Direct3D 9 and no Vulkan or OpenGL of its own. Browsers get OpenGL ES through ANGLE on Direct3D 11.

## How it fits together

| Folder | What it is | Licence |
|---|---|---|
| `ruxnwddm/` | The kernel driver, `ruxnwddm.sys`: a WDDM 2.2 render and display driver. The user-mode driver, `ruxnumd.dll` (the `umd_*` files), which Windows' Direct3D 11 runtime loads. `wddmtool`, `dispcfg` and `umdtest`, small tools to look at it in a guest | kernel driver and tools GPL-2.0, user-mode driver MIT |
| `dxmt-win/` | `winemetal.dll`, the layer that turns Metal's calls into the device's commands (`wm_*`, `winemetal_pvg.*`, `translate*`), and the build of [DXMT](https://github.com/3Shain/dxmt) for Windows on top of it, with our patch to it and to LLVM 15 for its shader compiler, airconv | MIT, the patches under their project's licence |
| `dxmt-win/helpers/` | DXMT's helper shaders for clears, written by hand as AIR | MIT |
| `wire/` | The library that writes the device's command bytes | MIT |
| `release/` | `package.sh`, which lays out the driver package, and `install.ps1`, which installs it in a guest | GPL-2.0-or-later |

The user-mode driver hands Direct3D 11 to DXMT, which turns it into Metal. Metal here isn't macOS's: it's `winemetal.dll`, which writes what the Mac's GPU should do into the device's own command format and gives it to the kernel driver. The kernel driver sends it to the host and shows the frames.

## Build

See [BUILDING.md](BUILDING.md). In short: it builds on a Mac, with llvm-mingw for the user-mode parts and clang with Microsoft's driver kit for the kernel driver. The kit can't be in this repo, so you fetch it yourself.

## How it's installed

The Mac app does it. At the end of a Windows install it puts the driver package on a small disk for the guest and runs `install.ps1` there, which trusts the package's test certificate, turns test signing on, installs the driver with `pnputil` and restarts. The package is what `release/package.sh` lays out: the signed driver files, the public certificate, `install.ps1` and a manifest with each file's SHA-256. It never carries a private key.

## Licence

The kernel driver is GPL-2.0-only OR MIT; the code in it that talks to the device is ported from s3k000's Linux driver, and he agreed to these terms for it. The user-mode driver's own files, `winemetal.dll`'s and the wire library are MIT. `ruxnumd.dll` loads DXMT's two DLLs, which are LGPL-2.1-or-later, and `winemetal.dll` has DXMT's shader compiler linked in, so the Direct3D side of the package as a whole is under the LGPL-2.1-or-later. While `ruxnpvg_ioctl.h` is GPL-2.0, the built `winemetal.dll` that includes it is distributed under GPL-2.0 as a whole; its other sources stay MIT and LGPL-2.1-or-later. The LGPL's text is in [LICENSE-LGPL-2.1](LICENSE-LGPL-2.1), LLVM's in [LICENSE-LLVM](LICENSE-LLVM), and which file is under what is in [LICENSE](LICENSE). What I took from other projects is in `THIRD-PARTY.md` in the repo's root.

The wire library and the first version of the kernel driver's device code are the work of the contributor who wrote the Mac host's graphics process. Thank you.
