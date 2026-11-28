#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
#
# Lays out the Windows graphics driver's release package, exactly as the Mac app's installer
# expects it (mac/host/src/install/driver.rs), from files that are ALREADY SIGNED. It
# signs nothing, holds no key, and uploads nothing: it copies the files into a folder under the
# names they are published by, writes the manifest with a SHA-256 for each, and stops.
#
# The shape is the Linux and Windows releases': files named guesthouse-WHAT-VERSION-NAME, a .sha256
# beside each, and a guesthouse-windows-driver-latest.json (like guesthouse-windows-latest.json) that
# names the version and every file's SHA-256. Integrity is those SHA-256 sums over https from the
# owner's file site, which only the release workflow writes to; nothing here is signed or signs.
#
# Usage: package.sh SIGNED_DIR OUT_DIR [VERSION]
#   SIGNED_DIR  holds, signed by the release build: ruxnwddm.sys, ruxnwddm-full.inf,
#               ruxnwddm-full.cat, ruxnumd.dll, winemetal.dll, ruxndxgi.dll, ruxnd3d11.dll, and
#               the PUBLIC certificate ruxn-driver.cer. (install.ps1 comes from beside this script.)
#   OUT_DIR     is made, and gets the files as guesthouse-windows-driver-VERSION-NAME and the
#               manifest guesthouse-windows-driver-latest.json, ready to be published flat at
#               https://files.ruxn.com/p/guesthouse/ by whoever publishes releases.
#   VERSION     three numbers; without it, the first three of the INF's DriverVer.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
signed=${1:?usage: package.sh SIGNED_DIR OUT_DIR [VERSION]}
out=${2:?usage: package.sh SIGNED_DIR OUT_DIR [VERSION]}

# What the installer needs, by the name each has in the package.
names="install.ps1 ruxn-driver.cer ruxnwddm.sys ruxnwddm-full.inf ruxnwddm-full.cat ruxnumd.dll winemetal.dll ruxndxgi.dll ruxnd3d11.dll"

# A package never carries a private key, whoever made it.
for file in "$signed"/*; do
	case $(basename "$file" | tr 'A-Z' 'a-z') in
	*.pfx | *.p12 | *.pvk | *.key | *.snk)
		echo "package.sh: $file looks like a private key. A package carries the public certificate only." >&2
		exit 1
		;;
	esac
done

source_of() {
	if [ "$1" = install.ps1 ]; then echo "$here/install.ps1"; else echo "$signed/$1"; fi
}

for name in $names; do
	if [ ! -f "$(source_of "$name")" ]; then
		echo "package.sh: $(source_of "$name") is not there." >&2
		exit 1
	fi
done

version=${3:-}
if [ -z "$version" ]; then
	# DriverVer = 11/26/2026,0.3.39.0 -> 0.3.39
	version=$(sed -n 's/^DriverVer *= *[0-9/]*, *\([0-9]*\.[0-9]*\.[0-9]*\)\..*/\1/p' "$signed/ruxnwddm-full.inf" | head -n 1)
fi
case $version in
[0-9]*.[0-9]*.[0-9]*) ;;
*)
	echo "package.sh: the version is not three numbers: '$version'." >&2
	exit 1
	;;
esac

mkdir -p "$out"
manifest=$out/guesthouse-windows-driver-latest.json
printf '{"version": "%s", "files": [' "$version" >"$manifest"
first=1
for name in $names; do
	published=guesthouse-windows-driver-$version-$name
	cp "$(source_of "$name")" "$out/$published"
	sum=$(shasum -a 256 "$out/$published" | awk '{print $1}')
	# Beside each file its .sha256 in sha256sum's own format, as the Linux and Windows releases
	# publish theirs (scripts/files-publish.sh takes FILE and FILE.sha256).
	printf '%s  %s\n' "$sum" "$published" >"$out/$published.sha256"
	[ $first = 1 ] || printf ', ' >>"$manifest"
	first=0
	printf '{"name": "%s", "as": "%s", "sha256": "%s"}' "$published" "$name" "$sum" >>"$manifest"
done
printf ']}\n' >>"$manifest"
# The "latest" file is the one a release replaces (files-publish.sh --replace); every other file
# is published once and never changes, which is why each has the version in its name.
echo "package.sh: wrote $manifest and $(echo $names | wc -w | tr -d ' ') files for $version in $out"
