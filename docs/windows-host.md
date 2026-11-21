# A Windows host

This is the first step of Guesthouse on Windows: Windows 11 and Ubuntu VMs with GPU-PV, run through Windows' Host Compute System with only the Virtual Machine Platform feature on. Hyper-V isn't needed, so Windows 11 Home can do it, and none of Hyper-V's tools are used. The rest of Guesthouse, the daemon and the app, still runs on Linux only. On Windows those crates build empty, so the whole workspace builds.

`tools/hcs-boot` is the program, and `crates/hcs` is what it calls.

## What it needs

- Windows 11, Home or Pro, with Virtual Machine Platform on. WSL 2 turns it on, so it's often on already.
- A card whose driver offers GPU-PV. `guesthouse-hcs-boot gpus` lists them.
- A Windows 11 ISO, or its `install.wim` or `install.esd`, or an Ubuntu Server ISO. Ubuntu 26.04 is the one aimed at.
- An elevated prompt for `prepare` and `boot`, as HCS, its network and attaching a disk all need one.
- For an Ubuntu guest's CUDA and `nvidia-smi`, WSL installed on the host, as `libdxcore.so` comes with it.

To see whether Virtual Machine Platform is on, in an elevated PowerShell:

```powershell
Get-WindowsOptionalFeature -Online -FeatureName VirtualMachinePlatform
```

If it says `Disabled`, turn it on and restart:

```powershell
dism.exe /online /enable-feature /featurename:VirtualMachinePlatform /all /norestart
```

## Trying all of it at once

`tools/hcs-boot/try-on-this-pc.ps1` runs every step in order in an elevated PowerShell. It checks the PC and builds the program. Then it makes and boots a Windows guest, and makes, installs and boots an Ubuntu guest. Each boot has a time limit, a failed step doesn't stop the next, and everything goes to `guesthouse-try.log` in the folder you give:

```powershell
.\tools\hcs-boot\try-on-this-pc.ps1 -Folder D:\vms -WindowsIso D:\iso\Win11.iso -UbuntuIso D:\iso\ubuntu-26.04-live-server-amd64.iso
```

## Building

```powershell
cargo build --release -p guesthouse-hcs-boot
```

## Using it

`gpus` needs no elevation. It lists each card with its partition adapter's path, which is how HCS takes a card, and its driver's folder:

```powershell
.\target\release\guesthouse-hcs-boot.exe gpus
```

`document` prints what `boot` would send to HCS, without sending it, and needs no elevation either.

`prepare` makes a disk from the ISO, and tells a Windows ISO from an Ubuntu one by what's on it. It never writes over a file that's there. The guest's administrator and a new password go in a file beside the disk.

```powershell
.\target\release\guesthouse-hcs-boot.exe prepare --image D:\iso\Win11.iso --disk D:\vms\win11.vhdx
```

`boot` copies the card's driver again if the host's changed, plugs the machine into a network, starts it, and then gives it the card. The guest has no screen anyone can see yet, so at every start it says on its serial port what it sees of its card, and `boot` prints that:

```powershell
.\target\release\guesthouse-hcs-boot.exe boot --disk D:\vms\win11.vhdx
```

Ctrl+C asks the guest to shut down, and a second Ctrl+C stops it at once. `--for-minutes N` does the same by itself after N minutes. The machine stops too if the program does, as HCS stops a machine when its last handle closes.

Without an NVIDIA card, `--gpu TEXT` picks the card whose name holds TEXT, and `--no-gpu` boots with none. `--memory-mb`, `--cpus`, `--no-network` and `--no-tpm` do what they say.

### A Windows guest

`prepare` makes a new VHDX, partitions it with diskpart, applies the edition with DISM, and writes the boot files with bcdboot. Then it adds the card's driver, an answer file and a small report. The report writes to the guest's serial port, and to a file on its disk, which `boot` prints once the machine has stopped, in case the serial port said nothing. The first start takes a few minutes, as Windows sets itself up. Lines like these are what to look for, the first saying the guest has the card and its driver:

