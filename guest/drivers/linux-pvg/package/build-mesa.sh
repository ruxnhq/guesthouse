#!/bin/sh
# SPDX-License-Identifier: MIT
#
# Builds our Mesa for Ubuntu 26.04 in the build VM, from a commit of a Mesa
# tree, and brings the install back to the Mac: OUT/stage-COMMIT (a DESTDIR
# install with opt/pvg-mesa in it), the logs beside it. Under the lock that
# keeps Mesa builds in guests apart; the VM runs at most an hour.
#
# Usage: build-mesa.sh PROJECT_ROOT MESA_TREE COMMIT [MESON_OPTION...]
# Environment: PVG_OUT (target/linux-pvg/mesa); PVG_WAYLAND_PROTOCOLS, the
#   release tarball wayland-protocols-1.50.tar.xz; PVG_VM, PVG_VM_PORT as
#   for vm.sh. PVG_VM_UP=yes: the VM runs already (no start, no stop, no
#   lock: the caller holds it).
set -eu
root=$(cd "$1" && pwd)
tree=$2
commit=$(git -C "$tree" rev-parse --short=11 "$3")
shift 3
out=${PVG_OUT:-$root/target/linux-pvg/mesa}
protocols=${PVG_WAYLAND_PROTOCOLS:?names wayland-protocols-1.50.tar.xz}
vm="sh $root/guest/drivers/linux-pvg/package/vm.sh $root"
mkdir -p "$out"
if [ "${PVG_VM_UP:-}" != yes ]; then
	exec lockf -t 3600 "$out/mesa-guest.lock" env PVG_VM_UP=yes PVG_STARTED_VM=yes \
		sh "$0" "$root" "$tree" "$commit" "$@"
fi
if [ "${PVG_STARTED_VM:-}" = yes ]; then
	$vm start 3600
	trap '$vm stop' EXIT
fi
echo "build-mesa: $tree at $commit"
# The tree at that commit, and the wayland-protocols release Mesa wants
# (Ubuntu 26.04 has 1.47).
$vm ssh 'rm -rf ~/mesa-src ~/dl && mkdir -p ~/mesa-src ~/dl'
git -C "$tree" archive "$commit" | $vm ssh 'tar -C ~/mesa-src -xf -'
cat "$protocols" |
	$vm ssh 'cat > ~/dl/wayland-protocols-1.50.tar.xz'
# A build of another commit starts its build folders again.
$vm ssh "[ \"\$(cat ~/mesa-commit 2>/dev/null)\" = $commit ] || rm -rf ~/build-mesa; echo $commit > ~/mesa-commit"
status=0
$vm ssh sh -s -- "$@" < "$root/guest/drivers/linux-pvg/package/vm-mesa.sh" > "$out/build-$commit.log" 2>&1 || status=$?
grep "vm-mesa\|error" "$out/build-$commit.log" | tail -60
for log in clc-setup clc-ninja mesa-setup mesa-ninja mesa-install; do
	$vm ssh "cat ~/$log.log" > "$out/$log-$commit.log" 2>/dev/null || true
done
[ $status = 0 ] || { echo "build-mesa: failed, see $out/build-$commit.log" >&2; exit 1; }
rm -rf "$out/stage-$commit"
mkdir -p "$out/stage-$commit"
$vm ssh 'tar -C ~/mesa-stage -cf - .' | tar -C "$out/stage-$commit" -xf -
echo "build-mesa: $out/stage-$commit"
