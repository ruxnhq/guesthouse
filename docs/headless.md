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
guesthouse-vm windows NAME [--json]
guesthouse-vm app NAME PROGRAM [--start-menu]
guesthouse-vm apps NAME
guesthouse-vm launcher NAME list
guesthouse-vm launcher NAME add PROGRAM
guesthouse-vm launcher NAME remove PROGRAM
guesthouse-vm share

guesthouse-vm new NAME --iso PATH [--kind windows|ubuntu|plain] [--disk GIB] [--disk-folder PATH]
                       [--cores N] [--ram GIB] [--user NAME] [--amd-card PCI]
                       [--amd-driver PATH] [--intel-driver PATH]
                       [--remote-desktop on|off] [--ssh on|off]
                       [--set-up-for RECIPE [--yes]] [--lean on|off]
                       [--skip-scanning on|off]
guesthouse-vm new NAME --kind macos [--disk GIB] [--cores N] [--ram GIB]
guesthouse-vm new NAME --kind android [--iso IMAGE] [--play on|off] [--disk GIB] [--cores N] [--ram GIB]
guesthouse-vm install NAME [--no-wait]
guesthouse-vm remove NAME [--yes]
guesthouse-vm recipes
guesthouse-vm recipe NAME [--no-wait]
guesthouse-vm duplicate NAME [--name NAME] [--throwaway [--delete-when-stopped]] [--no-wait]
guesthouse-vm export NAME FILE [--no-wait]
guesthouse-vm import FILE [--name NAME]
guesthouse-vm lean NAME [--no-wait]
guesthouse-vm scan-all NAME [--no-wait]

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

`windows` lists the app windows a running Windows machine has, one line each: the guest's handle for it, program, title, size and position of the window itself, and in brackets whether it has the focus, is minimized or hidden under an overflow cell, which popup it is of which window, and where its picture is on the guest's screen when that is not where the window is. `--json` prints the helper's last layout as it sent it, which also has each window's process id, class, the title bar's height and the width of its buttons, and whether it can be resized. It is the first step towards showing one program of a guest as a window of its own.

`app NAME PROGRAM` starts a program in a running Windows machine, by its path or name in the guest, such as `C:\Windows\notepad.exe`, and says what the helper answered. With `--start-menu` PROGRAM is the id of a Start menu program instead, as `apps` lists them. It starts the program in the guest only: the guest keeps the window where its helper parks it. `guesthouse --app` does the rest as well.

Both need `set NAME app-windows on` first, which is off for every machine until then, and both say so if it is off. With it on, QEMU gets a forward of its own on this PC, to port 8769 in the guest, as the files helper has one, and the machine needs a start for that. A machine that was started before it was turned on has no such forward, and the commands say to restart it. A helper in the user's session, `guest/app-windows.ps1` with `guest/app-windows.cs`, answers: the app sends it the machine's token first, then both sides send lines of JSON. If it isn't running, `app` starts it through the agent and leaves it running, and `windows` starts it for the look and ends it after, so no helper that a look started is left running. It takes a few seconds more then. With nobody logged in to the guest there is no session for the helper, and the command says so. The helper takes one connection at a time, and a second one replaces the first, so running either command while a window of the machine's own app windows is open makes that one connect again a moment later.

The helper leaves out what no user would call an app window. Menus, tooltips and drop-downs are windows of their own, and are listed as popups with the window they belong to.

`share` prints how each card with a running machine is shared, in the words of the library page's Sharing the Card panel: the whole card, then each machine's GPU time and video memory. It looks twice, a second apart, as AMD's and Intel's engine times need two looks. On NVIDIA each machine's time is the driver's last one-second sample of its processes, and its video memory is shown twice: what the guest allocated against its cap, and what the driver charges its processes with, which is more, as it includes the daemon's own work. It only reads.

### One step from a click to a window

`guesthouse --app NAME PROGRAM [--start-menu]` is the app's own program, not `guesthouse-vm`, as it starts the window viewer. It does everything in one go. It starts the machine if it is off, the same way `start` does, and says each step. It waits for the helper in the guest to answer, which includes a guest that has only just come up and has no session yet. It starts the program, and then makes sure the machine has one viewer for its app windows: if none is running it starts `guesthouse --app-windows NAME` for it, and if one is, it only starts the program and ends. It needs no display itself, so a failure goes to the terminal and to the log, in words. It refuses at once, before it starts anything, for a machine that is not Windows or has app-windows off.

