#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
#
# Makes the five .deb files in the build VM, or on the Ubuntu build-here.sh
# runs on, from what lib.sh's pvg_stage_inputs put in ~/pkg-in: the driver's
# sources (driver/, uapi/), the Mesa install (stage/opt/pvg-mesa), this
# folder's files (tools/), Mesa's licences (mesa-licences/), and the version
# and sources in version and sources-*. The packages land in ~/pkg-out.
# Usage: vm-debs.sh (it uses sudo for dpkg-deb's root-owned files)
set -eu
in=$HOME/pkg-in
out=$HOME/pkg-out
work=$HOME/pkg-work
version=$(cat "$in/version")
t=$in/tools
rm -rf "$out" "$work"
mkdir -p "$out" "$work"
fill() { sed -e "s|@VERSION@|$version|g" -e "s|@SOURCE@|$2|g" -e "s|@DEPENDS@|${3:-}|g" "$1"; }
# Usage: copyright DIR PACKAGE. Debian's copyright file, from copyright/PACKAGE.
copyright() {
	mkdir -p "$1/usr/share/doc/$2"
	cp "$t/copyright/$2" "$1/usr/share/doc/$2/copyright"
}

# pvg-dkms: the sources in /usr/src/pvg-VERSION, as DKMS wants them.
d=$work/dkms
mkdir -p "$d/DEBIAN" "$d/usr/src/pvg-$version" "$d/usr/lib/modules-load.d" "$d/etc/modprobe.d"
cp -R "$in/driver" "$in/uapi" "$d/usr/src/pvg-$version/"
fill "$t/dkms/dkms.conf" "" > "$d/usr/src/pvg-$version/dkms.conf"
cp "$t/dkms/modules-load.conf" "$d/usr/lib/modules-load.d/pvg.conf"
cp "$t/dkms/modprobe.conf" "$d/etc/modprobe.d/pvg.conf"
fill "$t/dkms/control" "$(cat "$in/sources-driver")" > "$d/DEBIAN/control"
echo /etc/modprobe.d/pvg.conf > "$d/DEBIAN/conffiles"
copyright "$d" pvg-dkms
for s in postinst prerm; do fill "$t/dkms/$s" "" > "$d/DEBIAN/$s"; chmod 755 "$d/DEBIAN/$s"; done

# pvg-mesa: /opt/pvg-mesa, and what makes it the pvg device's: glvnd's
# vendor file, offered only while a card of pvg's is there (a link in
# /etc/glvnd to /run, which egl-vendor fills from a udev rule), the Vulkan
# driver file, GBM's backend.
m=$work/mesa
mkdir -p "$m/DEBIAN" "$m/usr/share/vulkan/icd.d" "$m/usr/lib/aarch64-linux-gnu/gbm" "$m/etc/guesthouse" \
	"$m/usr/lib/pvg-mesa" "$m/usr/lib/udev/rules.d" "$m/etc/glvnd/egl_vendor.d"
