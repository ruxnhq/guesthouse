# macOS machines

This is a preview, and it's only in test builds from the `macos-guest` branch. It isn't in a release.

A macOS machine gets its GPU from [reims-vgpu](https://github.com/steelbrain/reims-vgpu), which isn't mine. macOS draws with Metal, and the host runs that on Vulkan. It needs an NVIDIA or AMD card with its driver loaded, and `/dev/udmabuf`. I've only run it on an RTX 5090.

## Making one

In New Machine, choose macOS Tahoe. There's no ISO to pick.

1. The first time, the app downloads macOS support. That's reims-vgpu's QEMU and drivers, built by me from its source. It's kept in `~/.local/share/guesthouse/parts`, so an update of the app doesn't fetch it again.
2. It makes the machine's boot image, and downloads Apple's installer from Apple, about 1 GiB.
3. The machine starts into the installer. Erase the disk in Disk Utility as APFS, then choose Reinstall macOS. That downloads the rest and takes a while.
4. Once macOS is installed, take the installer out in the machine's settings.

## What works

The screen, keyboard and mouse in the app's window, sound, the network, USB devices, and the jail.

## What doesn't yet

- Shutting down from the app only works once you're logged in. At the login screen it ends in a power off.
- Only macOS Tahoe is offered.
- Nothing here has run on an AMD card.

## Apple's licence

Apple's licence for macOS only allows it on Apple hardware. Whether you run it here is up to you.
