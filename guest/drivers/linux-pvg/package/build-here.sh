#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
#
# Makes the Linux driver package on the Ubuntu 26.04 arm64 it runs on, a
# machine, a CI runner or a container, with no build VM: what make-package.sh
# makes, from the same scripts. vm-mesa.sh builds our Mesa, vm-debs.sh makes
# the .deb files, and DKMS builds the module once as a check. Run as root, or
# as a user who may use sudo, after vm-packages.sh (in a container with
# PVG_HEADERS=linux-headers-generic).
#
# Usage: build-here.sh PROJECT_ROOT OUT_DIR
# Environment:
#   PVG_VERSION      the package's version (version.txt)
#   PVG_MESA_TREE    a Mesa tree of one's own, PVG_MESA_COMMIT its commit (HEAD);
#                    without it lib.sh's pinned commit is fetched
#   PVG_WORK         where the build happens (target/linux-pvg/here); Mesa's
#                    build folders stay there, so a second run is quicker
#   PVG_KERNEL       the kernel whose headers the DKMS check builds against
#                    (the newest generic one installed)
set -eu
root=$(cd "$1" && pwd)
out=$2
here=$root/guest/drivers/linux-pvg/package
. "$here/lib.sh"
version=${PVG_VERSION:-$(cat "$here/version.txt")}
work=${PVG_WORK:-$root/target/linux-pvg/here}
. /etc/os-release
[ "${ID:-}" = ubuntu ] && [ "${VERSION_ID:-}" = 26.04 ] && [ "$(dpkg --print-architecture)" = arm64 ] || {
	echo "build-here: this is for Ubuntu 26.04 on arm64, not ${PRETTY_NAME:-this system}" >&2
	exit 1
}
sudo=
[ "$(id -u)" = 0 ] || sudo=sudo
start=$(date +%s)
mkdir -p "$work/home/dl"

if [ -n "${PVG_MESA_TREE:-}" ]; then
	mesa_tree=$PVG_MESA_TREE
	mesa_commit=$(git -C "$mesa_tree" rev-parse "${PVG_MESA_COMMIT:-HEAD}")
else
	mesa_tree=$work/mesa-src
	pvg_fetch_mesa "$mesa_tree"
	mesa_commit=$PVG_MESA_PIN
fi
pvg_fetch_protocols "$work/home/dl/wayland-protocols-1.50.tar.xz"
driver_commit=${GITHUB_SHA:-$(git -C "$root" rev-parse HEAD 2>/dev/null || echo unknown)}
echo "build-here: version $version, driver $driver_commit, Mesa $mesa_commit"

# vm-mesa.sh and vm-debs.sh work in their user's home, as in the VM; here
# that is a folder of the build's own.
export HOME="$work/home"
rm -rf "$HOME/mesa-src"
mkdir -p "$HOME/mesa-src"
git -C "$mesa_tree" archive "$mesa_commit" | tar -x -C "$HOME/mesa-src"
[ "$(cat "$HOME/mesa-commit" 2>/dev/null)" = "$mesa_commit" ] || rm -rf "$HOME/build-mesa" "$HOME/build-clc"
echo "$mesa_commit" > "$HOME/mesa-commit"
sh "$here/vm-mesa.sh"
echo "build-here: Mesa after $(($(date +%s) - start)) s"

rm -rf "$HOME/pkg-in"
pvg_stage_inputs "$HOME/pkg-in" "$here" "$root/guest/drivers/linux-pvg" "$HOME/mesa-stage" "$mesa_tree" "$mesa_commit" \
	"$version" "guest/drivers/linux-pvg at $driver_commit" "$(pvg_mesa_source "$mesa_tree" "$mesa_commit")"
sh "$here/vm-debs.sh"

# The check: DKMS builds and installs the module from the package, for
# Ubuntu 26.04's kernel. A container runs another kernel, so the kernel is
# named, not taken from uname.
kernel=${PVG_KERNEL:-$(find /usr/src -maxdepth 1 -name 'linux-headers-*-generic' | sed 's|.*/linux-headers-||' | sort -V | tail -1)}
[ -n "$kernel" ] || { echo "build-here: no kernel headers to check the module with" >&2; exit 1; }
$sudo dpkg -i "$HOME/pkg-out/pvg-dkms_${version}_all.deb"
$sudo dkms install -m pvg -v "$version" -k "$kernel" ||
	{ cat "/var/lib/dkms/pvg/$version/build/make.log" >&2; exit 1; }
modinfo -F vermagic "/lib/modules/$kernel/updates/dkms/pvg.ko"*
$sudo dpkg -r pvg-dkms
if dkms status -m pvg | grep -q .; then
	echo "build-here: removing pvg-dkms left something in DKMS" >&2
	exit 1
fi

rm -rf "$out"
mkdir -p "$out"
cp "$HOME"/pkg-out/*.deb "$out/"
sed "s|@VERSION@|$version|" "$here/install.sh" > "$out/install.sh"
chmod 755 "$out/install.sh"
pvg_write_manifest "$out" "$version"
ls -l "$out"
cat "$out/$pvg_manifest_name"
echo "build-here: done after $(($(date +%s) - start)) s"