ln -s /run/pvg-mesa/10_pvg.json "$m/etc/glvnd/egl_vendor.d/10_pvg.json"
cp -a "$in/stage/opt" "$m/"
# Ubuntu's libgbm serves every process (vm-mesa.sh); headers and pkg-config
# files are for builds, which the guest does not do against ours.
rm -f "$m"/opt/pvg-mesa/lib/libgbm.so*
rm -rf "$m/opt/pvg-mesa/include" "$m/opt/pvg-mesa/lib/pkgconfig" "$m/opt/pvg-mesa/share/pkgconfig"
# Mesa's own vendor file names the library by its name alone; ours by its path.
rm -f "$m"/opt/pvg-mesa/share/glvnd/egl_vendor.d/*.json
cp "$t/mesa-pkg/10_pvg.json" "$m/opt/pvg-mesa/share/glvnd/egl_vendor.d/10_pvg.json"
cp "$t/mesa-pkg/egl-vendor" "$m/usr/lib/pvg-mesa/egl-vendor"
chmod 755 "$m/usr/lib/pvg-mesa/egl-vendor"
cp "$t/mesa-pkg/71-pvg-mesa.rules" "$m/usr/lib/udev/rules.d/"
for s in postinst prerm; do cp "$t/mesa-pkg/$s" "$m/DEBIAN/$s"; chmod 755 "$m/DEBIAN/$s"; done
ln -s /opt/pvg-mesa/share/vulkan/icd.d/kosmickrisp_mesa_icd.aarch64.json \
	"$m/usr/share/vulkan/icd.d/pvg_kosmickrisp_icd.aarch64.json"
ln -s /opt/pvg-mesa/lib/gbm/dri_gbm.so "$m/usr/lib/aarch64-linux-gnu/gbm/pvg_gbm.so"
cp "$t/mesa-pkg/mesa.env" "$m/etc/guesthouse/mesa.env"
mkdir -p "$m/opt/pvg-mesa/share/drirc.d"
cp "$in/drirc/"*.conf "$m/opt/pvg-mesa/share/drirc.d/"
# The libraries of Ubuntu's that ours link, as dpkg-shlibdeps finds them.
mkdir -p "$work/shlib/debian"
printf 'Source: pvg-mesa\n\nPackage: pvg-mesa\nArchitecture: arm64\n' > "$work/shlib/debian/control"
libs=$(find "$m/opt/pvg-mesa/lib" -type f -name "*.so*")
depends=$(cd "$work/shlib" && dpkg-shlibdeps -O -l"$m/opt/pvg-mesa/lib" --ignore-missing-info $libs 2>/dev/null |
	sed -n 's/^shlibs:Depends=//p')
[ -n "$depends" ] || { echo "vm-debs: dpkg-shlibdeps found nothing"; exit 1; }
fill "$t/mesa-pkg/control" "$(cat "$in/sources-mesa")" "$depends" > "$m/DEBIAN/control"
echo /etc/guesthouse/mesa.env > "$m/DEBIAN/conffiles"
# Mesa's copyright: ours, then which commit, then Mesa's licence page and every
# licence text of its licenses/ folder, as that commit has them.
copyright "$m" pvg-mesa
(
	printf '\nBuilt from: %s\n' "$(cat "$in/sources-mesa")"
	printf '\nMesa'"'"'s docs/license.rst:\n\n'
	cat "$in/mesa-licences/docs/license.rst"
	find "$in/mesa-licences/licenses" -type f | sort | while read -r f; do
		printf '\n\nMesa'"'"'s licenses/%s:\n\n' "${f#"$in"/mesa-licences/licenses/}"
		cat "$f"
	done
) >> "$m/usr/share/doc/pvg-mesa/copyright"

# guesthouse-desktop.
g=$work/desktop
mkdir -p "$g/DEBIAN" "$g/usr/lib/systemd/system" "$g/usr/libexec/guesthouse" "$g/etc/guesthouse" "$g/etc/pam.d"
cp "$t/desktop/guesthouse-desktop@.service" "$g/usr/lib/systemd/system/"
for f in desktop-prepare weston-session first-start; do
	cp "$t/desktop/$f" "$g/usr/libexec/guesthouse/"
	chmod 755 "$g/usr/libexec/guesthouse/$f"
done
cp "$t/desktop/weston.ini" "$g/etc/guesthouse/weston.ini"
cp "$t/desktop/pam" "$g/etc/pam.d/guesthouse-desktop"
fill "$t/desktop/control" "" > "$g/DEBIAN/control"
printf '/etc/guesthouse/weston.ini\n/etc/pam.d/guesthouse-desktop\n' > "$g/DEBIAN/conffiles"
copyright "$g" guesthouse-desktop
for s in postinst prerm; do cp "$t/desktop/$s" "$g/DEBIAN/$s"; chmod 755 "$g/DEBIAN/$s"; done

# guesthouse-gnome: GNOME in place of weston, chosen by install.sh --gnome.
n=$work/gnome
mkdir -p "$n/DEBIAN" "$n/usr/lib/systemd/system" "$n/usr/libexec/guesthouse" "$n/usr/lib/environment.d" \
	"$n/etc/dconf/profile" "$n/etc/dconf/db/guesthouse.d"
cp "$t/gnome/guesthouse-gnome-prepare.service" "$t/gnome/guesthouse-gnome-up@.service" "$n/usr/lib/systemd/system/"
cp "$t/gnome/gnome-prepare" "$n/usr/libexec/guesthouse/"
chmod 755 "$n/usr/libexec/guesthouse/gnome-prepare"
cp "$t/gnome/environment.conf" "$n/usr/lib/environment.d/90-guesthouse-gnome.conf"
cp "$t/gnome/dconf-profile" "$n/etc/dconf/profile/user"
cp "$t/gnome/dconf-defaults" "$n/etc/dconf/db/guesthouse.d/10-guesthouse"
fill "$t/gnome/control" "" > "$n/DEBIAN/control"
printf '/etc/dconf/profile/user\n/etc/dconf/db/guesthouse.d/10-guesthouse\n' > "$n/DEBIAN/conffiles"
copyright "$n" guesthouse-gnome
for s in postinst prerm postrm; do cp "$t/gnome/$s" "$n/DEBIAN/$s"; chmod 755 "$n/DEBIAN/$s"; done

# guesthouse-plasma: KDE Plasma in place of weston, chosen by install.sh --plasma.
k=$work/plasma
mkdir -p "$k/DEBIAN" "$k/usr/lib/systemd/system" "$k/usr/libexec/guesthouse" "$k/usr/lib/environment.d" "$k/etc/xdg"
cp "$t/plasma/guesthouse-plasma-prepare@.service" "$t/plasma/guesthouse-plasma-up@.service" "$k/usr/lib/systemd/system/"
cp "$t/plasma/plasma-prepare" "$k/usr/libexec/guesthouse/"
chmod 755 "$k/usr/libexec/guesthouse/plasma-prepare"
cp "$t/plasma/environment.conf" "$k/usr/lib/environment.d/90-guesthouse-plasma.conf"
# kxkbrc is plasma-prepare's to write at each start, from the Mac's layout.
cp "$t/plasma/kscreenlockerrc" "$k/etc/xdg/"
fill "$t/plasma/control" "" > "$k/DEBIAN/control"
echo /etc/xdg/kscreenlockerrc > "$k/DEBIAN/conffiles"
copyright "$k" guesthouse-plasma
for s in postinst prerm; do cp "$t/plasma/$s" "$k/DEBIAN/$s"; chmod 755 "$k/DEBIAN/$s"; done

for p in dkms:pvg-dkms_${version}_all mesa:pvg-mesa_${version}_arm64 desktop:guesthouse-desktop_${version}_all \
	gnome:guesthouse-gnome_${version}_all plasma:guesthouse-plasma_${version}_all; do
	dir=$work/${p%%:*}
	sudo chown -R root:root "$dir"
	sudo dpkg-deb --root-owner-group -Zxz --build "$dir" "$out/${p#*:}.deb" > /dev/null
	sudo chown -R "$(id -u):$(id -g)" "$dir"
done
ls -l "$out"
for f in "$out"/*.deb; do dpkg-deb -I "$f" | sed -n 's/^ \(Package\|Version\|Depends\|Installed-Size\):/vm-debs: &/p'; done
