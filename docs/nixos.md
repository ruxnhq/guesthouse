# Guesthouse on NixOS

The flake gives you the package and a NixOS module. The module sets up what `install.sh` sets up on other
distributions.

The package is the release, repackaged and patched to run on NixOS. It isn't built from source, as a
source build has no Windows display driver. That needs Windows and Microsoft's kit.

NixOS support is new. It's tested in a NixOS VM, but not yet on a PC with a GPU. If something doesn't
work, please open an issue.

## Setup

Add the flake as an input, and the module to your system.

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    guesthouse.url = "github:xnixy/guesthouse";
    guesthouse.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, guesthouse, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        guesthouse.nixosModules.default
        {
          programs.guesthouse.enable = true;
        }
      ];
    };
  };
}
```

Rebuild. Guesthouse is in the menu, and `guesthouse` starts it. Whoever is logged in at the desktop can start
machines, so nobody needs to be listed. `/dev/kvm` is open to everyone on NixOS, and systemd gives `/dev/udmabuf`,
which a machine on an AMD card needs, to whoever is logged in.

## Options

- `programs.guesthouse.enable` turns it all on.
- `programs.guesthouse.package` is the package. It defaults to the one in this flake.
- `programs.guesthouse.users` are users who start machines without being logged in at the desktop, as over SSH.
  They join the `kvm` group. Log in again after adding one, so the group takes effect.
- `programs.guesthouse.bridges` are the bridges a machine may attach to, such as `[ "br0" ]`. They go into
  `/etc/qemu/bridge.conf`, which the bridge helper reads. With none, the file isn't written.

## What the module does

- Installs the package, and `hwdata`, which the app reads card names from.
- Puts the bridge helper at `/run/wrappers/bin/guesthouse-bridge-helper`, setuid root. It has its own name so it
  doesn't clash with the one `virtualisation.libvirtd` makes.
- Turns on `hardware.graphics`, unless you set it yourself. QEMU loads the host's OpenGL libraries.
- Writes two firmware descriptors to `/etc/qemu/firmware`, from nixpkgs' OVMF. Both have Secure Boot. One has
  Microsoft's keys enrolled, which Windows 11 wants.

## NVIDIA

Guesthouse needs NVIDIA's open kernel modules, from a driver branch it has been tested on: 615 or 620.
NixOS 26.05's own drivers stop at 595, so pin a tested version:

```nix
{ config, ... }:
{
  services.xserver.videoDrivers = [ "nvidia" ];
  hardware.nvidia.open = true;
  hardware.nvidia.package = config.boot.kernelPackages.nvidiaPackages.mkDriver {
    version = "615.78.08";
    sha256_64bit = "sha256-Pj9t3cLudnoIGFMAr3vjyyhuznZpjS3eNSRZl4LQf/4=";
    openSha256 = "sha256-HBINiOjL0ZJLIAJeNIBYHBnwgUXtNwPPtnFpAI1YwF4=";
    settingsSha256 = "sha256-inDRpG02sdDgHmlqgu/DsgK8OFdOt1fZIYyXdhlGC/c=";
    persistencedSha256 = "sha256-RzeR6Ldct6MUxjnXRyThdh5Y3jjMehTVg85MtwuWNX4=";
  };
}
```

On nixos-unstable, `nvidiaPackages.new_feature` was 615.78.08 when this was written, with no hashes to copy.
It moves with nixpkgs, so check the version it gives you.

With a driver from another branch, plain machines still work, and the module says so when you rebuild. A
Linux machine starts without its GPU and tells you why. A Windows machine's GPU hasn't been tried on one.

The package finds the driver's libraries through `/run/opengl-driver/lib`, which `hardware.graphics` fills
with the driver NixOS has installed.

An AMD card needs nothing more, as `amdgpu` is NixOS's default.

## Updates

The app doesn't update this package itself, as the Nix store is read-only. Update the flake input instead,
then rebuild:

```sh
nix flake update guesthouse
```

The flake moves to each release once it's out, so a new release can take a little while to reach it.

## Testing a machine start

Two tests are run by hand, as they need nested KVM, which CI may not have.

- `nix build .#legacyPackages.x86_64-linux.machine-test` boots NixOS in a VM, makes a plain machine and starts it.
- `nix build .#legacyPackages.x86_64-linux.jail-test` runs a small QEMU in the jail the app builds for a GPU
  machine. A GPU machine can't be made without a GPU, so the test puts the jail together itself.
