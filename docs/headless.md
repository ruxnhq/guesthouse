# Running machines without a window

`guesthouse-vm` makes, starts and stops machines from a terminal. It has no window and doesn't need GTK, so it runs on a PC with no desktop, over ssh. It's in the same folder as the app, which is `~/.local/lib/guesthouse/current` after `install.sh`. It makes the same calls the app does, so a machine made or started here is the app's too, and the other way round.

```
guesthouse-vm list
guesthouse-vm status NAME
guesthouse-vm start NAME [--no-wait]
guesthouse-vm stop NAME
guesthouse-vm off NAME
guesthouse-vm log NAME
guesthouse-vm command NAME
guesthouse-vm snapshots NAME
guesthouse-vm take-snapshot NAME SNAPSHOT
guesthouse-vm go-back NAME [SNAPSHOT] [--yes]
guesthouse-vm delete-snapshot NAME SNAPSHOT [--yes]
guesthouse-vm idle NAME [--minutes N]
guesthouse-vm share

guesthouse-vm new NAME --iso PATH [--kind windows|ubuntu|plain] [--disk GIB] [--disk-folder PATH]
                       [--cores N] [--ram GIB] [--user NAME] [--amd-card PCI]
                       [--amd-driver PATH] [--intel-driver PATH]
                       [--remote-desktop on|off] [--ssh on|off]
                       [--set-up-for RECIPE [--yes]]
guesthouse-vm install NAME [--no-wait]
guesthouse-vm remove NAME [--yes]
guesthouse-vm recipes
guesthouse-vm recipe NAME [--no-wait]
guesthouse-vm duplicate NAME [--name NAME] [--throwaway [--delete-when-stopped]] [--no-wait]
guesthouse-vm export NAME FILE [--no-wait]
guesthouse-vm import FILE [--name NAME]

guesthouse-vm show NAME
guesthouse-vm set NAME SETTING VALUE
guesthouse-vm forward NAME add|remove tcp|udp HOST GUEST

guesthouse-vm setup [--driver PATH] [--card PCI]
```

NAME is a machine's id, or its name in any case. If a name fits two machines, it lists them, and the id picks one.

## Starting and stopping

- `list` has a line for each machine: its name, id, kind and what it's doing.
- `start` does what Start does in the app, and prints each step as the task writes it to the log. It ends when the machine is ready, with exit code 0, or prints what went wrong and exits with 1. `--no-wait` returns as soon as the start is under way, and `status` says how far it is.
- `stop` asks the guest to shut down and waits. If the guest doesn't shut down in time, the VM is turned off and it says so. `off` is Force Off: the power is cut and the guest gets no chance to save anything.
- A Windows machine with `auto-snapshot` on takes a snapshot first at a start when a change that can leave Windows unable to come up is due, as the app's Start does. See [A snapshot before a risky start](#a-snapshot-before-a-risky-start).
- `log` says where the log is and shows the end of it. The log is one file for every machine, so the lines aren't sorted by machine.

## Reading how busy a machine is

`idle` reads a running machine and says in plain words whether it would count as idle. It acts on nothing: no machine is paused or stopped, and no setting changes. It is the measuring half of stopping or parking a machine nobody uses, which does not exist yet. A reading takes a few seconds.

For a Windows machine it reads four things:

