#!/bin/sh
# Builds the macOS part: what a macOS machine runs on, which the app fetches the first time one is
# made. It's reims-vgpu's QEMU with its library linked in, the libraries that QEMU needs from the build
# system, the option ROM and the guest's kexts as reims-vgpu's tree has them, and reims-util. Beside it
# goes the source of all of that. It needs podman or docker and no root, and publishes nothing.
#
# Usage: packaging/build-macos-part.sh --out FOLDER
#
# The part has a number of its own and not Guesthouse's version, as it only changes when a commit below
# does. A new number means a new tag, macos-part-N, and its hash in app-core's macos_part.rs. FOLDER is
# a new one outside any home folder, as build-release.sh's is, and for the same reasons.
set -eu

PART=2
ARCH=x86_64
REIMS_UPSTREAM=https://github.com/steelbrain/reims-vgpu
REIMS_COMMIT=818868564e31f9023c074f73840de97542592c91
# The QEMU that commit's vendor/qemu names, which is checked.
REIMS_QEMU_COMMIT=010bf82714f00d0f2758671d4a81402a6d94767f
UTIL_UPSTREAM=https://github.com/steelbrain/reims-util
UTIL_COMMIT=84e5c98ae7fa6b0b68e7be690525dda29697ab34
# Acidanthera's kexts that keep the guest's cores on one clock. On a host whose TSC Linux found
# unstable, each vCPU's TSC drifts from the others', and a macOS guest with more than one core then
# never finishes booting. CpuTscSync needs Lilu, which loads before it. Release zips and licences,
# checked by hash; each is BSD-3-Clause.
LILU_VERSION=1.7.2
LILU_SHA256=53967d7dcfaab01023a33df2e969a89522f13d6654a6a56ac4711b62dabf3ab8
LILU_LICENCE_SHA256=74c3ca48189b796b0d262866e67de8efba77a71516667ce68dc3a8f13c2a3c57
TSC_VERSION=1.1.2
TSC_SHA256=bc289f780c52015ae788827b3db8e6c2ab9f512992b3a157a4a5c15df1eb4ed3
TSC_LICENCE_SHA256=4be2c86dd60442507470ed5283946b66899a794c7ca0d768460a9794a62b4ac6
QEMU_PROGRAM=qemu-system-$ARCH
# What a macOS machine's QEMU loads of QEMU's own firmware. The rest of pc-bios is for other machines.
FIRMWARE='kvmvapic.bin linuxboot_dma.bin multiboot_dma.bin pvh.bin efi-virtio.rom'
# Where efi-virtio.rom's source is, in QEMU's tree.
FIRMWARE_SOURCE=roms/ipxe

# These stay the host's, as in build-release.sh, and for its reasons.
HOST_LIBRARIES='^(linux-vdso|ld-linux|libc|libm|libdl|libpthread|librt|libresolv|libgcc_s|libpipewire-0\.3|libepoxy|libgbm|libdrm|libdrm_amdgpu|libEGL|libGL|libGLX|libGLdispatch|libOpenGL|libvulkan)[.-]'

REPO=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
NAME=guesthouse-macos-part-$PART
OUT=

say() {
    printf '%s\n' "$*"
}

die() {
    printf 'build-macos-part: %s\n' "$*" >&2
    exit 1
}

# One commit of a repository, and nothing before it.
fetch_commit() { # folder, upstream, commit
    [ ! -d "$1" ] || return 0
    git init -q "$1.new"
    git -C "$1.new" fetch -q --depth 1 "$2" "$3"
    git -C "$1.new" checkout -q FETCH_HEAD
    mv "$1.new" "$1"
}

fetch_checked() { # url, sha256, file
    [ -f "$3" ] || { curl -fsSL -o "$3.new" "$1" && mv "$3.new" "$3"; } || die "$1 couldn't be fetched."
    [ "$(sha256sum "$3" | cut -d ' ' -f 1)" = "$2" ] || die "$1 isn't what it should be."
}

