#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
#
# Guesthouse's Linux driver package for Ubuntu 26.04 arm64: installs the
# kernel driver (pvg-dkms), our Mesa (pvg-mesa) and the desktop
# (guesthouse-desktop) from the folder it lies in, with what they need from
# Ubuntu's archive. Run as root, with the network. On /dev/ttyAMA0 it says
# exactly one of
#   guesthouse: driver installed
#   guesthouse: driver failed CODE
# and exits 0 or 1. The desktop starts at the next boot, which then says
# "guesthouse: first start ok" once weston is up.
#
# The desktop is weston, or Ubuntu's GNOME (guesthouse-gnome) with
# "install.sh --gnome" or GUESTHOUSE_DESKTOP=gnome, or KDE Plasma
# (guesthouse-plasma) with "install.sh --plasma" or GUESTHOUSE_DESKTOP=plasma,
# and from then on at every later install, since /etc/guesthouse/desktop
# says which.
#
# CODE is one of:
#   root      not run as root
#   system    not Ubuntu 26.04 on arm64
#   files     a file of the package is missing or its sum is not the manifest's
#   network   Ubuntu's archive could not be reached in 3 minutes
#   headers   the running kernel's headers could not be installed
#   packages  apt could not install the three packages and their dependencies
#   module    DKMS did not build or install pvg.ko for the running kernel
#   account   no account to start the desktop for
#
# Its log is /var/log/guesthouse/driver-install.log. When it went well the
# package's version is in /var/lib/guesthouse/driver-version.
set -u
here=$(cd "$(dirname "$0")" && pwd)
version=@VERSION@
log=/var/log/guesthouse/driver-install.log
serial() { echo "guesthouse: $*" > /dev/ttyAMA0 2>/dev/null || true; }
note() { echo "install.sh: $*"; }
fail() {
	note "failed: $1${2:+, $2}"
	serial "driver failed $1"
	exit 1
}
[ "$(id -u)" = 0 ] || { echo "install.sh: run it as root" >&2; serial "driver failed root"; exit 1; }
mkdir -p /var/log/guesthouse /var/lib/guesthouse
exec >> "$log" 2>&1
note "$(date -u '+%Y-%m-%d %H:%M:%S') version $version from $here"

. /etc/os-release
[ "${ID:-}" = ubuntu ] && [ "${VERSION_ID:-}" = 26.04 ] && [ "$(dpkg --print-architecture)" = arm64 ] ||
	fail system "${PRETTY_NAME:-unknown} $(dpkg --print-architecture 2>/dev/null)"

# The files, against the manifest beside them.
manifest=$here/guesthouse-linux-driver-latest.json
[ -f "$manifest" ] || fail files "no manifest"
work=/var/cache/guesthouse/driver-$version
rm -rf "$work"
mkdir -p "$work"
python3 - "$manifest" "$here" "$work" <<'EOF' || fail files "see above"
import hashlib, json, shutil, sys
manifest, here, work = sys.argv[1:4]
for entry in json.load(open(manifest))["files"]:
    path = f"{here}/{entry['as']}"
    digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
    if digest != entry["sha256"].lower():
        sys.exit(f"{entry['as']}: sha256 {digest}, the manifest says {entry['sha256']}")
    shutil.copy(path, work)
EOF
# apt reads the packages as its own user, which a FAT disk may not let it.
chmod 644 "$work"/*.deb

export DEBIAN_FRONTEND=noninteractive
# At a first boot cloud-init or unattended-upgrades may hold dpkg's lock a while.
apt="apt-get -q -o DPkg::Lock::Timeout=600"
tries=0
until $apt update; do
	tries=$((tries + 1))
	[ $tries -ge 18 ] && fail network
	sleep 10
done
$apt -y install "linux-headers-$(uname -r)" || fail headers "linux-headers-$(uname -r)"
chosen=$(cat /etc/guesthouse/desktop 2>/dev/null || true)
case ${1:-} in --gnome) chosen=gnome ;; --plasma) chosen=plasma ;; esac
case ${GUESTHOUSE_DESKTOP:-} in gnome | plasma) chosen=$GUESTHOUSE_DESKTOP ;; esac
extra=
case $chosen in gnome | plasma) extra="$work/guesthouse-${chosen}_${version}_all.deb" ;; *) chosen= ;; esac
# debconf would ask which display manager when GDM and SDDM are both there.
[ "$chosen" = plasma ] && echo "sddm shared/default-x-display-manager select sddm" | debconf-set-selections
[ "$chosen" = gnome ] && echo "gdm3 shared/default-x-display-manager select gdm3" | debconf-set-selections
$apt -y install --no-install-recommends "$work"/pvg-dkms_*.deb "$work"/pvg-mesa_*.deb \
	"$work"/guesthouse-desktop_*.deb $extra || fail packages
dkms status -m pvg -v "$version" -k "$(uname -r)" | grep -q installed ||
	fail module "$(dkms status -m pvg 2>&1)"
modinfo -k "$(uname -r)" pvg > /dev/null || fail module "modinfo finds no pvg"
account=$(head -1 /etc/guesthouse/desktop-user 2>/dev/null)
desktop=guesthouse-desktop@$account.service
[ -n "$chosen" ] && desktop=guesthouse-$chosen-up@$account.service
[ -n "$account" ] && systemctl is-enabled "$desktop" > /dev/null || fail account
# The next start says "first start ok" again.
rm -f /var/lib/guesthouse/first-start-ok
echo "$version" > /var/lib/guesthouse/driver-version
note "installed for $account${chosen:+ with $chosen}, module $(modinfo -k "$(uname -r)" -F filename pvg)"
serial "driver installed"
exit 0
