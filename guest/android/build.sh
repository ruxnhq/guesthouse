#!/bin/bash
# Builds Guesthouse's Android image: LineageOS 24 (Android 17) for VMs, as a userdebug build, with
# Guesthouse's setup app, microG and Digitalis's ARM64 translation. Runs inside the container made from
# Containerfile, with the source tree at /src. See README.md.
#
# Usage: build.sh [sync]   "sync" fetches or updates the source first; without it the tree is built as
# it is, which keeps what this script put in it.
set -euo pipefail

HERE=$(cd -- "$(dirname -- "$0")" && pwd)
SRC=/src
JOBS=${JOBS:-12}

# What goes in the image from outside the tree, each checked against its SHA-256.
MICROG_RELEASE=v0.3.18.252432
MICROG_SERVICES=com.google.android.gms-252432036.apk
MICROG_SERVICES_SHA256=1094fa60ace2367ac52bd08d1bd4f33fa8144ba930e355121b28234b8ebbf382
MICROG_COMPANION=com.android.vending-84022636.apk
MICROG_COMPANION_SHA256=3f1bde2f72bca746fbb53b61a32c2a85fcb026f886ee96d6fe9a05de9a6c6d5b
DIGITALIS_RELEASE=android17-2026-11-23-5d66dd11
DIGITALIS_ARCHIVE=digitalis-prebuilts-$DIGITALIS_RELEASE.tar.gz
DIGITALIS_SHA256=682125b347789f65559669d3cfccb6ce84b59900c9923b89d3c092a19b8e7725
# Digitalis's files that the tree's own binary_translation modules also install. Its content goes into
# those modules, and they're installed as packages, so each path has one owner.
DIGITALIS_SHARED="etc/ld.config.arm64.txt etc/binfmt_misc/arm64_dyn etc/binfmt_misc/arm64_exe etc/init/berberis.rc"

fetch() {
    local url=$1 file=$2 wanted=$3
    if [ ! -f "$file" ] || [ "$(sha256sum "$file" | cut -d' ' -f1)" != "$wanted" ]; then
        curl --fail --location --silent --show-error --retry 3 --output "$file.part" "$url"
        [ "$(sha256sum "$file.part" | cut -d' ' -f1)" = "$wanted" ] || { echo "$url is not the file it should be" >&2; exit 1; }
        mv "$file.part" "$file"
    fi
}

cd "$SRC"
if [ "${1:-}" = sync ]; then
    [ -d .repo ] || repo init -u https://github.com/LineageOS/android.git -b lineage-24.0 --git-lfs --no-clone-bundle --depth 1
    repo sync -c --prune --force-sync --fail-fast --retry-fetches=8 -j 8
fi
repo manifest -r -o "$SRC/pinned-manifest.xml"

# Guesthouse's own layer.
rm -rf vendor/guesthouse
mkdir -p vendor/guesthouse/microg
cp -a "$HERE/vendor-guesthouse/." vendor/guesthouse/
DOWNLOADS=$SRC/.guesthouse-downloads
mkdir -p "$DOWNLOADS"
fetch "https://github.com/microg/GmsCore/releases/download/$MICROG_RELEASE/$MICROG_SERVICES" \
    "$DOWNLOADS/$MICROG_SERVICES" "$MICROG_SERVICES_SHA256"
fetch "https://github.com/microg/GmsCore/releases/download/$MICROG_RELEASE/$MICROG_COMPANION" \
    "$DOWNLOADS/$MICROG_COMPANION" "$MICROG_COMPANION_SHA256"
cp "$DOWNLOADS/$MICROG_SERVICES" vendor/guesthouse/microg/GmsCore.apk
cp "$DOWNLOADS/$MICROG_COMPANION" vendor/guesthouse/microg/Companion.apk

# Digitalis, with its makefile pointed at where it sits, and its shared files given to the tree's modules.
fetch "https://github.com/DigitalisX64/digitalis_prebuilts/releases/download/$DIGITALIS_RELEASE/$DIGITALIS_ARCHIVE" \
    "$DOWNLOADS/$DIGITALIS_ARCHIVE" "$DIGITALIS_SHA256"
rm -rf vendor/digitalis/prebuilts
mkdir -p vendor/digitalis
tar --extract --gzip --file "$DOWNLOADS/$DIGITALIS_ARCHIVE" --directory vendor/digitalis
mv vendor/digitalis/digitalis-prebuilts vendor/digitalis/prebuilts
MAKEFILE=vendor/digitalis/prebuilts/digitalis-prebuilts.mk
sed -i 's|^LOCAL_PATH := $(call my-dir)|DIGITALIS_PATH := vendor/digitalis/prebuilts|; s|\$(LOCAL_PATH)/|$(DIGITALIS_PATH)/|g' "$MAKEFILE"
for file in $DIGITALIS_SHARED; do
    cp "vendor/digitalis/prebuilts/system/$file" "frameworks/libs/binary_translation/prebuilt/system/$file"
    sed -i "\\|:system/$file\$|d" "$MAKEFILE"
done
echo 'PRODUCT_PACKAGES += ld.config.arm64.txt arm64_dyn arm64_exe berberis.rc' >> "$MAKEFILE"

# The device trees' changes: the layer inherited, the native bridge declared, GRUB reading Guesthouse's
# choices. Applied once; a tree that has them already is left as it is.
for patch in "$HERE"/patches/*.patch; do
    project=$(basename "$patch" .patch | sed 's|^device_virt_|device/virt/|')
    if git -C "$project" apply --check "$patch" 2>/dev/null; then
        git -C "$project" apply "$patch"
    fi
done

export PATH="$SRC/prebuilts/sdk/tools/linux/bin/:$PATH" AB_OTA_UPDATER=false ROOMSERVICE_BRANCHES="lineage-24.0"
set +u
source build/envsetup.sh
breakfast virtio_x86_64 userdebug
m -j"$JOBS" vm-utm-zip