fetch_clock_kexts() {
    kexts=$OUT/clock-kexts
    acidanthera=https://github.com/acidanthera
    raw=https://raw.githubusercontent.com/acidanthera
    mkdir -p "$kexts"
    fetch_checked "$acidanthera/Lilu/releases/download/$LILU_VERSION/Lilu-$LILU_VERSION-RELEASE.zip" \
        "$LILU_SHA256" "$kexts/Lilu.zip"
    fetch_checked "$raw/Lilu/$LILU_VERSION/LICENSE.txt" "$LILU_LICENCE_SHA256" "$kexts/Lilu-LICENSE.txt"
    fetch_checked "$acidanthera/CpuTscSync/releases/download/$TSC_VERSION/CpuTscSync-$TSC_VERSION-RELEASE.zip" \
        "$TSC_SHA256" "$kexts/CpuTscSync.zip"
    fetch_checked "$raw/CpuTscSync/$TSC_VERSION/LICENSE" "$TSC_LICENCE_SHA256" "$kexts/CpuTscSync-LICENSE"
}

fetch_sources() {
    fetch_clock_kexts
    fetch_commit "$OUT/reims-vgpu" "$REIMS_UPSTREAM" "$REIMS_COMMIT"
    fetch_commit "$OUT/reims-util" "$UTIL_UPSTREAM" "$UTIL_COMMIT"
    tree=$OUT/reims-vgpu
    [ -f "$tree/vendor/qemu/configure" ] ||
        git -C "$tree" submodule update -q --init --depth 1 vendor/qemu
    [ "$(git -C "$tree/vendor/qemu" rev-parse HEAD)" = "$REIMS_QEMU_COMMIT" ] ||
        die "reims-vgpu's QEMU is not the commit it should be"
    [ -f "$tree/vendor/qemu/$FIRMWARE_SOURCE/src/Makefile" ] ||
        git -C "$tree/vendor/qemu" submodule update -q --init --depth 1 "$FIRMWARE_SOURCE"
}

build_library() {
    (cd "$OUT/reims-vgpu" && CARGO_TARGET_DIR="$OUT/target" cargo xtask dist --platform vulkan)
    [ -f "$OUT/target/dist-vulkan/lib/pkgconfig/reims-vgpu.pc" ] || die "the library wasn't built."
}

# The prefix is only where the files sit in relation to each other, as QEMU finds its own from where it
# is. DT_RPATH and not DT_RUNPATH, as in build-release.sh.
build_qemu() {
    build=$OUT/qemu-build
    mkdir -p "$build"
    # shellcheck disable=SC2046 # the file holds one flag a line, and none has a space in it
    [ -f "$build/build.ninja" ] || (cd "$build" &&
        PKG_CONFIG_PATH="$OUT/target/dist-vulkan/lib/pkgconfig" "$OUT/reims-vgpu/vendor/qemu/configure" \
            --prefix=/qemu $(cat "$REPO/packaging/macos-configure-flags") \
            '--extra-ldflags=-Wl,--disable-new-dtags,-rpath,$ORIGIN/../lib')
    ninja -C "$build"
    rm -rf "$OUT/qemu-install"
    DESTDIR="$OUT/qemu-install" ninja -C "$build" install >/dev/null
}

build_util() {
    (cd "$OUT/reims-util" && CARGO_TARGET_DIR="$OUT/util-target" cargo build --release --locked)
}

library_package() {
    package=$(dpkg-query -S "$(readlink -f "$1")" 2>/dev/null | head -n 1 | cut -d : -f 1)
    [ -z "$package" ] ||
        dpkg-query -W -f '${Package} ${Version} source ${source:Package} ${source:Version}' "$package"
}

needed_names() {
    readelf -d "$1" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'
}

