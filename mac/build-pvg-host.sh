#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# Builds guesthouse-pvg-host, the graphics process (mac/pvg-host), for macOS on Apple silicon,
# and puts it in the helpers folder that mac/nightly.sh --helpers takes, beside the QEMU that
# qemu/mac/build.sh makes: target/mac-helpers unless told otherwise. Run from anywhere. In the app
# it is signed with the hardened runtime and no entitlements, which is all it needs.
#
#   sh mac/build-pvg-host.sh [--helpers FOLDER]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$here/../target/mac-helpers
while [ $# -gt 0 ]; do
    case $1 in
        --helpers) out=${2:?--helpers wants a folder}; shift 2 ;;
        -h | --help) sed -n 3,8p "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "build-pvg-host.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done
# The folder as given, before the build moves to the repo's root.
case $out in /*) ;; *) out=$PWD/$out ;; esac
cd "$here/.."
export PATH="$HOME/.cargo/bin:$PATH"
# The same minimum as the app. The Objective-C files that build.rs compiles take it from here too.
export MACOSX_DEPLOYMENT_TARGET=27.0
target=aarch64-apple-darwin
# Keep the builder's home folder out of the paths a panic message carries.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~"

cargo build --locked --release -p guesthouse-pvg-host --target "$target"
mkdir -p "$out"
cp "target/$target/release/guesthouse-pvg-host" "$out/"
# The app takes only a build that publishes frames; the usage line says whether it does.
"$out/guesthouse-pvg-host" 2>&1 | grep -q -- '--frames' \
    || { echo "build-pvg-host.sh: this build has no --frames" >&2; exit 1; }
echo "done: $out/guesthouse-pvg-host"
