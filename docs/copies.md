# Copies of a machine

## Duplicate

Duplicate, in a machine's settings, makes a copy of a machine that is off. `guesthouse-vm duplicate NAME [--name NAME]` does the same from a terminal. The copy is a machine of its own: change it, snapshot it or delete it, and the original stays as it was.

The copy is made by a task, as a start is, so the app can close while it runs. It's built in a hidden folder and only shows up in the library once it's whole, so a copy cut short by a crash or a power cut never looks like a machine. What's left of one is taken away when the app next opens, or by `guesthouse-vm list`, the next Duplicate or the next import.

### What it costs

The disk is the copy's own. On btrfs or XFS its blocks are shared with the original's until either one writes to them, so the copy takes moments and next to no room. Anywhere else, ext4 included, the disk is copied whole, which takes as much room as the original takes now and some minutes for a large one. The question before the copy says which it will be and how many GiB. A disk on another drive is copied onto that drive, beside the original's.

### What stays the same, and what's new

The guest has to keep working, so the copy has what it relies on: the disk with its snapshots, the firmware settings, and the virtual TPM. Windows may have sealed its disk's key in the TPM, and a new one would ask for the recovery key. The account's password, the settings and the shared folder stay the same too.

What would clash while both run is new:

- The id and the folder, from the copy's name.
- The MAC address. Two machines with one MAC on a bridge each lose their connection. Windows sees the new address as a new network adapter.
- The agent's token. Each machine has a token of its own. The copy's disk still knows the original's token, so each machine is moved to a new token of its own at its next start, as soon as Windows' agent answers. Until each has started once, they share it. A throwaway's disk never keeps a change, so a throwaway copy, or a copy of a throwaway, keeps the shared token on its disk for good. Each of its runs moves to a token of the run's own as soon as the agent answers, and the original moves away from the shared one at its own next start.
- The port for viewing from other devices. Two machines can't listen on one port, so the switch and its port stay with the original: the copy starts with it off, and turning it on gives it a port of its own. remoteman in the guest is the original's, with the same fingerprint and the same paired devices, so once it's on in the copy, a device paired with the original can view the copy too.
- An Ubuntu guest's SSH host keys and machine id. An Ubuntu guest is only ever on QEMU's own NAT, so two never meet on one network, but anything in the guest keyed on the machine id, as Ubuntu Pro and management agents are, would be told one machine by both, and either could show the other's host key. Only a running guest can be changed, over SSH, so the copy's next start does it once Ubuntu is up, and the guest then restarts once, as the machine id is read at boot. Until then, for about a minute, the copy is up as the original. An import does the same on the new PC. The host forgets the host key it had learned and learns the new one, as after an install. It waits until the guest's first boot and its GPU step are over, for up to five minutes, and leaves it for the next start after that. If it doesn't work, the start says so and the next start tries again. A copy's snapshots from before hold the original's, so going back to any snapshot of a copy does it again at the next start. A machine whose guest has no SSH key of its own, from before each machine had one, keeps the original's. A throwaway's disk never keeps the change, which would cost a restart at every run, so a throwaway copy keeps the original's, and the first start once Throwaway is off makes them new.

While a change is waiting, the machine's folder has `guest.token.next`, the token the guest is to be moved to, and after a Duplicate of a machine whose change was still waiting it can have `guest.token.next.again`, an empty mark that one more change is owed once that one is done, as the copy shares the waiting token, which the original's guest may have already. A copy of a machine with no change waiting gets a waiting token of its own, and each moves in one change. Both go once the changes are done. A copy and an import plan changes of their own, so neither file goes into an export. If the guest refuses the change, the machine's card and `guesthouse-vm show` say so, with what the guest said, as until it goes through the machine shares its token with another. The next start tries again.

