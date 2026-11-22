#!/bin/sh
# Builds the NVAPI wrapper for Windows guests and puts it at the path given, or in guest/, where a
# build of the repo's app takes it from. It needs Rust's x86_64-pc-windows-gnu target and MinGW's
# linker: `rustup target add x86_64-pc-windows-gnu` and the mingw-w64 package on Debian and Ubuntu,
# mingw64-gcc and rust-std-static-x86_64-pc-windows-gnu on Fedora.
set -eu
repo=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$repo/guest/nvapi64.dll}
target=x86_64-pc-windows-gnu
(cd "$repo" && cargo build --release --locked --target "$target" -p guesthouse-nvapi-shim)
built=${CARGO_TARGET_DIR:-$repo/target}/$target/release/nvapi64.dll
mkdir -p "$(dirname "$out")"
# Without the line tables the release profile keeps, as nobody debugs it from the guest.
x86_64-w64-mingw32-strip -o "$out" "$built"
echo "the NVAPI wrapper is $out"
