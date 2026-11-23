#!/bin/bash
# Tells dpkg the host's driver is installed, at the host's version, as a package of no files that
# provides what the distro's packages ask for when they need NVIDIA's libraries. Without it, apt meets
# a package such as Ubuntu's nvidia-cuda-toolkit by installing the distro's own NVIDIA libraries, of
# another version than the host's, beside ours: its libcuda, NVML, OpenCL ICD, udev rules and services.
# It only provides names, so it blocks nothing: installing the distro's driver on purpose still works,
# and apt takes this package away to do it. Then a boot leaves the distro's alone, and nvfwd-check says so.
# Usage:
#   driver-package.sh VERSION   at each boot with an image. Says what it did. A guest without dpkg needs
#                               none, and one where dpkg is busy gets it at the next boot. It never fails.
#   driver-package.sh --replace by hand, as root, on a boot with the GPU: puts it in place of the distro's
#                               own NVIDIA libraries and what came with them, in one go through apt, which
#                               says what it will remove and asks first. What needed them stays.
set -uo pipefail
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"

# Builds the package for a version into a folder of its own, and says where the file is.
build_package() {
    local version=$1 folder
    folder=$(mktemp -d) || return 1
    # apt reads a local package as its own user.
    chmod 755 "$folder"
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
    dpkg-deb --build --root-owner-group "$folder/package" "$folder/$driver_package.deb" > /dev/null || return 1
    echo "$folder/$driver_package.deb"
}

# The host's own version string, which the control file takes as it is.
is_version() {
    [[ $1 =~ ^[0-9]+(\.[0-9]+)*$ ]]
}

replace() {
    local version file name removals=()
    [ "$(id -u)" = 0 ] || { echo "Run it as root." >&2; exit 1; }
    version=$(value_of "$state_file" version 2>/dev/null)
    if [ "$(value_of "$state_file" gpu 2>/dev/null)" != on ] || ! is_version "$version"; then
        echo "This boot has no GPU, so the host's driver version isn't known here. Run it on a boot with the GPU." >&2
        exit 1
    fi
    for name in $(distro_driver_packages) $(distro_driver_extras); do
        removals+=("$name-")
    done
    file=$(build_package "$version") || { echo "The package couldn't be built." >&2; exit 1; }
    apt-get install --purge "$file" "${removals[@]}"
    local status=$?
    rm -rf "$(dirname "$file")"
    exit "$status"
}

command -v dpkg-deb > /dev/null && command -v dpkg > /dev/null || exit 0
[ "${1:-}" != --replace ] || replace
version=${1:-}
is_version "$version" || { echo "No package for the host's driver: '$version' isn't a version."; exit 0; }
[ "$(installed_version "$driver_package")" != "$version" ] || exit 0
others=$(distro_driver_packages)
if [ -n "$others" ]; then
    echo "No package for the host's driver: the distro's own NVIDIA libraries are installed ($others)."
    exit 0
fi
if file=$(build_package "$version") && said=$(dpkg -i "$file" 2>&1); then
    echo "dpkg now counts the host's driver $version as installed."
else
    echo "No package for the host's driver this boot, the next one tries again: $(echo "${said:-}" | tail -n 1)"
fi
[ -z "${file:-}" ] || rm -rf "$(dirname "$file")"
exit 0
