#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
#
# Makes Guesthouse's Linux driver package for Ubuntu 26.04 arm64, the folder
# the Mac app takes (GUESTHOUSE_DRIVER_DIR) and puts on a disk labelled RUXN:
#   install.sh                          installs the three below (and with --gnome or
#                                       --plasma one of the last two), says how on /dev/ttyAMA0
#   pvg-dkms_V_all.deb                  the kernel driver's sources, built by DKMS
#   pvg-mesa_V_arm64.deb                our Mesa in /opt/pvg-mesa
#   guesthouse-desktop_V_all.deb        weston at boot on the pvg device
#   guesthouse-gnome_V_all.deb          Ubuntu's GNOME in place of weston
#   guesthouse-plasma_V_all.deb         KDE Plasma in place of weston
#   guesthouse-linux-driver-latest.json the manifest: every file once, with its sum
# from the commits it names: the driver (guest/drivers/linux-pvg of this repo) and
# our Mesa (ruxnlabs/mesa at the commit lib.sh pins). The Mesa is built in
# the build VM when PVG_MESA_STAGE is not given (build-mesa.sh); the .deb
# files are made there too, and the DKMS package is built and installed there
# once as a check. build-here.sh does the same on Ubuntu itself, with no VM.
#
# Usage: make-package.sh PROJECT_ROOT OUT_DIR
# Environment:
#   PVG_VERSION      the package's version (version.txt)
#   PVG_DRIVER       the commit of guest/drivers/linux-pvg (HEAD)
#   PVG_MESA_TREE    a Mesa tree of one's own, PVG_MESA_COMMIT its commit (HEAD); without it
#                    the pinned commit is fetched into target/linux-pvg/mesa-src
#   PVG_MESA_STAGE   a Mesa install from build-mesa.sh for that commit, if there is one
#                    (target/linux-pvg/mesa/stage-COMMIT is taken when it is there)
#   PVG_VM_UP        yes: the build VM runs already
set -eu
root=$(cd "$1" && pwd)
out=$2
here=$root/guest/drivers/linux-pvg/package
. "$here/lib.sh"
version=${PVG_VERSION:-$(cat "$here/version.txt")}
driver=$(git -C "$root" rev-parse --short=7 "${PVG_DRIVER:-HEAD}")
if [ -n "${PVG_MESA_TREE:-}" ]; then
	mesa_tree=$PVG_MESA_TREE
	mesa=$(git -C "$mesa_tree" rev-parse --short=11 "${PVG_MESA_COMMIT:-HEAD}")
else
	mesa_tree=$root/target/linux-pvg/mesa-src
	pvg_fetch_mesa "$mesa_tree"
	mesa=$(git -C "$mesa_tree" rev-parse --short=11 HEAD)
fi
vm="sh $here/vm.sh $root"
stage=${PVG_MESA_STAGE:-$root/target/linux-pvg/mesa/stage-$mesa}
if [ ! -d "$stage/opt/pvg-mesa" ]; then
	protocols=$root/target/linux-pvg/wayland-protocols-1.50.tar.xz
	pvg_fetch_protocols "$protocols"
	PVG_WAYLAND_PROTOCOLS=$protocols PVG_OUT=$root/target/linux-pvg/mesa \
		sh "$here/build-mesa.sh" "$root" "$mesa_tree" "$mesa"
fi
if [ "${PVG_VM_UP:-}" != yes ]; then
	$vm start 900
	trap '$vm stop' EXIT
fi
echo "make-package: version $version, driver $driver, Mesa $mesa"

# What goes into the VM: the driver at its commit, the Mesa install, this
# folder as it is (it is packaging).
src=$(mktemp -d /tmp/pvg-src.XXXXXX)
in=$(mktemp -d /tmp/pvg-pkg.XXXXXX)
git -C "$root" archive "$driver" guest/drivers/linux-pvg/driver guest/drivers/linux-pvg/uapi | tar -x -C "$src"
pvg_stage_inputs "$in" "$here" "$src/drivers/linux-pvg" "$stage" "$mesa_tree" "$mesa" "$version" \
	"guest/drivers/linux-pvg at $driver" "$(pvg_mesa_source "$mesa_tree" "$mesa")"
rm -rf "$src"
$vm ssh 'rm -rf ~/pkg-in && mkdir ~/pkg-in'
COPYFILE_DISABLE=1 tar -C "$in" -cf - . | $vm ssh 'tar -C ~/pkg-in -xf - 2>/dev/null'
rm -rf "$in"
$vm ssh sh -s < "$here/vm-debs.sh"

# A check in the build VM: the DKMS package builds and installs the module
# for the VM's kernel, which is Ubuntu 26.04's own.
$vm ssh "sudo dpkg -i ~/pkg-out/pvg-dkms_${version}_all.deb > ~/dkms-check.log 2>&1; \
	dkms status -m pvg; modinfo -F vermagic pvg; sudo dpkg -r pvg-dkms >> ~/dkms-check.log 2>&1; \
	dkms status -m pvg | grep -q . && echo 'make-package: dkms remove left something' || true"

rm -rf "$out"
mkdir -p "$out"
$vm ssh 'tar -C ~/pkg-out -cf - .' | tar -C "$out" -xf -
sed "s|@VERSION@|$version|" "$here/install.sh" > "$out/install.sh"
chmod 755 "$out/install.sh"
pvg_write_manifest "$out" "$version"
ls -l "$out"
cat "$out/$pvg_manifest_name"
