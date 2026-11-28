#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# Checks an app that mac/nightly.sh made with --helpers: that what the app runs is inside it, signed as
# it should be, and starts. It looks at the app, or at the app inside a disk image, and changes nothing.
#
#   sh mac/Nightly/check-bundle.sh APP_OR_DMG [--lookup]
#
# It checks that
#   - Contents/Helpers has qemu-system-aarch64, guesthouse-pvg-host and rmhost, Contents/Frameworks has
#     the libraries QEMU loads, and Contents/Resources/qemu has the firmware, the framebuffer ROM and the licences;
#   - codesign --verify --deep --strict passes, every Mach-O file has the hardened runtime and all have
#     the identity the app has (the Team ID, or ad hoc together), so library validation lets QEMU load
#     its libraries;
#   - QEMU has the hypervisor entitlement and nothing else, and the graphics process and rmhost have
#     no entitlements;
#   - the bundled QEMU starts and lists the pvg-fwd device, and the graphics process says --frames;
#   - with --lookup, the app's own lookup (the core's tools and firmware search, the one the app calls)
#     finds all of it inside the bundle.
set -eu

target=${1:?usage: check-bundle.sh APP_OR_DMG [--lookup]}
lookup=no
[ "${2:-}" != --lookup ] || lookup=yes
here=$(cd "$(dirname "$0")" && pwd)
mount=
cleanup() {
    status=$?
    if [ -n "$mount" ]; then hdiutil detach -quiet "$mount" >/dev/null 2>&1 || true; rmdir "$mount" 2>/dev/null || true; fi
    exit "$status"
}
trap cleanup EXIT INT TERM HUP
case $target in
    *.dmg)
        mount=$(mktemp -d)
        hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mount" "$target"
        app=$mount/Guesthouse.app
        ;;
    *) app=$target ;;
esac
[ -d "$app/Contents" ] || { echo "check-bundle.sh: $app is not an app" >&2; exit 1; }

failed=0
check() { # DESCRIPTION, then a command
    description=$1
    shift
    if "$@" >/dev/null 2>&1; then echo "ok: $description"; else echo "FAIL: $description"; failed=1; fi
}
helpers=$app/Contents/Helpers
qemu=$helpers/qemu-system-aarch64

for program in qemu-system-aarch64 guesthouse-pvg-host rmhost; do
    check "$program is in Contents/Helpers" test -x "$helpers/$program"
done
check "QEMU's firmware is in Contents/Resources/qemu" test -s "$app/Contents/Resources/qemu/edk2-aarch64-code.fd" -a -s "$app/Contents/Resources/qemu/edk2-aarch64-vars.fd"
check "the framebuffer option ROM is in Contents/Resources/qemu, a whole number of 16 KiB" \
    test -s "$app/Contents/Resources/qemu/pvgfb.rom" -a "$(( $(wc -c <"$app/Contents/Resources/qemu/pvgfb.rom") % 16384 ))" -eq 0
check "the libraries QEMU loads are in Contents/Frameworks" test -n "$(ls "$app/Contents/Frameworks" 2>/dev/null)"
check "the notice and the licences are in the app" test -s "$app/Contents/Resources/NOTICE.txt" -a -s "$app/Contents/Resources/licences/Guesthouse-GPL-2.0.txt" -a -d "$app/Contents/Resources/qemu/licences"
check "codesign --verify --deep --strict" codesign --verify --deep --strict "$app"

# Every Mach-O file: the hardened runtime, and one identity. Ad hoc has no team, so the signature's
# kind is compared then.
identity_of() { codesign -dv "$1" 2>&1 | sed -n 's/^TeamIdentifier=//p; s/^Signature=//p' | tr '\n' ' '; }
app_identity=$(identity_of "$app")
find "$app" -type f | while IFS= read -r file; do
    file -b "$file" | grep -q 'Mach-O' || continue
    flags=$(codesign -dv "$file" 2>&1 | sed -n 's/^CodeDirectory.*flags=\(0x[0-9a-f]*\)(\(.*\)).*/\2/p')
    case $flags in *runtime*) ;; *) echo "FAIL: no hardened runtime on ${file#"$app"/}"; exit 1 ;; esac
    [ "$(identity_of "$file")" = "$app_identity" ] || { echo "FAIL: ${file#"$app"/} is signed by another identity"; exit 1; }
done && echo "ok: every Mach-O file has the hardened runtime and the app's identity" || failed=1

entitlements_of() { codesign -d --entitlements - --xml "$1" 2>/dev/null | tr -d ' \n\t'; }
case $(entitlements_of "$qemu") in
    *"<key>com.apple.security.hypervisor</key><true/>"*) echo "ok: QEMU has the hypervisor entitlement" ;;
    *) echo "FAIL: QEMU lacks the hypervisor entitlement"; failed=1 ;;
esac
other=$(entitlements_of "$qemu" | grep -o '<key>[^<]*</key>' | grep -vc 'com.apple.security.hypervisor' || true)
[ "$other" = 0 ] && echo "ok: QEMU has no other entitlement" || { echo "FAIL: QEMU has $other other entitlements"; failed=1; }
for program in guesthouse-pvg-host rmhost; do
    if [ -z "$(entitlements_of "$helpers/$program" | grep -o '<key>[^<]*</key>' || true)" ]; then
        echo "ok: $program has no entitlements"
    else
        echo "FAIL: $program has entitlements"; failed=1
    fi
done

check "the bundled QEMU starts and has the pvg-fwd device" sh -c '"$1" -device help | grep -q "\"pvg-fwd\""' sh "$qemu"
check "the graphics process says --frames" sh -c '"$1" 2>&1 | grep -q -- --frames' sh "$helpers/guesthouse-pvg-host"
check "rmhost says what it is" sh -c '"$1" 2>&1 | grep -qi rmhost' sh "$helpers/rmhost"

if [ "$lookup" = yes ]; then
    repo=$(cd "$here/../.." && pwd)
    [ -f "$repo/Cargo.toml" ] || repo=$(cd "$here/../../.." && pwd)
    check "the app's own lookup finds the helpers and the firmware inside the bundle" \
        env GUESTHOUSE_BUNDLE_APP="$app" cargo test --manifest-path "$repo/Cargo.toml" -p guesthouse-host-mac --test bundle_lookup
fi
[ "$failed" = 0 ] || { echo "check-bundle.sh: something is wrong with $app"; exit 1; }
echo "check-bundle.sh: all fine"
