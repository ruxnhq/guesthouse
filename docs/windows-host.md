# A Windows host

This is the first step of Guesthouse on Windows: a Windows 11 VM with GPU-PV, run through Windows' Host Compute System with only the Virtual Machine Platform feature on. Hyper-V isn't needed, so Windows 11 Home can do it, and none of Hyper-V's tools are used. The rest of Guesthouse, the daemon and the app, still runs on Linux only.

`tools/hcs-boot` is the program, and `crates/hcs` is what it calls.

## What it needs

- Windows 11, Home or Pro, with Virtual Machine Platform on. WSL 2 turns it on, so it's often on already.
- A card whose driver offers GPU-PV. `guesthouse-hcs-boot gpus` lists them.
- A Windows 11 ISO, or its `install.wim` or `install.esd`.
- An elevated prompt for `prepare` and `boot`, as HCS, its network and attaching a disk all need one.

To see whether Virtual Machine Platform is on, in an elevated PowerShell:

```powershell
Get-WindowsOptionalFeature -Online -FeatureName VirtualMachinePlatform
```

If it says `Disabled`, turn it on and restart:

```powershell
dism.exe /online /enable-feature /featurename:VirtualMachinePlatform /all /norestart
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

`prepare` makes a disk from the ISO. It makes a new VHDX, partitions it with diskpart, applies the edition with DISM, writes the boot files with bcdboot, and puts on the card's driver, an answer file and a small report. It never writes over a file that's there. The guest's administrator and a new password go in a file beside the disk.

```powershell
.\target\release\guesthouse-hcs-boot.exe prepare --image D:\iso\Win11.iso --disk D:\vms\win11.vhdx
```

`boot` copies the card's driver to the disk again if the host's changed, plugs the machine into a network, starts it, and then gives it the card. The guest has no screen anyone can see yet, so at every start it says on its serial port what it sees of its card, and `boot` prints that:

```powershell
.\target\release\guesthouse-hcs-boot.exe boot --disk D:\vms\win11.vhdx
```

The first start takes a few minutes, as Windows sets itself up. Lines like these are what to look for, the first saying the guest has the card and its driver:

```
guest: guesthouse: card: NVIDIA GeForce RTX 5090, driver 32.0.16.xxxx, status OK
guest: guesthouse: nvidia-smi: GPU 0: NVIDIA GeForce RTX 5090 (UUID: ...)
guest: guesthouse: address: 172.24.80.12
```

Ctrl+C asks the guest to shut down, and a second Ctrl+C stops it at once. The machine stops too if the program does, as HCS stops a machine when its last handle closes.

With a Pro guest, Remote Desktop to the address it prints shows its screen.

Without an NVIDIA card, `--gpu TEXT` picks the card whose name holds TEXT, and `--no-gpu` boots with none. `--memory-mb`, `--cpus`, `--no-network` and `--no-tpm` do what they say.

## What it doesn't do yet

- Only the driver's folder in the driver store goes to the guest. That's what Direct3D needs. The files a driver also puts in `System32`, which some programs look for there, aren't placed yet.
- There's no screen but Remote Desktop.
- When it fails, HCS's own answer is printed with the document that was sent. That answer names what it refused.
