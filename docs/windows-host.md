# A Windows host

This is the first step of Guesthouse on Windows: Windows 11 and Ubuntu VMs with GPU-PV, run through Windows' Host Compute System with only the Virtual Machine Platform feature on. Hyper-V isn't needed, so Windows 11 Home can do it, and none of Hyper-V's tools are used. The rest of Guesthouse, the daemon and the GTK app, still runs on Linux only. On Windows the daemon's crates build empty, so `cargo test` builds the workspace, but the GTK app doesn't build on Windows at all.

`tools/hcs-boot` is the program. `crates/host-windows` is the work it does, which the Guesthouse service will share, and `crates/hcs` is the calls into Windows underneath. `crates/app-windows` is the window that will drive the service, as [the app](#the-app) says.

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

Without `--gpu`, a guest gets the card with the most memory of its own, which on a PC with an integrated card too is the discrete one, NVIDIA, AMD or Intel. `--gpu TEXT` picks the card whose name holds TEXT, and `--no-gpu` boots with none. `--memory-mb`, `--cpus`, `--no-network` and `--no-tpm` do what they say. `--no-nvapi-wrapper` boots an NVIDIA guest with NVIDIA's own NVAPI in System32 rather than Guesthouse's wrapper, for a game whose anti-cheat refuses the wrapper.

### A Windows guest

A guest's first boot sets it up with no card, and every boot after that has the card. The host's card reset twice while a guest with GPU-PV was setting up or shutting down, and setup doesn't need the card. A Windows guest counts as set up once its first report arrives, and an Ubuntu guest once its install is done.

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

## The service

HCS needs an administrator, and Guesthouse's app runs as you, so the app never touches a machine itself. It asks the Guesthouse service, `guesthouse-service`, which runs as the system, on the pipe `\\.\pipe\guesthouse`, in the words of `crates/control`. The service does the same work `guesthouse-hcs-boot` does, and keeps every rule itself:

- It answers the system, an elevated administrator, and the user signed in at the console. Anyone else is refused and written in its log.
- It keeps the machines in `%ProgramData%\Guesthouse\machines`, a folder each, which it makes its own, so only the system and administrators can open them. A folder that's there already and wasn't made by an administrator is refused rather than taken over. Its log is `%ProgramData%\Guesthouse\service.log`.
- It opens the ISO you name as you first, and refuses one you couldn't read yourself, or one on a share.
- A machine's name, memory, processors and disk size are held to limits, and one machine is made at a time.
- A machine's memory, processors, card and DLSS switch can be changed once it's made, only while it's off, and to the same limits as a new machine's. A card asked for by name has to be one of this PC's. They're kept in the machine's folder, and the next start gives it them.
- A Linux guest is given a copy of its installer in its folder, as each boot changes the permissions of every disk it's given, and the service mustn't do that to your file. The copy goes once the guest has installed.
- Machines are HCS's `guesthouse-NAME`, so no other program's machines are touched. One left running when the service last stopped is stopped when it starts.
- When the service stops, or Windows shuts down, each running guest is asked to shut down, and stopped if it hasn't in a minute.

To try it without installing it, in an elevated prompt:

```powershell
cargo build --release -p guesthouse-service -p guesthouse-host-windows
.\target\release\guesthouse-service.exe run
```

It serves until Ctrl+C. The relay must sit beside it, as `boot` looks for it there, which a release build does.

## The app

`crates/app-windows` is the app for all of this, `guesthouse-app.exe`, a WinUI 3 window made with Microsoft's windows-reactor. It runs as you, with no elevation, and does nothing to a machine itself: it asks the Guesthouse service on its pipe, `\\.\pipe\guesthouse`, in the words in `crates/control`. The service runs as the system, as HCS needs, and keeps every rule. Without it the window says the service isn't running, and asks again every two seconds.

```powershell
cargo build --release -p guesthouse-app-windows
.\target\release\guesthouse-app.exe
```

The first build on a PC downloads the Windows App Runtime from NuGet, and every build copies it beside the exe, so the app needs nothing installed. Copy the whole folder to move it. Every Windows program here carries its C runtime inside it, as `.cargo/config.toml` says, so none needs Visual C++'s runtime on the PC either.

On the left is each machine, with its system and a dot for where it stands: green running, amber on its way, grey stopped. A machine's page has Start and Stop at its top, and Delete under More. Stop asks the guest to shut down, and while it does, the button says Stop now, which turns it off at once. Delete asks first, and only a stopped machine can go. Below that are its Status, Connect and Settings. Settings has its memory, processors and card, which can be changed while it's stopped, and Save asks the service, which says why if it won't. A Windows guest with an NVIDIA card has a DLSS switch there too. On, the guest gets Guesthouse's NVAPI in place of NVIDIA's, as NVIDIA's won't tell DLSS which card it's on in a virtual machine. Guesthouse's isn't signed, and some games' anti-cheat refuses to start with it, so turning DLSS off puts NVIDIA's back at the machine's next start. Status says whether the machine is set up: its first boot sets it up without the card, and every boot after that has it. Connect shows the guest's administrator and password when asked, opens a Windows guest's screen in remoteman's viewer once it's set up (Open screen) or Remote Desktop on its address, and gives the `ssh` line for an Ubuntu guest. Open console shows any running machine's own screen, as HCS serves it on the machine's console pipe, in a window of its own through Windows' Remote Desktop control (`src/console.rs`), from the firmware on; closing it leaves the machine running, and a machine that stops has its console closed. It doesn't connect yet: the console's pipe hangs up on the control, which says it hit an internal error. `cargo run -p guesthouse-app-windows --example console-window -- NAME` tries it without the app. Under that is its log, as the service sends it, with problems marked in red. New machine makes one from an image, and shows the service's progress as it goes.

A build with the `preview` feature shows made-up machines instead of the service's, so each of the window's states can be looked at, light or dark, with no service and no GPU. It never reaches the service. `src/app/preview.rs` lists the states:

```powershell
cargo build -p guesthouse-app-windows --features preview
$env:GUESTHOUSE_APP_PREVIEW = "running"; $env:GUESTHOUSE_APP_THEME = "light"
.\target\debug\guesthouse-app.exe
```

An installed app, one beside the installer's `uninstall.exe`, asks the file drop for `guesthouse-windows-latest.json` when it starts, the file a Windows release publishes last (`crates/control/src/updates.rs`). When that names a newer version than the app's, a card at the top of each page says so, with Download, which opens the installer in the browser, and Not now, which puts it aside until the app next starts. It's fetched with the `curl.exe` Windows has, over HTTPS only, in 20 seconds at most, and no larger than 4 KB, and nothing is said when it can't be had. Only a link to an installer on the file drop is ever offered, whatever the file says. A build of the repo never asks, and `GUESTHOUSE_UPDATE_CHECK=0` turns it off. The preview's `update` state shows the card.

Closing the window ends the app, and the machines go on as they were. A panic leaves its words in `guesthouse-app\panic.txt` in your temporary folder, as the app has no console.

## Installing

`scripts/windows-setup.ps1` builds the service, the serial relay and `hcs-boot`, and packs them into one file. It adds the app if it finds it, and says so if it doesn't. It adds remoteman too, for [a Windows guest's screen](#a-windows-guests-screen):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows-setup.ps1
```

It leaves two files in `target\installer\`, the folder to publish for testers at `https://files.ruxn.com/p/guesthouse/`: `guesthouse-setup-VERSION.exe`, where VERSION is the Windows host's, in `windows/version.txt`, and `guesthouse-setup-VERSION.exe.sha256` beside it, its SHA-256 as `sha256sum` writes one (`<hex>  <file>`), so `sha256sum -c` checks a download. The script publishes nothing. `scripts/windows-release.ps1` releases it, as [RELEASING.md](../RELEASING.md#the-windows-host) says.

remoteman isn't built here. The installer carries remoteman's own release files, the version `windows/remoteman.json` pins: remoteman's host's installer, `remoteman-host-setup-V.exe`, and its Windows viewer, `remoteman-viewer-windows-V.zip`. The pin names each file and its SHA-256, and the place they're published, `https://files.ruxn.com/p/remoteman/`, where each has its `.sha256` beside it. The script downloads both into `target\remoteman-cache\V\` and checks each against the pin. A file that doesn't match is refused and not kept, and a cached file that doesn't match is downloaded again. With no network and nothing cached, the build stops and says so: it never builds an installer without the screen unless asked. In the payload, and so in the install folder:

- `screen\remoteman-host-setup.exe` is the host's installer as it is. The service runs it in each Windows guest.
- `viewer\` is the viewer's zip unpacked, `remoteman.exe` in it, with FFmpeg's DLLs, the Windows App Runtime, remoteman's licence and FFmpeg's. The runtime is the same as the app's, so the installer carries it once.
- `GPL-3.0.txt`, from `windows\installer\`, is the GPL 3 that FFmpeg's LGPL 3 builds on.
- The end of the installed `THIRD-PARTY.md` says which remoteman the installer carries and which licence files came with it.

`-RemotemanFrom FOLDER` takes the two files from a folder instead of downloading them, still checked against the pin. `-RemotemanUnpinned` skips the check, for trying a local build of remoteman only, so it works only with `-RemotemanFrom`, never on a download or the cache. It says so at the start and the end of the build and in the installed `THIRD-PARTY.md`, and it takes the newest `remoteman-host-setup-*.exe` and `remoteman-viewer-windows-*.zip` when the pin's names aren't there. Never hand such an installer to anyone. `-NoScreen` builds without remoteman. An installer without it still installs everything else, and installed over one with it, it removes the earlier screen's files.

The app starts the viewer as `viewer\remoteman.exe` in the install folder, `C:\Program Files\Guesthouse\viewer\remoteman.exe`. From the app's own `C:\Program Files\Guesthouse\app\guesthouse-app.exe` that's `..\viewer\remoteman.exe`.

Opening the file asks for an administrator once, then shows each step. It copies the files into `C:\Program Files\Guesthouse`, which only administrators can write to, and `setup.ps1` does the rest:

- It checks for Windows 11, and for Virtual Machine Platform. If that's off it turns it on, and the last page says Windows has to restart before a machine can run. It never turns on Hyper-V.
- It registers the service `Guesthouse`, which starts with Windows, as the system. Windows is told to wait up to 90 s for it at shutdown, so it has time to ask each machine to shut down. It's started again if it ends.
- The app gets a shortcut in Start, and the uninstaller is in Settings > Apps.

An install over an earlier one stops the service first, which asks each running machine to shut down and can take a minute. remoteman's viewer is closed if it's open from the install folder, as the app is. `app\`, `viewer\` and `screen\` keep only the new install's files: what the earlier one had there and this one doesn't is deleted. It never ends the service by force, as that would cut a machine off while it's shutting down. If the service doesn't stop in 90 s, the step fails and says why.

The uninstaller, in Settings > Apps or `uninstall.exe` in the install folder, removes the service and the files, remoteman's with them. What remoteman's host installed in a guest stays in the guest. Your machines in `%ProgramData%\Guesthouse` stay, so installing again finds them, unless you tick Also delete my machines. Virtual Machine Platform stays on.

`--silent` has no pages, `--uninstall --delete-machines` deletes the machines too, and `--dry-run` shows every page and walks every step with no administrator and no change to the PC. Its log is `%TEMP%\guesthouse-setup.log`, and the install's own is `setup.log` in the install folder. `crates/installer-windows/test/walk.ps1` drives the pages in a dry run and takes a picture of each.

## A Windows guest's screen

A Windows guest's screen is [remoteman](https://github.com/ruxnhq/remoteman)'s: its host runs in the guest and encodes the guest's desktop on the guest's card, and its viewer runs on this PC. Guesthouse holds both ends, so it pairs them itself, and nobody is asked to:

- `boot` looks for remoteman's host's installer, `screen\remoteman-host-setup.exe` from remoteman's release, beside itself, beside the service or `guesthouse-hcs-boot`. When it's there, every boot puts it on the guest's disk with `guest/screen.ps1`, and the guest's report runs that script as SYSTEM. The script runs the installer unattended: `remoteman-host-setup.exe --silent --trust FILE --encoder NAME --no-pair --log FILE`, again whenever the installer, the viewer to trust or the encoder changes. remoteman's installer does all of remoteman's own work: where its files go, its service and its command line, its trust list. Run again, it keeps the host's identity, so its fingerprint stays the same.
- The viewer's identity is made on the host, as remoteman makes its own, beside the machine's disk in `NAME.screen`. The guest's installer is told to trust only that, and `--no-pair` leaves its host no way to pair anyone else. The guest asks its host for its fingerprint with `remoteman-host.exe fingerprint --state C:\ProgramData\remoteman\host`.
- The guest says its host's fingerprint on its serial port, and `boot` keeps it in the machine's record. The service's `Screen` answer gives the app the guest's address and fingerprint and the viewer's identity, and the app writes the identity into a state folder of the user's and starts the viewer with remoteman's `connect` command, which takes the address, the fingerprint to expect, the identity and `--placement main`.
- The guest signs in to its own account by itself at every start, as remoteman's host shows a sign-in screen otherwise. Its password is kept in its own registry, where Windows keeps an automatic sign-in's, and it has no lock screen and doesn't lock when idle. That starts from the boot after the first one with the installer.
- remoteman's host picks NVIDIA's encoder itself. AMD's and Intel's encoders for Media Foundation aren't registered in a guest, so with their cards it's told to use Direct3D 12's video encoding, which both have.

The installer puts remoteman's release in place, as [Installing](#installing) says: the host's installer at `screen\remoteman-host-setup.exe` beside the service, and the viewer at `viewer\remoteman.exe`, with FFmpeg's DLLs and the Windows App Runtime beside it.

The app's Open screen does all of that. It starts the viewer at `viewer\remoteman.exe` in the folder above its own, as the installer lays them out, or at `GUESTHOUSE_VIEWER` when that's set, as when trying the app from a build. Without the app, with the service running:

```powershell
cargo run -p guesthouse-control --example screen -- NAME "C:\Program Files\Guesthouse\viewer\remoteman.exe"
```

A viewer from remoteman's own `target\release` needs FFmpeg's DLLs beside it or on `PATH`. The viewer isn't jailed yet, though it decodes what the guest sends: that's next for it.

## What it doesn't do yet

- The service hasn't run as a service yet, and the installer hasn't installed it for real: they've been tried only by their tests and a dry run, which run unelevated.
- A Windows guest gets only the driver's folder in the driver store. That's what Direct3D needs. The files a driver also puts in `System32`, which some programs look for there, aren't placed yet.
- An Ubuntu guest without WSL on the host has no `libdxcore.so`, so `nvidia-smi` and CUDA won't run in it.
- A Windows guest's console, its own display before it's set up or when its network is down, isn't in the app yet. An Ubuntu guest has no screen but SSH.
- When it fails, HCS's own answer is printed with the document that was sent. That answer names what it refused.