- the last keyboard or mouse input in the user's session, or that nobody is logged in
- the Remote Desktop sessions that are connected (one that was left disconnected doesn't count)
- what programs have asked Windows to stay awake, from `powercfg /requests`, for the display, the system, away mode and execution
- the guest's CPU time on this PC, as a share of one core over the few seconds the reading takes, which the host reads from QEMU's own process and needs nothing from the guest

Windows words its own output in the PC's language, so none of it is read by a word. The guest says its sessions by Windows' numbers and station names, and `powercfg`'s text is read by its layout. If that layout isn't what was expected, the reading says it could not be read.

A machine counts as idle only when every one of those was read and every one was quiet. Any one that says busy makes it busy, whatever else could not be read. A reading that could not be taken, such as an agent that did not answer, is never idle: it says it cannot tell. `--minutes N` is how old the last input must be to count as quiet, and is 30 by default. The CPU limit is a few percent of one core. Both are first guesses that no idle Windows has been measured against.

For an Ubuntu machine, a plain VM or a macOS machine, only the CPU time can be read, as nothing in them answers Guesthouse. That is not enough to call one idle, so it says it cannot tell, and says busy if the CPU time is.

The reading sends `guest/idle.ps1` through the agent. The only thing it does in Windows is to start a task in the user's session for a moment, as only a program there can see that session's input. The task, its helper script in `C:\gpupv` and its answer file in the public documents folder are removed when it is done, or at the next reading if one was cut short.

`share` prints how each card with a running machine is shared, in the words of the library page's Sharing the Card panel: the whole card, then each machine's GPU time and video memory. It looks twice, a second apart, as AMD's and Intel's engine times need two looks. On NVIDIA each machine's time is the driver's last one-second sample of its processes, and its video memory is shown twice: what the guest allocated against its cap, and what the driver charges its processes with, which is more, as it includes the daemon's own work. It only reads.

## Making a machine

The disc decides what the machine is, as it does in the app's New Machine dialog: a disc with Windows 11 Pro or Windows 11 IoT Enterprise LTSC is a Windows machine with GPU-PV, an Ubuntu server disc is Ubuntu, and anything else is a plain VM that you install by hand. `new` says which it found. `--kind` is there to say what you expect, and it refuses if the disc is something else here. A disc that would be Windows or Ubuntu on a PC with the GPU part is a plain VM on a PC without it, and `new` says so.

What you don't give, you get as the dialog would start you: the sizes for that kind, with RAM as high as this PC allows, and for Windows the NVIDIA card setup chose, or another card by its address with `--amd-card`: another NVIDIA card, or an AMD or Intel card. For an AMD card, `--amd-driver` takes AMD's installer and unpacks the card's driver from it, as the dialog does when it asks for the installer, and `--intel-driver` does the same with Intel's for an Intel card. `--amd-driver` takes the place of an AMD driver that was unpacked before, and every AMD machine on this PC uses the one that's there. Whatever else would stop the machine, like its sizes or too little room for the install, is said before either installer is read, so a `new` that's refused leaves the driver as it was. The dialog's refusals come out as they are, in plain words, and nothing is made.

A Windows machine gets Remote Desktop and no SSH, as the dialog starts it. `--remote-desktop off` and `--ssh on` change that, and [Getting in](#getting-in) says how you reach either. SSH needs OpenSSH for Windows, which comes with a release. A build from source may not have it, and then `new` refuses `--ssh on` and says why.

`--disk-folder` keeps the disk on another drive. The disk gets a folder of its own inside the one you name, and `remove` takes it with the machine. The disk's file starts small and grows as the guest fills it, so `--disk` is the most it can hold, not what it takes at once.

`new` makes the disk and its settings and stops there. A Windows or Ubuntu machine is then installed with `install`, which follows the install step by step like `start`. The guest account's password is made at the install and kept in the machine's folder. There's no way to choose it here.

`--set-up-for` makes a Windows or Ubuntu machine for a [recipe](recipes.md): it prints the whole recipe first, takes the recipe's sizes unless you typed your own, and runs the recipe's steps once the install has finished and the machine is up. `recipes` lists them, and `recipe NAME` runs a machine's steps again. A recipe of your own that has steps goes ahead only with `--yes` after you've read it.

`remove` says what it would delete, and does it when you add `--yes`. A machine whose disk is outside its folder only loses its entry, and the disk stays.

`duplicate` copies a machine that is off. It says what the copy costs, then follows the copy like `start`, and the copy is called NAME Copy unless `--name` says otherwise. `--throwaway` makes the copy a throwaway, which starts clean every time, and `--delete-when-stopped` has it deleted once it has run and stopped, as `set NAME throwaway on` does for any installed machine. [Copies](copies.md) says what a copy keeps and what's new, and how a throwaway works.

`export` writes a machine that is off to one file, for Guesthouse on another PC, and follows the task like `start`. `import` makes a machine of such a file, called what it was called there unless `--name` says otherwise, and says what it changed on the way. [Copies](copies.md) says what goes in the file and how an import checks it.

## Settings

`show` lists a machine's settings, with the port forwards. `set` changes one:

| Setting | Value | Machines |
|---|---|---|
| `cores`, `ram` | a whole number, within what this PC allows | all |
| `vram` | the video memory cap in GiB, 0 for none | Windows |
| `sound` | on or off | Windows |
| `window-speed` | on or off: whether programs in a window are shown without a copy of every frame, and on NVIDIA whether Vulkan and OpenGL present through DXGI | Windows |
| `dlss` | on or off: whether Guesthouse's NVAPI wrapper, which DLSS needs, is in System32 in place of NVIDIA's. Off for a game whose anti-cheat refuses the unsigned wrapper | Windows on an NVIDIA card |
| `start-unjailed` | on or off: on lets a machine with a GPU start unjailed where the kernel can't jail it, as "Start without a jail" does in the app. Where it can, the machine is jailed either way | all |
| `throwaway` | on or off: on makes the machine start clean every time, from its disk as it is now, as Throwaway does in the app. [Copies](copies.md) says what goes and what stays. Only while it's off and installed | all |
| `screen` | 1920x1080, 2560x1440 or 3840x2160 | Windows |
| `refresh` | Hz | Windows |
| `network` | nat, none or bridged | not Ubuntu |
| `passt` | on or off, with NAT | not Ubuntu |
| `bridge` | the bridge's name | not Ubuntu |
| `only-through` | an interface's name such as wg0, or off, with NAT | not Ubuntu |
| `dns` | an IPv4 address, or auto | not Ubuntu |
| `share` | a folder, or `none` | not Ubuntu |
| `auto-snapshot` | on or off: whether a start takes a snapshot first when it has a risky change to make | Windows |
| `remote-desktop` | on or off | Windows |
| `ssh` | on or off | Windows |

These are the settings pages' own checks, and a running machine refuses what the page greys out, which is all of them except `screen`, `refresh`, `window-speed`, `dlss`, `remote-desktop` and `ssh`. The first two take effect at once if Windows is up on the display driver, as they do in the app, and `window-speed` and `dlss` at the next start. `remote-desktop` and `ssh` reach Windows at once if it's running, and `set` waits for that and says how it went. Otherwise they reach it at the next start, or at the install of a machine that hasn't been installed yet. Everything else in a machine's settings is still the app's, such as HDR, V-Cache pinning, the clipboard, USB devices and snapshots.

`auto-snapshot` can be changed at any time, and it counts from the next start. A machine that was never told is off, and `show` says which. A throwaway takes none, whatever it says, and `show` says so.

`forward` adds or removes a port on this PC that reaches a port in the guest. It can only be changed while the machine is off.

## A snapshot before a risky start

This is off until you turn it on for a machine, with `set NAME auto-snapshot on` or the switch in its Snapshots settings. A snapshot needs the machine off, so a Windows machine with it on takes it as `start` begins, before QEMU runs. It does so only if a change is due that can leave Windows unable to come up, and no successful start has met that change yet:

- Windows is to write the driver package for its card maker's vendor ID, or this is the first start that shows that ID.
- The disk is due to move to NVMe, or is on NVMe and has not started there yet.
- A new build, or a new host driver, has files to put in the guest.
- A new build of the display driver is to go into Windows.

A start with none of these due takes none, and costs a few small reads. The snapshot is the machine's own, named for what it is before and when, such as `Automatic: before the vendor ID, 23 Nov, 14:05`, and it replaces the one before it. Snapshots you took in the app are never touched. A snapshot that can't be taken, as something else has the disk open or there is no room, is said as a note and the start goes on. A disk that isn't a qcow2 file can't hold one, so it gets none, and the app's settings say so. It also holds the machine's notes about its guest (which vendor ID it shows, and where its disk is), so going back puts them back with the disk.

If that start then fails, `start` says so and tells you the snapshot, and `go-back NAME` goes back to it. It shows what is lost and does it with `--yes`. The machine must be off. A later start that works clears the offer, and so does going back. `snapshots NAME` lists them all, with the automatic one marked.

`snapshots NAME` gives each snapshot's tag in brackets. `take-snapshot NAME SNAPSHOT` takes one by that name, as Take Snapshot does in the app. `go-back NAME SNAPSHOT` goes back to the snapshot of that name, or of that tag when two share a name, and `delete-snapshot NAME SNAPSHOT` deletes it. Both say what is lost and do it only with `--yes`. All three refuse where the app's Snapshots page does: while the machine is running, a throwaway's run included, or busy with a task, and before its install has finished. On a throwaway, which is off, they change its clean state, as [Copies](copies.md) says. Linux machines (Ubuntu) and plain machines take none: an Ubuntu machine has no firmware file for a snapshot to keep, and a plain machine has no change that the app makes to its guest.

A change made to a running Windows with no start after it, such as installing the display driver from the settings, takes no snapshot, as there is no moment to take one. Starting a machine with your own QEMU command from `command` takes none either.

## Setup

`setup` does what the first-run wizard does for GPU machines. It looks at the PC and prints each check as the wizard shows it, and stops if one failed. It then copies NVIDIA's driver files from the installer you give with `--driver`, which is the `.exe` for the version it names, and writes down the card. With more than one NVIDIA card it takes the best supported one, as the wizard does, unless you name one with `--card`. That is the card a machine runs on when it wasn't given one of its own. New Machine, a machine's settings and `new --amd-card` can give a machine any of the NVIDIA cards. An AMD or Intel card's driver isn't part of `setup`. `new` unpacks it, with `--amd-driver` or `--intel-driver`.

I couldn't run `setup` on a real card, because the PC I tried this on has none. Its checks, the copy of the driver and the choice of the card are tested with pretend cards.

## What it doesn't do yet

I've run a plain machine this way on a PC with no desktop session, from `new` to `remove`. I haven't tried a GPU machine with no window, or an install of Windows or Ubuntu with `install`.

A plain VM that you install by hand has no screen here either, and a disc that waits for a key, as Windows' does, gets no key. QEMU's own monitor socket in the machine's run folder can send one, but Guesthouse doesn't wrap that yet.

## Getting in

There's no screen. QEMU runs with no viewer, and the machine runs the same. To use Windows you go in over the network, with Remote Desktop or SSH. A new Windows machine has Remote Desktop on from its install, and SSH if you made it with `--ssh on`.

1. With the machine off, forward a port on this PC to the guest's: `guesthouse-vm forward NAME add tcp 3389 3389` for Remote Desktop, and `guesthouse-vm forward NAME add tcp 2222 22` for SSH. It takes effect at the next start. It's `hostfwd` on QEMU's own network, so it only listens on the PC the machine runs on: `127.0.0.1:3389` there goes to port 3389 in Windows.
2. Start the machine, and connect Remote Desktop to `127.0.0.1:3389` on that PC, or run `ssh -p 2222 USER@127.0.0.1` there. From another PC, tunnel it first, with `ssh -L 3389:127.0.0.1:3389 -L 2222:127.0.0.1:2222 you@host`, and connect to the same ports on your own PC.
3. Sign in as the machine's account. Its name is `gpupv` unless you chose one with `--user`, and its password is in the `windows-password` file in the machine's folder, which `show` names. Windows asks for it before the session starts. SSH also takes any key in `~/.ssh/id_*.pub` on the PC the machine runs on: the install puts those in `C:\ProgramData\ssh\administrators_authorized_keys`, and each start adds one that's new. A key you use from another PC has to go in that file too. SSH starts you in Windows' command prompt.

Windows lets Remote Desktop and SSH in from one address only: this PC's end of QEMU's private network, which is where every forward comes from. So a forward is the way in, whatever the machine's network. With `set NAME network bridged` the guest has an address on your network too, and neither answers there. If you want that, open the port in Windows' own firewall yourself. Guesthouse doesn't touch Windows' own rules.

A machine made before this version keeps what its Windows has until you choose. `set NAME remote-desktop on` turns it on, at once if the machine is running, and `show` says `not set` until then. A guest without OpenSSH gets it from this PC when you turn SSH on, so it needs no internet. Turning SSH off stops it and closes its port, and leaves the program in Windows.

Remote Desktop signs into the same session the machine's screen shows, so in the app the window shows Windows' lock screen while you're connected, and after you leave, until you sign in there again.

## Starting it by hand

`guesthouse-vm command NAME` prints what would start a machine: the daemon with its environment, then QEMU's words, as a script. It starts nothing. It is for running the machine from your own script or from libvirt, and [standalone](standalone.md) says how.

## Logging in over ssh

A VM's sockets are in `$XDG_RUNTIME_DIR/guesthouse`. If that variable isn't set, Guesthouse uses `/run/user/<your user id>` when the system has made that folder for you, and only then your state folder. So a login over ssh finds the same machines as the desktop does, as long as the folder is there. With `sudo loginctl enable-linger $USER` systemd keeps your user's folder when nobody is logged in.