# The libraries to bundle: what the program needs and what those need in turn, but nothing that only
# the host's own libraries need. Prints a name and a path a line.
bundled_closure() {
    resolved=$OUT/resolved.txt
    ldd "$1" | awk '$2 == "=>" && $3 ~ /^\// { print $1, $3 }' >"$resolved"
    queue=$(needed_names "$1")
    seen=" "
    while [ -n "$queue" ]; do
        # shellcheck disable=SC2086 # names, one a word
        set -- $queue
        name=$1
        shift
        queue=$*
        case $seen in *" $name "*) continue ;; esac
        seen="$seen$name "
        if printf '%s\n' "$name" | grep -Eq "$HOST_LIBRARIES"; then
            continue
        fi
        path=$(awk -v name="$name" '$1 == name { print $2; exit }' "$resolved")
        [ -n "$path" ] || die "$name isn't on this system, and the program needs it"
        printf '%s %s\n' "$name" "$path"
        queue="$queue $(needed_names "$path" | tr '\n' ' ')"
    done
    rm -f "$resolved"
}

bundle_libraries() { # program, folder, list
    mkdir -p "$2"
    cat >"$3" <<'EOF'
# The libraries in qemu/lib aren't mine, and come with their licences. The part's source archive has
# the exact source of each. Its SOURCES.txt says what's where.
#
# Library, package, its version, and the source package it was built from:
EOF
    bundled_closure "$1" | while read -r name path; do
        cp -L "$path" "$2/$name"
        chmod u+w "$2/$name"
        printf '%s %s\n' "$name" "$(library_package "$path" || echo unknown)" >>"$3"
    done
}

write_version() {
    cat <<EOF
Guesthouse's macOS part $PART, built from:
reims-vgpu  $REIMS_UPSTREAM  $REIMS_COMMIT
its QEMU    vendor/qemu in that tree  $REIMS_QEMU_COMMIT
reims-util  $UTIL_UPSTREAM  $UTIL_COMMIT
Lilu        https://github.com/acidanthera/Lilu  $LILU_VERSION, its release zip
CpuTscSync  https://github.com/acidanthera/CpuTscSync  $TSC_VERSION, its release zip
EOF
}

copy_licences() { # folder
    tree=$OUT/reims-vgpu
    mkdir -p "$1/reims-vgpu" "$1/qemu" "$1/reims-util"
    cp -R "$tree/LICENSE" "$tree/LICENSES" "$1/reims-vgpu/"
    cp "$tree/vendor/qemu/LICENSE" "$tree/vendor/qemu/COPYING" "$tree/vendor/qemu/COPYING.LIB" "$1/qemu/"
    cp -R "$OUT/reims-util/LICENSE" "$OUT/reims-util/LICENSES" "$1/reims-util/"
    mkdir -p "$1/Lilu" "$1/CpuTscSync"
    cp "$OUT/clock-kexts/Lilu-LICENSE.txt" "$1/Lilu/LICENSE.txt"
    cp "$OUT/clock-kexts/CpuTscSync-LICENSE" "$1/CpuTscSync/LICENSE"
}

# One [[kext]] entry of the list, written as reims-vgpu's own are.
kext_entry() { # folder, name, comment
    printf '\n[[kext]]\nBundlePath = "%s.kext"\nExecutablePath = "Contents/MacOS/%s"\n' "$2" "$2"
    printf 'PlistPath = "Contents/Info.plist"\nArch = "x86_64"\nMinKernel = ""\nMaxKernel = ""\n'
    printf 'Comment = "%s"\nsha256 = "%s"\n' "$3" "$(sha256sum "$1/$2.kext/Contents/MacOS/$2" | cut -d ' ' -f 1)"
}

