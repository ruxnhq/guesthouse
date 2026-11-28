# shellcheck shell=sh
# SPDX-License-Identifier: GPL-2.0-or-later
#
# What both ways of making the package share, sourced by make-package.sh
# (on a Mac, with the build VM) and build-here.sh (on Ubuntu 26.04 arm64
# itself): where our Mesa comes from, how the package's inputs are laid out,
# and the manifest the Mac app reads.

# Our Mesa: KosmicKrisp with the pvg bridge, and Zink. The package is built
# from this commit unless PVG_MESA_TREE names a tree of one's own.
PVG_MESA_REPO=https://github.com/ruxnlabs/mesa.git
PVG_MESA_PIN=ae2c437cab9593e5e6a11176da4dedf283c14d42

# Mesa wants wayland-protocols 1.50 and Ubuntu 26.04 has 1.47. The address
# and the sum are those of Mesa's own subprojects/wayland-protocols.wrap.
PVG_PROTOCOLS_URL=https://gitlab.freedesktop.org/wayland/wayland-protocols/-/releases/1.50/downloads/wayland-protocols-1.50.tar.xz
PVG_PROTOCOLS_SHA256=4460c20cfcf68b994a5ee206ed15fc19ad312f16c96075186e83b0e3f4aa48a5

# The packages in the folder, by their names in it, install.sh first.
pvg_package_files() {
	echo install.sh "pvg-dkms_$1_all.deb" "pvg-mesa_$1_arm64.deb" "guesthouse-desktop_$1_all.deb" \
		"guesthouse-gnome_$1_all.deb" "guesthouse-plasma_$1_all.deb"
}

# The name a file is published under on files.ruxn.com, which the manifest gives.
pvg_published_name() {
	echo "guesthouse-linux-driver-$1-$2"
}

pvg_manifest_name=guesthouse-linux-driver-latest.json

pvg_sha256() {
	if command -v sha256sum > /dev/null; then
		sha256sum "$1" | cut -d ' ' -f 1
	else
		shasum -a 256 "$1" | cut -d ' ' -f 1
	fi
}

# Where the package says its Mesa came from: the pinned repository, or the
# tree PVG_MESA_TREE named. Usage: pvg_mesa_source TREE COMMIT
pvg_mesa_source() {
	if [ -n "${PVG_MESA_TREE:-}" ]; then
		echo "Mesa, a tree of its builder's, at $(git -C "$1" rev-parse "$2")"
	else
		echo "Mesa from $PVG_MESA_REPO at $(git -C "$1" rev-parse "$2")"
	fi
}

# Usage: pvg_fetch_mesa DIR. The pinned commit, alone, into DIR, checked by
# its hash. A DIR that holds it already is left as it is.
pvg_fetch_mesa() {
	if [ "$(git -C "$1" rev-parse HEAD 2>/dev/null)" = "$PVG_MESA_PIN" ]; then
		return
	fi
	rm -rf "$1"
	git init -q "$1"
	git -C "$1" fetch -q --depth 1 "$PVG_MESA_REPO" "$PVG_MESA_PIN"
	git -C "$1" checkout -q --detach FETCH_HEAD
	[ "$(git -C "$1" rev-parse HEAD)" = "$PVG_MESA_PIN" ] || {
		echo "the Mesa fetched from $PVG_MESA_REPO is not $PVG_MESA_PIN" >&2
		return 1
	}
}

# Usage: pvg_fetch_protocols FILE. wayland-protocols' release, checked by its sum.
pvg_fetch_protocols() {
	if [ ! -f "$1" ] || [ "$(pvg_sha256 "$1")" != "$PVG_PROTOCOLS_SHA256" ]; then
		curl -fsSL --retry 3 -o "$1.part" "$PVG_PROTOCOLS_URL"
		mv "$1.part" "$1"
	fi
	[ "$(pvg_sha256 "$1")" = "$PVG_PROTOCOLS_SHA256" ] || {
		echo "$1 is not wayland-protocols 1.50 as Mesa names it" >&2
		return 1
	}
}

# Usage: pvg_stage_inputs IN HERE DRIVER_DIR MESA_STAGE MESA_TREE MESA_COMMIT VERSION DRIVER_SOURCE MESA_SOURCE
# Lays out in IN what vm-debs.sh makes the .deb files from: the driver's
# sources (driver/, uapi/), the Mesa install (stage/opt/pvg-mesa), this
# folder's package files (tools/), Zink's options (drirc/), Mesa's licences
# (mesa-licences/), and the version and the sources in version, sources-*.
pvg_stage_inputs() {
	# sh has no local variables, so these carry a prefix of their own.
	s_in=$1 s_here=$2 s_driver=$3 s_stage=$4 s_tree=$5 s_commit=$6
	mkdir -p "$s_in/stage" "$s_in/tools" "$s_in/drirc" "$s_in/mesa-licences"
	cp -R "$s_driver/driver" "$s_driver/uapi" "$s_in/"
	cp -R "$s_stage/opt" "$s_in/stage/"
	cp -R "$s_here/dkms" "$s_here/mesa-pkg" "$s_here/desktop" "$s_here/gnome" "$s_here/plasma" "$s_here/copyright" "$s_in/tools/"
	cp "$s_here/drirc/"*.conf "$s_in/drirc/"
	git -C "$s_tree" archive "$s_commit" docs/license.rst licenses | tar -x -C "$s_in/mesa-licences"
	echo "$7" > "$s_in/version"
	echo "$8" > "$s_in/sources-driver"
	echo "$9" > "$s_in/sources-mesa"
}

# Usage: pvg_write_manifest OUT VERSION. The manifest beside the files in OUT,
# in the shape of the Windows one: every file once, its published name, its
# name in the package and its SHA-256.
pvg_write_manifest() {
	(
		cd "$1" || exit 1
		first=yes
		printf '{"version": "%s", "files": [\n' "$2"
		for f in $(pvg_package_files "$2"); do
			[ -f "$f" ] || { echo "$f is missing" >&2; exit 1; }
			[ $first = yes ] || printf ',\n'
			first=no
			printf '  {"name": "%s", "as": "%s", "sha256": "%s"}' "$(pvg_published_name "$2" "$f")" "$f" "$(pvg_sha256 "$f")"
		done
		printf '\n]}\n'
	) > "$1/$pvg_manifest_name.part"
	mv "$1/$pvg_manifest_name.part" "$1/$pvg_manifest_name"
}

# Usage: pvg_publish_folder OUT VERSION PUBLISH. Copies the package in OUT
# into PUBLISH under the names it is published under, and the manifest.
pvg_publish_folder() {
	mkdir -p "$3"
	for f in $(pvg_package_files "$2"); do
		cp "$1/$f" "$3/$(pvg_published_name "$2" "$f")"
	done
	cp "$1/$pvg_manifest_name" "$3/"
}
