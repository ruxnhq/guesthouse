# QEMU for the Mac host

The Mac host runs its machines in QEMU on Hypervisor.framework, with the guest's graphics device in a process of its own, `guesthouse-pvg-host`. QEMU needs a few changes for that. They're the patches in `patches/`, on QEMU's master at commit `6bf90d4fc5027567cb704d7888f5ec792b4ed75a` (2026-11-24, version 11.1.91), applied in order.

| Patch | Author | What |
|---|---|---|
| `0001` to `0004` | Lucas Amaral | Patches 1 to 4 of Mohamed Mediouni's series "[RFC PATCH v8 00/10] vmapple: making it work on the latest macOS host releases" on qemu-devel, 2026-09-16. When a guest touches a device's memory with an instruction Hypervisor.framework doesn't decode for QEMU (an access with no syndrome, such as a load or store of a pair of registers), QEMU used to stop the machine. With these it decodes the instruction itself and carries on. |
| `0005` | Mohamed Mediouni | Patch 5 of the same series, "vmapple: apple-gfx: make it work on the latest macOS release". macOS 27 removed the old way of handing the framework a guest's memory, and without this patch QEMU's `apple-gfx` doesn't build against the macOS 27 SDK. |
| `0006` | s3k000 | `pvg-fwd`, a PCI device that shows the guest what `apple-gfx-pci` shows and forwards its registers and interrupts over a socket to the process that has the device. |
| `0007` | xnixy | `host-loopback=off` for QEMU's user network, the same as `qemu/qemu-host-loopback.patch` for the Linux host. A Linux machine's network has it, so the guest reaches no service on the Mac's loopback through 10.0.2.2. |
| `0008` | xnixy | Holds the decoding of `0001` to `0004` to the access that faulted. The instruction is read again from guest memory after the exit, so each access it makes has to lie in the faulting address's page and go the same way, read or write, or the guest gets a data abort. A failed decode is reported once. |
| `0009` | xnixy | Gives the framework each guest memory range at its own place in its RAM region (`0005` took the region's start), keeps the region for as long as the framework may use it, and makes no host device for an MMIO device whose realize failed. |

Each of the list's patches is as its author sent it, with his name, date, message and sign-off, and none of them is in QEMU yet. Patches 6 to 10 of the series are about QEMU's machine for macOS guests, which the Mac host doesn't use, so they aren't here.

The first four are here because the Windows guest has only ever been run on a QEMU that has them. A Linux guest with our driver runs without them: its register accesses are all single loads and stores.

None of this goes to QEMU. `pvg-fwd` and `0007` have no Signed-off-by for that reason.

## Building it

```sh
brew install glib pixman libslirp ninja pkgconf
sh qemu/mac/build.sh
sh mac/build-pvg-host.sh
```

`build.sh` fetches that one commit from QEMU's own git, checks it against the commit's hash and its tree's hash before anything else, applies the patches, and builds `qemu-system-aarch64` with Hypervisor.framework, the framework's own device and the network, and none of QEMU's other optional parts. It takes a few minutes. It writes `target/mac-helpers/`, laid out as `mac/nightly.sh --helpers` takes it:

- `qemu-system-aarch64`, and `guesthouse-pvg-host` from `mac/build-pvg-host.sh`. In the app they go to `Contents/Helpers`.
- `Frameworks/`, the libraries QEMU links that macOS doesn't have (glib and what glib needs). QEMU finds them through its rpaths, in the app's `Contents/Frameworks` and in this folder.
- `Resources/qemu/`, the firmware (`edk2-aarch64-code.fd` and `edk2-aarch64-vars.fd`), where a QEMU in the app finds it, with `licences/` (QEMU's, the firmware's and the libraries') and `SOURCE.txt`, which names the commit, the patches and the libraries' versions.

In the app, QEMU is signed with the hardened runtime and the one entitlement Hypervisor.framework asks for, `com.apple.security.hypervisor` (`mac/Nightly/entitlements/qemu-system-aarch64.entitlements`). The graphics process is signed with the hardened runtime and no entitlements.

`--configure-only` stops after QEMU's configure. `QEMU_SLIRP=disabled` builds without the network, if you don't have libslirp and don't want QEMU to fetch it.

The patches are GPL-2.0-or-later, as the parts of QEMU they change are. QEMU itself is under its own licences, in its `COPYING` and `LICENSE`.