The viewer is one for each machine, whatever starts it. It holds a lock in the machine's run folder as long as it lives, and a viewer that finds it taken says so and ends, so two launches at the same moment leave one viewer. The viewer ends when the machine stops. When the last window of a program closes the viewer stays, with no windows, and the next `guesthouse --app` for the machine finds it and reuses it.

`apps NAME` lists the Start menu programs of a running Windows machine, one line each: the id, which is the shortcut's place in the menu, and the name. Uninstallers and help files are marked. Only programs with a Start menu shortcut are in it. The Store's Notepad and Calculator are not, and are started by name or path, such as `notepad.exe`.

### Launcher entries

`launcher NAME add PROGRAM` puts a program of the machine in this desktop's menu, as `Notepad (Office PC)`, with an entry that runs `guesthouse --app`. PROGRAM is the id or the name of a Start menu program, which also gives the entry the program's icon, or else a path or name in the guest, which has none. A machine that is off cannot be asked, so there PROGRAM is taken as a path or name, and the command says so; add it again with the machine running to get the Start menu's name and icon. `list` shows the entries and `remove PROGRAM` takes one away, by its name, its program or its id.

The entries are `guesthouse-ID-PROGRAM.desktop` in `$XDG_DATA_HOME/applications` (`~/.local/share/applications`) and the icons are in `$XDG_DATA_HOME/icons/hicolor/48x48/apps`. Nothing is written anywhere else, and what the guest says of a program cannot make a file name or a second line in an entry. Removing a machine, in the app or with `remove`, removes its entries and their icons, and no other machine's.

The file's name is also the app id that the program's windows carry in the viewer, so the desktop's task bar puts the windows of one program under one entry, with that entry's name and icon. A program that has no entry of the user's gets a hidden one (`NoDisplay=true`) written by the viewer, which has the same effect and does not show in the menu. A program that has no Start menu shortcut has no icon yet.

## Making a machine

The disc decides what the machine is, as it does in the app's New Machine dialog: a disc with Windows 11 Pro or Windows 11 IoT Enterprise LTSC is a Windows machine with GPU-PV, an Ubuntu server disc is Ubuntu, and anything else is a plain VM that you install by hand. `new` says which it found. `--kind` is there to say what you expect, and it refuses if the disc is something else here. A disc that would be Windows or Ubuntu on a PC with the GPU part is a plain VM on a PC without it, and `new` says so.

An Android machine, a [preview](android.md), needs no disc. Without `--iso` it downloads Guesthouse's Android image, or uses the one downloaded before. `--iso` takes an image you built yourself, and `--play on` gives it Play services through microG.

What you don't give, you get as the dialog would start you: the sizes for that kind, with RAM as high as this PC allows, and for Windows the NVIDIA card setup chose, or another card by its address with `--amd-card`: another NVIDIA card, or an AMD or Intel card. For an AMD card, `--amd-driver` takes AMD's installer and unpacks the card's driver from it, as the dialog does when it asks for the installer, and `--intel-driver` does the same with Intel's for an Intel card. `--amd-driver` takes the place of an AMD driver that was unpacked before, and every AMD machine on this PC uses the one that's there. Whatever else would stop the machine, like its sizes or too little room for the install, is said before either installer is read, so a `new` that's refused leaves the driver as it was. The dialog's refusals come out as they are, in plain words, and nothing is made.

