#!/bin/sh
# SPDX-License-Identifier: MIT
#
# Builds pvgfb.efi and wraps it in a PCI option ROM, pvgfb.rom.
#
#   sh mac/pvgfb/build.sh [--out FOLDER] [--helpers FOLDER]
#
#   --out FOLDER      where pvgfb.efi and pvgfb.rom go (default mac/build/pvgfb)
#   --helpers FOLDER  also copy the ROM to FOLDER/Resources/qemu, where mac/nightly.sh --helpers
#                     takes it into the app, beside QEMU's firmware
#
# Needs clang (Xcode's), lld-link (Homebrew's lld; LLD_LINK names another) and python3.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
out=$here/../build/pvgfb
helpers=
while [ $# -gt 0 ]; do
    case $1 in
        --out) out=${2:?--out wants a folder}; shift 2 ;;
        --helpers) helpers=${2:?--helpers wants a folder}; shift 2 ;;
        *) echo "build.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done
lld_link=${LLD_LINK:-$(command -v lld-link || echo /opt/homebrew/bin/lld-link)}
[ -x "$lld_link" ] || { echo "build.sh: lld-link is not installed (brew install lld, or set LLD_LINK)" >&2; exit 1; }

mkdir -p "$out"
out=$(cd "$out" && pwd)

# The Windows target gives a PE/COFF object, which is what UEFI loads. Floating point and SIMD
# registers are left alone, as the firmware does not save them for drivers.
cflags="-target aarch64-unknown-windows -std=c11 -ffreestanding -fshort-wchar
	-fno-stack-protector -fno-builtin -fno-strict-aliasing
	-mgeneral-regs-only -O2 -Wall -Wextra -Werror"

objects=
for file in driver device gop name lib; do
	# shellcheck disable=SC2086
	clang $cflags -c "$here/$file.c" -o "$out/$file.o"
	objects="$objects $out/$file.o"
done

# shellcheck disable=SC2086
"$lld_link" /nologo /machine:arm64 /subsystem:efi_boot_service_driver \
	/entry:PvgfbEntry /nodefaultlib /out:"$out/pvgfb.efi" $objects

python3 "$here/mkrom.py" "$out/pvgfb.efi" "$out/pvgfb.rom"
echo "built $out/pvgfb.efi and $out/pvgfb.rom"

if [ -n "$helpers" ]; then
    mkdir -p "$helpers/Resources/qemu"
    cp "$out/pvgfb.rom" "$helpers/Resources/qemu/pvgfb.rom"
    echo "put pvgfb.rom in $helpers/Resources/qemu"
fi
