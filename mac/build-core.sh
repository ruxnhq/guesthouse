#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# Builds the Rust core for macOS on Apple silicon and wraps it as an XCFramework that Swift
# imports as `GuesthouseCore`. Run from anywhere; the output is build/GuesthouseCore.xcframework.
set -e
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"
export PATH="$HOME/.cargo/bin:$PATH"
# The same minimum as the app, so the linker has nothing to warn about.
export MACOSX_DEPLOYMENT_TARGET=27.0
target=aarch64-apple-darwin
# Keep the builder's home folder out of the paths a panic message carries.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~"

out=build/GuesthouseCore.xcframework
headers=build/core-headers
rm -rf "$out" "$headers"
mkdir -p "$headers"
cp host/include/guesthouse_core.h "$headers/"
cat > "$headers/module.modulemap" <<EOF
module GuesthouseCore {
    header "guesthouse_core.h"
    export *
}
EOF

# The remote-view crate's sentences, as data, from the pinned crate: the app and its tests read them.
manifest=host/Cargo.toml
cargo metadata --format-version 1 --manifest-path "$manifest" \
    | python3 -c 'import json,sys,os; m=json.load(sys.stdin); p=[x for x in m["packages"] if x["name"]=="guesthouse-remote-view-core"][0]; print(os.path.join(os.path.dirname(p["manifest_path"]),"wording.json"))' \
    > build/wording.path
cp "$(cat build/wording.path)" build/wording.json
rm build/wording.path

cargo build --release --manifest-path host/Cargo.toml --target "$target"
xcodebuild -create-xcframework \
    -library "../target/$target/release/libguesthouse_core.a" -headers "$headers" \
    -output "$out"