A Windows machine gets Remote Desktop and no SSH, as the dialog starts it. `--remote-desktop off` and `--ssh on` change that, and [Getting in](#getting-in) says how you reach either. SSH needs OpenSSH for Windows, which comes with a release. A build from source may not have it, and then `new` refuses `--ssh on` and says why.

`--disk-folder` keeps the disk on another drive. The disk gets a folder of its own inside the one you name, and `remove` takes it with the machine. The disk's file starts small and grows as the guest fills it, so `--disk` is the most it can hold, not what it takes at once.

`new` makes the disk and its settings and stops there. A Windows or Ubuntu machine is then installed with `install`, which follows the install step by step like `start`. The guest account's password is made at the install and kept in the machine's folder. There's no way to choose it here.

`--set-up-for` makes a Windows or Ubuntu machine for a [recipe](recipes.md): it prints the whole recipe first, takes the recipe's sizes unless you typed your own, and runs the recipe's steps once the install has finished and the machine is up. `recipes` lists them, and `recipe NAME` runs a machine's steps again. A recipe of your own that has steps goes ahead only with `--yes` after you've read it.

`--lean on` makes a Windows machine as [lean Windows](recipes.md#lean-windows), without the apps Windows adds for every account, and prints all it takes out and switches off first. It's off unless you say so or the recipe turns it on, and `--lean off` turns a recipe's off. `show` says `lean on` for a machine made with it, and which apps it took out, and `lean NAME` does it again in the running guest.

`--skip-scanning on` has Defender leave out of its real-time scanning the folders the recipe names, and prints what that means first. It's off unless you say it, a recipe can't turn it on, and it needs a recipe that names folders. [Recipes](recipes.md#leaving-folders-out-of-virus-scanning) says what it means and doesn't. `show` says on its `defender` line which folders Defender leaves out, for as long as it does, and `scan-all NAME` has Defender scan them again.

`remove` says what it would delete, and does it when you add `--yes`. A machine whose disk is outside its folder only loses its entry, and the disk stays.

`duplicate` copies a machine that is off. It says what the copy costs, then follows the copy like `start`, and the copy is called NAME Copy unless `--name` says otherwise. `--throwaway` makes the copy a throwaway, which starts clean every time, and `--delete-when-stopped` has it deleted once it has run and stopped, as `set NAME throwaway on` does for any installed machine. [Copies](copies.md) says what a copy keeps and what's new, and how a throwaway works.

`export` writes a machine that is off to one file, for Guesthouse on another PC, and follows the task like `start`. `import` makes a machine of such a file, called what it was called there unless `--name` says otherwise, and says what it changed on the way. [Copies](copies.md) says what goes in the file and how an import checks it.

## Settings

`show` lists a machine's settings, with the port forwards. For a Windows machine it also says how its display driver stands and which vendor ID its next start shows Windows, as the settings page does. Going back to a snapshot puts both back as that disk had them, so `show` is how to check one. `set` changes one:

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
| `app-windows` | on or off: whether the machine gets a way in for the app windows helper, and the commands `windows` and `app` work. It counts from the next start. | Windows |
| `ssh` | on or off | Windows |
| `vendor-id` | `retry`: a machine kept on Microsoft's vendor ID after starts without its GPU has its card maker's tried again at the next start. Some games will not start on Microsoft's. It can be said at any time. | Windows |

These are the settings pages' own checks, and a running machine refuses what the page greys out, which is all of them except `screen`, `refresh`, `window-speed`, `dlss`, `remote-desktop`, `ssh` and `app-windows`. `app-windows` can be changed at any time, and a running machine has it from its next start. The first two take effect at once if Windows is up on the display driver, as they do in the app, and `window-speed` and `dlss` at the next start. `remote-desktop` and `ssh` reach Windows at once if it's running, and `set` waits for that and says how it went. Otherwise they reach it at the next start, or at the install of a machine that hasn't been installed yet. Everything else in a machine's settings is still the app's, such as HDR, V-Cache pinning, the clipboard, USB devices and snapshots.

`auto-snapshot` can be changed at any time, and it counts from the next start. A machine that was never told is off, and `show` says which. A throwaway takes none, whatever it says, and `show` says so.

`forward` adds or removes a port on this PC that reaches a port in the guest. It can only be changed while the machine is off.

## A snapshot before a risky start

This is off until you turn it on for a machine, with `set NAME auto-snapshot on` or the switch in its Snapshots settings. A snapshot needs the machine off, so a Windows machine with it on takes it as `start` begins, before QEMU runs. It does so only if a change is due that can leave Windows unable to come up, and no successful start has met that change yet:

- Windows is to write the driver package for its card maker's vendor ID, or this is the first start that shows that ID.
- The disk is due to move to NVMe, or is on NVMe and has not started there yet.
- A new build, or a new host driver, has files to put in the guest.
- A new build of the display driver is to go into Windows.

A start with none of these due takes none, and costs a few small reads. The snapshot is the machine's own, named for what it is before and when, such as `Automatic: before the vendor ID, 23 Nov, 14:05`, and it replaces the one before it. Snapshots you took in the app are never touched. A snapshot that can't be taken, as something else has the disk open or there is no room, is said as a note and the start goes on. A disk that isn't a qcow2 file can't hold one, so it gets none, and the app's settings say so. It also holds the machine's notes about its guest (which vendor ID it shows, and where its disk is), so going back puts them back with the disk.

If that start then fails, `start` says so and tells you the snapshot, and `go-back NAME` goes back to it. It shows what is lost and does it with `--yes`. The machine must be off. A later start that works clears the offer, and so does going back. A start right after going back to a snapshot takes no automatic one, as the snapshot you went back to already is that disk: if the start fails, that snapshot is the one offered, and the automatic snapshot there was stays. `snapshots NAME` lists them all, with the automatic one marked.

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