# Lilu and CpuTscSync go into the kexts folder, and first in its list, ahead of reims-vgpu's.
add_clock_kexts() { # kexts folder
    for name in Lilu CpuTscSync; do
        python3 -m zipfile -e "$OUT/clock-kexts/$name.zip" "$1/" || die "$name's zip couldn't be unpacked."
        [ -f "$1/$name.kext/Contents/MacOS/$name" ] || die "$name's zip has no $name.kext."
    done
    rm -rf "$1"/*.dSYM
    list=$1/set.toml
    {
        sed -n '/^\[\[kext\]\]/q;p' "$list" | sed '$ { /^$/d; }'
        kext_entry "$1" Lilu "Acidanthera's kernel patcher, which CpuTscSync needs"
        kext_entry "$1" CpuTscSync "Keeps every core's TSC in step, for hosts whose TSC is unstable"
        printf '\n'
        sed -n '/^\[\[kext\]\]/,$p' "$list"
    } >"$list.new"
    mv "$list.new" "$list"
}

stage_part() {
    part=$OUT/part
    tree=$OUT/reims-vgpu
    rm -rf "$part"
    mkdir -p "$part/qemu/bin" "$part/qemu/share/qemu"
    cp "$OUT/qemu-install/qemu/bin/$QEMU_PROGRAM" "$part/qemu/bin/"
    bundle_libraries "$OUT/qemu-build/$QEMU_PROGRAM" "$part/qemu/lib" "$part/LIBRARIES.txt"
    cp -R "$OUT/qemu-install/qemu/share/qemu/keymaps" "$part/qemu/share/qemu/"
    for blob in $FIRMWARE; do
        cp "$OUT/qemu-install/qemu/share/qemu/$blob" "$part/qemu/share/qemu/"
    done
    cp "$tree/firmware/prebuilt/ReimsGraphics.rom" "$part/"
    cp -R "$tree/guest/prebuilt" "$part/kexts"
    add_clock_kexts "$part/kexts"
    cp "$OUT/util-target/release/reims-util" "$part/"
    strip "$part/reims-util" "$part/qemu/bin/$QEMU_PROGRAM"
    copy_licences "$part/licences"
    write_version >"$part/VERSION"
    "$part/qemu/bin/$QEMU_PROGRAM" -device help | grep -q '"reims-graphics-pci"' ||
        die "the QEMU that was built has no reims-graphics-pci."
}

# The newest glibc anything in the part asks for. It has to run where Guesthouse does.
check_glibc() {
    ceiling=2.39
    needs=$(find "$OUT/part" -type f \( -perm -u+x -o -name '*.so*' \) -exec objdump -T {} + 2>/dev/null |
        sed -n 's/.*GLIBC_\([0-9][0-9.]*\).*/\1/p' | sort -uV | tail -n 1)
    [ "$(printf '%s\n%s\n' "$needs" "$ceiling" | sort -V | tail -n 1)" = "$ceiling" ] ||
        die "the part needs glibc $needs, and $ceiling is the newest it may."
}

dsc_files() {
    awk '/^Checksums-Sha256:/ { block = 1; next } block && /^ / { print $1, $3; next } { block = 0 }' "$1"
}

# Ubuntu's source packages for the bundled libraries, at the exact versions they were built from.
fetch_library_sources() { # folder
    mkdir -p "$1"
    awk '!/^#/ && $4 == "source" { print $5 "=" $6 }' "$OUT/part/LIBRARIES.txt" | sort -u |
        while read -r wanted; do
            dsc=${wanted%%=*}_$(printf '%s' "${wanted#*=}" | sed 's/^[0-9]*://').dsc
            [ -f "$1/$dsc" ] || (cd "$1" && apt-get source -q --download-only --only-source "$wanted") ||
                die "the archive has no $wanted. Remove the image and build again."
            dsc_files "$1/$dsc" | while read -r sha256 file; do
                [ "$(sha256sum "$1/$file" | cut -d ' ' -f 1)" = "$sha256" ] || die "$file isn't what $dsc lists."
            done
        done
}