```
guest: guesthouse: card: NVIDIA GeForce RTX 5090, driver 32.0.16.xxxx, status OK
guest: guesthouse: nvidia-smi: GPU 0: NVIDIA GeForce RTX 5090 (UUID: ...)
guest: guesthouse: address: 172.24.80.12
```

With a Pro guest, Remote Desktop to the address it prints shows its screen.

### An Ubuntu guest

`prepare` makes an empty disk for Ubuntu's installer, and a small FAT disk beside it, the files disk, labelled `CIDATA`. The files disk does three jobs:

- It boots the installer. Its UEFI fallback is the ISO's own Shim and GRUB, with a menu that starts the installer with no questions and its console on the serial port.
- It's the installer's answer file, which cloud-init finds by its label.
- It keeps the host's GPU files, which the guest mounts at `/usr/lib/wsl` as WSL does. These are WSL's own DXCore and D3D12 libraries, the libraries the driver puts in `lxss\lib`, and the driver's folder from the driver store.

The first `boot` installs Ubuntu. That takes a while, and the machine turns itself off when it's done. `boot` sees the guest say so, and stops attaching the installer. The next `boot` starts the installed system. The installer speaks on the serial port only when the files disk's menu started it, so an install boot that's silent for five minutes started something else, and `boot` stops it and says so.

At every start a service in the guest mounts the GPU files and loads `dxgkrnl`, the guest's half of GPU-PV. Then it reports on the serial port. A distro's kernel has no `dxgkrnl`, as it's only in Microsoft's WSL kernel. So on its first start, and after a kernel update, the guest builds it. It downloads the build tools from Ubuntu and `dxgkrnl`'s source from Microsoft's WSL kernel on GitHub, which Guesthouse never keeps a copy of. The guest's Secure Boot is off, as that module isn't signed. Lines like these are what to look for:

```
guest: guesthouse: dxg: present
guest: guesthouse: host driver: nv_dispi.inf_amd64_...
guest: guesthouse: nvidia-smi: GPU 0: NVIDIA GeForce RTX 5090 (UUID: ...)
```

SSH to the address it prints, with the account in the file beside the disk.

### The jail

What a guest writes to its serial port is the one thing on a Windows host that a Guesthouse program reads from a guest. The machine itself runs in Windows' own worker process, which Windows already keeps apart. So `boot` hands the port to `guesthouse-serial-relay`, which sits beside it and runs jailed, as QEMU does on Linux:

- In an AppContainer with no capabilities, it can open none of your files, and it has no network, not even loopback. That does what Landlock does on Linux.
- Process mitigations block every win32k call, code made at run time, and DLLs from a share or a low-integrity file. That does what seccomp does.
- Its job lets it start no process, keeps it off the desktop and the clipboard, caps its memory, and stops it when `boot` ends. That does what no new privileges does.

The jail's test reads four of these back from Windows inside the jailed process: the AppContainer, and that win32k calls, dynamic code and child processes are refused. The rest are set and not yet read back. The relay shows any control character, an escape sequence included, as a code, so a guest can't drive your terminal. `boot` never reads the port unjailed unless `--no-jail` asks it to. Windows keeps the relay's AppContainer under the name `guesthouse.serial-relay`. The first start lets that container read and run the relay's own file, and nothing else.

## What it doesn't do yet

- None of the boots has run yet. They need an elevated prompt, and `try-on-this-pc.ps1` is the first run.
- A Windows guest gets only the driver's folder in the driver store. That's what Direct3D needs. The files a driver also puts in `System32`, which some programs look for there, aren't placed yet.
- An Ubuntu guest without WSL on the host has no `libdxcore.so`, so `nvidia-smi` and CUDA won't run in it.
- There's no screen but Remote Desktop or SSH.
- When it fails, HCS's own answer is printed with the document that was sent. That answer names what it refused.
