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
          programs.guesthouse = {
            enable = true;
            users = [ "alice" ];
          };
        }
      ];
    };
  };
}
```

Rebuild, then log in again so the group takes effect. Guesthouse is in the menu, and `guesthouse` starts it.

## Options

- `programs.guesthouse.enable` turns it all on.
- `programs.guesthouse.package` is the package. It defaults to the one in this flake.
- `programs.guesthouse.users` are the users who may use `/dev/kvm`. They join the `kvm` group.
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

Use the open kernel modules, as GPU-PV needs them.

```nix
services.xserver.videoDrivers = [ "nvidia" ];
hardware.nvidia.open = true;
```

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