A copy is the same Windows, so it is as lean as the original, keeps the record of what [lean Windows](recipes.md#lean-windows) took out and what Windows put back, and Defender in it leaves out the same folders, which its settings and `show` say.

Host drives stay with the original, as two guests can't share a whole drive. Add them to the copy in its settings if it's the copy that should have them.

Ubuntu's host name stays the same, as it's the same for every Ubuntu guest Guesthouse installs, copy or not: `sudo hostnamectl set-hostname NAME` in the guest changes it. The Windows computer name stays the same too, for the same reason, and changing it would need a restart in the middle of the start. Windows' own machine id (its SID) can only be made new by Sysprep, which takes the guest back to its first-run setup, so it stays as well. Two Windows machines on one bridged network show up under one name. The TPM's own identity is the same, so a service that checks it can tell the two apart only by the rest.

A machine whose disk is outside its folder, as the first one imported from the scripts can be, makes a copy that has its disk in its own folder and can be deleted with it. A disk built on a backing file keeps reading the same backing file, the one the original's settings name: see [A disk built on another file](headless.md#a-disk-built-on-another-file).

## Throwaway machines

A throwaway starts clean every time. Turn Throwaway on in a machine's settings, under Copies, or tick it in the Duplicate question to make a copy that is one. From a terminal it's `guesthouse-vm set NAME throwaway on`, or `duplicate NAME --throwaway`. It's for trying a program you don't trust, or a driver, and getting the machine back as it was by shutting it down.

Whatever is on the machine's disk when it becomes a throwaway is its clean state. Each start writes to a layer of its own on top of the disk. The task that starts the VM makes that layer, opens it and deletes it before QEMU starts, so only QEMU and that task hold it. When QEMU ends, from the guest's own Shut Down, a Force Off, a crash or anything else, the layer goes with it. That needs nothing of the app: it can be closed, or the machine started with `guesthouse-vm`, and the next start is still clean. The firmware settings and the TPM get fresh copies at each start too, and what the app noted about the guest during a run, such as whether its display driver is in, goes back to what it was. A run keeps those copies and its agent token in a `throwaway` folder in the machine's folder, which is taken away once the run has ended, by the app's next look at the library, `guesthouse-vm list` or the next start of any machine, as a copy made to go is.

Some things stay, as they aren't the machine's:

- What the guest writes to its shared folder or to a host drive.
- Its settings, which you change in the app.
- Snapshots. Taking one, or going back to one, while the machine is off changes the clean state itself.

On a lean Windows throwaway, the app doesn't look at each start for apps Windows put back: what a run gets back is gone at the next start, and it isn't taken out again and again. Which folders Defender leaves out goes back with the clean state too, so *Scan Them Again* is refused on a throwaway: turn Throwaway off, take them back, and turn it on again. Run Again for lean Windows works for the one run.

A Windows guest's agent token is changed at each start, as soon as Windows' agent answers. Windows' own services, and any program set to start with Windows, run from boot and could read the token on the disk before that. A program you start in the guest only sees the run's own token, which no other run and no other machine takes. A throwaway made from a machine shares that machine's token on its disk until the original has started once since. A machine on SATA isn't moved to NVMe while it's a throwaway, as that change would go with the run. It moves once Throwaway is off again. For the same reason a throwaway takes no snapshot of its own before a start, and a start that fails offers no way back: the next start is clean anyway.

The machine stays in the library after a run, ready to start clean again. Delete it when you're done with it, or make it with Duplicate and ask there for the copy to go once it has stopped (`duplicate NAME --throwaway --delete-when-stopped`). Such a copy is deleted, disk and all, once it has had its VM up and is off again, by whichever comes first: the app's next look at the library, `guesthouse-vm list`, or the next start of any machine. Nothing has to stay running for it, so with the app closed it goes at the next of those. Only a copy made that way is ever deleted like this, and turning its Throwaway off keeps it.

What doesn't work yet:

- `guesthouse-vm command`, for a QEMU that you start yourself, refuses a throwaway, as that QEMU would write to the clean disk. Turn Throwaway off to use it.
- A build of Guesthouse from before throwaways can't read a throwaway machine's settings, and leaves it out of its library rather than write to its disk.
- An install, from New Machine or Start Over, writes to the disk itself. A machine can only be made a throwaway once it's installed. Start Over on a throwaway installs onto its disk, and what that install leaves is its new clean state.

## Export and import

Export, in a machine's settings under Copies, writes a machine that is off to one file, which Import Machine, in the main menu, makes a machine of on another PC running Guesthouse. From a terminal it's `guesthouse-vm export NAME FILE` and `guesthouse-vm import FILE [--name NAME]`. The export is a task, like Duplicate, so the app can close while it runs. It's written as FILE.part and only takes the name you chose once it's whole. An export that was killed leaves its FILE.part, which the next export to the same name takes away.

The file holds the machine's settings, its disk with the snapshots inside it, the firmware settings, the TPM, the snapshots' own files, and the guest's account password and agent token. A recipe the machine was made with stays here, as its steps would run in the guest on the other PC with nobody there having read them. What they installed is on the disk. A snapshot Guesthouse took by itself before a start comes in as one of yours. Keep it as safe as the machine: anyone with the file can read the guest's disk. Nothing in it names a path of this PC's. The shared folder, host drives, port forwards, a bridge, viewing from other devices and the choice to start without a jail stay behind, as they're this PC's. The new PC's own come in their place: a new MAC, NAT where it was bridged, and for Windows a card of the maker Windows was installed for. An import of a Windows machine installed for AMD or Intel is refused, plainly, on a PC with no card of that maker.

The file is one disk file's size and more, as it's the disk as it is. An export of a disk built on a backing file is refused, as the file carries one disk. Join the two first with `qemu-img commit` or `qemu-img convert`.

### What an import trusts

Nothing. A file from somewhere else can be made to look like an export, so an import checks all of it before the machine shows up:

- Each file in it must be named as a plain path inside the new machine's folder, in letters, digits and a few marks. Each is made new, for you alone, and each must have as many bytes as it said and the hash it said. One that doesn't stops the import.
- The settings are read as a machine's settings, and every one that names something of this PC, as a path, a folder, a drive, a port, a bridge or the jail does, comes from here, never from the file. A file from a newer Guesthouse, with settings this one doesn't know, is refused.
- The disk must be a qcow2 that stands alone. A disk built on another file, or keeping its data in one, could have QEMU read any file of yours, so it's refused. So is a snapshot this app didn't name, as each name becomes the name of a file. qemu-img reads the disk with limits on its memory and time.
- An agent token or a password that doesn't look like the app's own is left out, and the import says so. The guest is moved to a new agent token at its first start here.
- Whether the machine is lean Windows, and which apps that took out, are read as the app's own notes and written out again, keeping only names on lean Windows' own list. Lean Windows only ever takes out apps on that list. Its first start here looks again for any Windows put back.
- Which folders Defender leaves out of its scanning is never taken from the file: it's set in the guest, whatever a file says. So the first start of an imported Windows machine asks the guest, once the start is done, and its settings and `show` say what Defender there leaves out, with *Scan Them Again* to take it back. Until it has answered, nothing is shown. Going back to a snapshot asks again the same way.

An import cut short, by a damaged file, a full drive or the app closing, leaves nothing in the library, and the app clears what it left when it next opens. Import runs in the app, so closing the app stops it, while `guesthouse-vm import` runs until it's done.

The TPM's state is read by swtpm, which runs outside the jail, as it does for every machine. An import takes it as it is, as Windows may have sealed its disk's key in it.
