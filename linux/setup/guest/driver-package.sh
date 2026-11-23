#!/bin/bash
# Tells dpkg the host's driver is installed, at the host's version, as a package of no files that
# provides what the distro's packages ask for when they need NVIDIA's libraries. Without it, apt meets
# a package such as Ubuntu's nvidia-cuda-toolkit by installing the distro's own NVIDIA libraries, of
# another version than the host's, beside ours: its libcuda, NVML, OpenCL ICD, udev rules and services.
# It only provides names, so it blocks nothing: installing the distro's driver on purpose still works,
# and apt takes this package away to do it. Then this leaves the distro's alone, and nvfwd-check says so.
# Usage: driver-package.sh VERSION. Says what it did. A guest without dpkg needs none, and one where
# dpkg is busy gets it at the next boot. It never fails the boot.
set -uo pipefail
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"
version=${1:-}

command -v dpkg-deb > /dev/null && command -v dpkg > /dev/null || exit 0
# The host's own version string, which the control file takes as it is.
[[ $version =~ ^[0-9]+(\.[0-9]+)*$ ]] || { echo "No package for the host's driver: '$version' isn't a version."; exit 0; }
[ "$(installed_version "$driver_package")" != "$version" ] || exit 0
others=$(distro_driver_packages)
if [ -n "$others" ]; then
    echo "No package for the host's driver: the distro's own NVIDIA libraries are installed ($others)."
    exit 0
fi

folder=$(mktemp -d) || exit 0
trap 'rm -rf "$folder"' EXIT
mkdir -p "$folder/package/DEBIAN"
cat > "$folder/package/DEBIAN/control" << EOF
Package: $driver_package
Version: $version
Architecture: $(dpkg --print-architecture)
Maintainer: Guesthouse <nvfwd@localhost>
Section: libs
Priority: optional
Provides: libcuda.so.1 (= $version), libnvidia-ml.so.1 (= $version), nvidia-opencl-icd
Description: the host's NVIDIA driver, as this guest uses it
 The guest uses the host's driver from /opt/nv/driver. This package has no files.
 It tells apt the driver is there, so apt doesn't install the distro's own.
EOF
if dpkg-deb --build --root-owner-group "$folder/package" "$folder/driver.deb" > /dev/null &&
    dpkg -i "$folder/driver.deb" > "$folder/said" 2>&1; then
    echo "dpkg now counts the host's driver $version as installed."
else
    echo "No package for the host's driver this boot, the next one tries again: $(tail -n 1 "$folder/said" 2>/dev/null)"
fi
exit 0
