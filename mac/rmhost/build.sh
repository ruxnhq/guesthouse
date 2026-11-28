#!/bin/sh
# SPDX-License-Identifier: MIT
#
# Builds rmhost, which shares a machine's screen with a remoteman viewer, and the programs that test
# it: testsource (a stand-in for the graphics device), attacker and viewcheck.
#
#   sh mac/rmhost/build.sh [--out FOLDER] [--helpers FOLDER] [--no-serve]
#
#   --out FOLDER      where everything goes (default mac/build/rmhost)
#   --helpers FOLDER  also copy rmhost into that folder, which is what mac/nightly.sh's
#                     --helpers takes to put it in the app
#   --no-serve        leave out `rmhost serve` and viewcheck, and so remoteman's core: that rmhost
#                     only records and counts, and has no remoteman code in it
#
# `rmhost serve` links remoteman's core, which is not in this repository. The one pin is
# remoteman-source.json: the version shared/remote-view/remoteman.json names for the other hosts,
# and the commit of remoteman's repository that released it. The build fetches exactly that commit
# (git checks the hash), builds the core with cargo, --locked, into the output folder, and links it.
# It is GPL-3.0-or-later, so an rmhost with `serve` in it is under that licence as a whole.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
out=$here/../build/rmhost
helpers=
serve=yes
while [ $# -gt 0 ]; do
    case $1 in
        --out) out=${2:?--out wants a folder}; shift 2 ;;
        --helpers) helpers=${2:?--helpers wants a folder}; shift 2 ;;
        --no-serve) serve=no; shift ;;
        *) echo "build.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$out"
out=$(cd "$out" && pwd)

cflags="-O2 -g -Wall -Wextra -Wno-unused-parameter -mmacosx-version-min=14.0 -I$here/../rmframes"
frameworks="-framework Foundation -framework Metal -framework IOSurface -framework CoreVideo
    -framework CoreMedia -framework VideoToolbox -framework AVFoundation -framework Security"

pin() { sed -n "s/^ *\"$1\": *\"\\([^\"]*\\)\".*/\\1/p" "$here/remoteman-source.json"; }

clang $cflags -fobjc-arc -o "$out/testsource" \
    "$here/testsource.m" "$here/../rmframes/rmframes_pub.c" "$here/../rmframes/rmframes_recv.c" \
    "$here/../rmframes/rmframes_peer.c" -framework Foundation -framework Metal -framework IOSurface \
    -framework Security
echo "built $out/testsource"
clang $cflags -o "$out/attacker" "$here/attacker.c" "$here/../rmframes/rmframes_sub.c" \
    "$here/../rmframes/rmframes_peer.c" -framework CoreFoundation -framework IOSurface -framework Security
echo "built $out/attacker"

sources="$here/main.m $here/source.m $here/encoder.m $here/../rmframes/rmframes_sub.c
    $here/../rmframes/rmframes_peer.c"
extra=
if [ "$serve" = yes ]; then
    repository=$(pin repository)
    version=$(pin version)
    commit=$(pin commit)
    # The version is the one the other hosts use, so the pins cannot drift apart.
    shared=$here/../../shared/remote-view/remoteman.json
    if [ -f "$shared" ]; then
        said=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$shared" | head -n 1)
        [ "$said" = "$version" ] || {
            echo "build.sh: remoteman-source.json says $version and shared/remote-view/remoteman.json says $said" >&2
            exit 1
        }
    fi
    src=$out/remoteman-src
    if [ "$(git -C "$src" rev-parse HEAD 2>/dev/null || true)" != "$commit" ]; then
        rm -rf "$src"
        mkdir -p "$src"
        git -C "$src" init -q
        git -C "$src" fetch -q --depth 1 "$repository" "$commit"
        git -C "$src" checkout -q FETCH_HEAD
        [ "$(git -C "$src" rev-parse HEAD)" = "$commit" ] || { echo "build.sh: fetched the wrong commit" >&2; exit 1; }
    fi
    target=$out/remoteman-target
    (
        cd "$src"
        PATH=$HOME/.cargo/bin:$PATH
        MACOSX_DEPLOYMENT_TARGET=14.0 CARGO_TARGET_DIR=$target \
            cargo build --locked --release -p remoteman-core --target aarch64-apple-darwin
    )
    printf '%s\n' "$commit" >"$out/remoteman-commit"
    sources="$sources $here/serve.m $here/pairctl.m $here/devicectl.m $here/qmp_input.c $here/hid_qcode.c"
    extra="-DRMHOST_SERVE -I$src/crates/remoteman-core/include
        $target/aarch64-apple-darwin/release/libremoteman_core.a
        -framework Security -framework SystemConfiguration -lresolv"
fi
# shellcheck disable=SC2086
clang $cflags -fobjc-arc -o "$out/rmhost" $sources $extra $frameworks
echo "built $out/rmhost"
if [ "$serve" = yes ]; then
    echo "with remoteman's core $version at $commit"
    # shellcheck disable=SC2086
    clang $cflags -o "$out/viewcheck" "$here/viewcheck.c" $extra -framework CoreFoundation
    echo "built $out/viewcheck"
fi

if [ -n "$helpers" ]; then
    mkdir -p "$helpers"
    cp "$out/rmhost" "$helpers/rmhost"
    if [ "$serve" = yes ]; then
        # remoteman's licence, the GPL 3 an rmhost with serve in it is under, goes with it.
        mkdir -p "$helpers/Resources/licences"
        cp "$src/LICENSE" "$helpers/Resources/licences/remoteman-GPL-3.0.txt"
    fi
    echo "put rmhost in $helpers"
fi