write_sources_note() {
    cat <<EOF
The source of everything in Guesthouse's macOS part $PART.

reims-vgpu/     $REIMS_UPSTREAM at $REIMS_COMMIT, unmodified. The library in QEMU is built
                from crates/, ReimsGraphics.rom from firmware/, and the kexts from guest/, whose
                prebuilt/sources.toml names every file they were built from. vendor/qemu is its QEMU, at
                $REIMS_QEMU_COMMIT, with what its build fetched into subprojects/. Its
                $FIRMWARE_SOURCE is the source of efi-virtio.rom, and pc-bios/optionrom of the other
                firmware in qemu/share/qemu.

reims-util/     $UTIL_UPSTREAM at $UTIL_COMMIT, unmodified.

kexts/          The release zips of Lilu $LILU_VERSION and CpuTscSync $TSC_VERSION, as Acidanthera
                published them. Their source: https://github.com/acidanthera/Lilu/tree/$LILU_VERSION
                and https://github.com/acidanthera/CpuTscSync/tree/$TSC_VERSION.

crates/         Every crate those two builds took, as cargo vendor wrote them: metal2vulkan and SDL3's
                source among them, in reims-vgpu/.

libraries/      Ubuntu 24.04's source packages for the libraries in qemu/lib, at the exact versions
                they were built from. To unpack one: dpkg-source -x NAME_VERSION.dsc

recipes/        How the image was set up and how all of this was built.

Which library came from which source package:

EOF
    grep -v '^#' "$OUT/part/LIBRARIES.txt"
}

stage_source() {
    source=$OUT/source
    mkdir -p "$source/crates" "$source/recipes"
    for tree in reims-vgpu reims-util; do
        rm -rf "${source:?}/$tree"
        mkdir "$source/$tree"
        tar -C "$OUT/$tree" --exclude=.git -cf - . | tar -C "$source/$tree" -xf -
        [ -d "$source/crates/$tree" ] ||
            (cd "$OUT/$tree" && cargo vendor -q --versioned-dirs "$source/crates/$tree" >/dev/null)
    done
    fetch_library_sources "$source/libraries"
    mkdir -p "$source/kexts"
    cp "$OUT/clock-kexts/Lilu.zip" "$OUT/clock-kexts/CpuTscSync.zip" "$source/kexts/"
    cp "$REPO/packaging/Containerfile.macos" "$REPO/packaging/macos-configure-flags" \
        "$REPO/packaging/build-macos-part.sh" "$source/recipes/"
    write_sources_note >"$source/SOURCES.txt"
}

pack() { # folder, archive
    tar -C "$1" --transform "s,^\.,$NAME,S" --owner=0 --group=0 --sort=name -cJf "$2" .
}

build() {
    fetch_sources
    build_library
    build_qemu
    build_util
    stage_part
    check_glibc
    stage_source
    pack "$OUT/part" "$OUT/$NAME-$ARCH.tar.xz"
    pack "$OUT/source" "$OUT/$NAME-source.tar.xz"
    (cd "$OUT" && sha256sum "$NAME-$ARCH.tar.xz" "$NAME-source.tar.xz" >"$NAME-SHA256SUMS")
    say "the macOS part is $OUT/$NAME-$ARCH.tar.xz"
    cat "$OUT/$NAME-SHA256SUMS"
}

in_image() {
    engine=$(command -v podman || command -v docker) || die "this needs podman or docker."
    case ${engine##*/} in
        podman) as_user=--userns=keep-id ;;
        *) as_user=--user=$(id -u):$(id -g) ;;
    esac
    tag=guesthouse-build-macos:$(sha256sum <"$REPO/packaging/Containerfile.macos" | cut -c 1-12)
    "$engine" image inspect "$tag" >/dev/null 2>&1 ||
        "$engine" build -t "$tag" -f "$REPO/packaging/Containerfile.macos" "$REPO/packaging" >&2
    mkdir -p "$OUT/home"
    "$engine" run --rm --security-opt label=disable "$as_user" -e "HOME=$OUT/home" \
        -e "CARGO_HOME=$OUT/home/cargo" -v "$OUT:$OUT" -v "$REPO:$REPO:ro" -w "$OUT" "$tag" \
        nice -n 19 sh "$REPO/packaging/build-macos-part.sh" --inside --out "$OUT"
}

main() {
    inside=no
    while [ $# -gt 0 ]; do
        case $1 in
            --inside) inside=yes ;;
            --out)
                OUT=$2
                shift
                ;;
            *) die "there's no option called $1" ;;
        esac
        shift
    done
    [ -n "$OUT" ] || die "say where to build with --out FOLDER."
    mkdir -p "$OUT"
    case $inside in
        yes) build ;;
        no) in_image ;;
    esac
}

{ main "$@"; }
