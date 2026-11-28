<!-- SPDX-License-Identifier: MIT -->
# pvgfb

A small arm64 UEFI driver that puts a framebuffer on the paravirtualized
graphics PCI device (vendor 0x106B, device 0xEEEE, class 0x038000) and
publishes it as a Graphics Output Protocol. It is packed as the device's PCI
option ROM, so the firmware loads it and Windows finds a GOP framebuffer to
draw on before it has a driver of its own.

It is plain freestanding C. It has its own small UEFI header (`efi.h`) and
needs neither edk2 nor gnu-efi.

## Files

- `efi.h`: the UEFI types, GUIDs and protocol layouts the driver uses.
- `pvgfb.h`: the device's registers and the per-device state.
- `driver.c`: the entry point and the driver binding (Supported, Start, Stop).
- `device.c`: the register sequence.
- `gop.c`: QueryMode, SetMode and a software Blt.
- `name.c`: the driver's name for the shell.
- `lib.c`: memcpy, memmove and memset.
- `build.sh`: builds `pvgfb.efi` and `pvgfb.rom`.
- `mkrom.py`: wraps the driver in a PCI expansion ROM.

## The device

BAR 0 holds 32-bit little-endian registers. The firmware leaves memory
decoding off, so Start turns it on through PciIo. Then:

| Step | Register | Access | Value |
|---|---|---|---|
| 1 | 0x1200 | write | 1, turns the early framebuffer on (the others read 0 before this) |
| 2 | 0x1204 | read | the number of modes |
| 3 | 0x1208, 0x120c | write, read | a mode index, then its size as height << 16 \| width (0xffffffff past the last) |
| 4 | 0x1210 | write | the framebuffer's guest-physical address >> 14 |
| 5 | 0x1214 | write | its length in bytes, a whole number of 16 KiB |
| 6 | 0x1218 | write | 0x20, bits per pixel |
| 7 | 0x121c | write | the mode, as height << 16 \| width; the picture appears now |

Writing 0 to 0x1200 turns it off. Pixels are BGRA 8:8:8:8, which is GOP's
`PixelBlueGreenRedReserved8BitPerColor`, and a row is width * 4 bytes. The
driver writes nothing else.

The framebuffer is guest RAM: one reserved-memory allocation, big enough for
the largest mode and aligned to 16 KiB, made once in Start. Reserved memory
outlives ExitBootServices, so the OS does not reuse it.

## How it behaves

- Supported: the controller has PciIo and the vendor and device IDs match.
- Start: opens PciIo by driver, enables memory decoding, reads up to 16
  modes, allocates the framebuffer, sets mode 0, and makes a child handle
  with the GOP and a device path (the PCI path plus an ACPI _ADR node for a
  display output). The child opens PciIo by child controller.
- SetMode: clears the screen and writes steps 5 to 7 for the new mode. The
  address in step 4 is written once.
- Stop: for the child, writes 0 to 0x1200 and removes the GOP and device
  path; for the controller, frees the framebuffer and restores the PCI
  attributes.

## Build

    sh mac/pvgfb/build.sh [--out FOLDER] [--helpers FOLDER]

writes `pvgfb.efi` and `pvgfb.rom` to `mac/build/pvgfb` (or FOLDER). With `--helpers`, the ROM is also put in
`FOLDER/Resources/qemu`, which is where `mac/nightly.sh --helpers` takes it into the app, beside QEMU's
firmware. It needs clang (the one with Xcode works), lld-link (`brew install lld`; set `LLD_LINK` for another)
and python3. The nightly's helpers job and `.github/workflows/mac-graphics.yml` build it.

The ROM is padded to a multiple of 16 KiB. QEMU under Hypervisor.framework
cannot map a ROM BAR that is not a whole number of the host's 16 KiB pages,
and an 8 KiB ROM stopped QEMU with HV_BAD_ARGUMENT before the firmware ran.

## Use

The Mac app finds `pvgfb.rom` beside QEMU's firmware (`Contents/Resources/qemu` in the app) and gives it to each
machine it installs, Windows or Ubuntu: it is copied into the machine's folder and named by `option_rom` in
`machine.json`, and the launch passes it to the device as `romfile=`. The framework's own ROM for the device,
which `guesthouse-pvg-host --option-rom` names, is an x86-64 image, which the arm64 firmware cannot run, so it is
not used.

## Tested

In firmware only, on QEMU's edk2 for arm64, 2026-11-25:

- PV device as the only display, modes 1024x768 then 640x480: the firmware's
  text, logo and progress bar at 1024x768 with the right colours; the shell
  works, scrolls cleanly, and `drivers` and `dh` list the driver and its
  child handle. `reconnect` on the controller runs Stop and Start again.
- The same with 640x480 first: everything at 640x480.
- With ramfb present as well, in both orders: both displays show the console.

With Windows 11 Pro ARM 26300.9457, the same day, the PV device as the only
display:

- Modes 1024x768, 1280x800, 1920x1080, 640x480: the firmware starts at
  1024x768, Windows' boot loader calls SetMode for 1280x800, and the desktop
  shows there. So a change of mode after Start works.
- 1920x1080 as the only mode: the desktop shows at 1920x1080.
- In both, Windows draws with its Basic Display Driver and the trace holds
  only the registers above.

Not tested: Blt from the screen to a buffer, a second device, and a Mac
with System Integrity Protection on.
