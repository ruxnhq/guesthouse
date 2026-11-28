#!/bin/sh
# Builds what a release publishes: the payload that install.sh downloads, and the source of everything
# in it, QEMU included, which the GPL asks for. It needs no root, and publishes nothing.
#
# Usage: packaging/build-release.sh [--out FOLDER] [--display-driver FOLDER] [STAGE...]
#
# The stages run in this order, and with none named all of them do:
#   qemu      virglrenderer, then QEMU, its firmware blobs, the libraries it needs from the build
#             system, and the bridge helper
#   bundled   guesthouse-VERSION-bundled-source.tar, the source of those libraries and blobs
#   app       the app, the daemon and their helpers
#   payload   guesthouse-VERSION-x86_64.tar.xz
#   installer-libs  GTK and libadwaita for the AppImage's install window, on a PC that has neither yet
#   appimage  guesthouse-installer-VERSION.AppImage, the payload in one file that installs itself
#   source    guesthouse-VERSION-source.tar.xz, and SHA256SUMS
#
# check-driver isn't one of these: it only looks at what --display-driver names, and builds nothing.
# library-sources isn't either: it's the first part of bundled, the libraries' source packages, on their own.
#
# Everything built here only runs on this system's glibc or a newer one, so a release doesn't run this
# directly: packaging/build-in-images.sh runs each stage in its own image. On its own it builds for
# this PC, which is what install.sh --from-source wants. QEMU_UPSTREAM can name a local clone of QEMU.
#
# The display driver can't be built here, as it needs Windows and Microsoft's kit, so --display-driver
# names the folder holding what guest/drivers/windows-display/BUILDING.md builds: GuesthouseDisplay.dll and
# GuesthouseDisplay.inf. GUESTHOUSE_DISPLAY_DRIVER does the same. Without it a build for this PC still
# works, and its Windows machines stay on the basic display. The source stage refuses, as that's a release.
set -eu

QEMU_TAG=v11.1.3
QEMU_COMMIT=005c61c0ef6f6ae615283440b4bb7be51b2c6ddd
QEMU_UPSTREAM=${QEMU_UPSTREAM:-https://gitlab.com/qemu-project/qemu.git}
# QEMU's own release of that tag. Its firmware blobs are the ones in the tree, and unlike a clone it has
# the source of each in roms/, which a clone leaves empty.
QEMU_RELEASE_URL=https://download.qemu.org/qemu-${QEMU_TAG#v}.tar.xz
QEMU_RELEASE_SHA256=5a8289c7940dc77d3873c81a6c29ba3afe1446fc9d36e77fa1658cdbecb84b22
# The distros' virglrenderer has no AMD native context, so QEMU gets its own, from upstream's source.
VIRGL_TAG=virglrenderer-1.3.0
VIRGL_COMMIT=ca50e008863837e094747a69974dde3ae148aeaa
VIRGL_UPSTREAM=${VIRGL_UPSTREAM:-https://gitlab.freedesktop.org/virgl/virglrenderer.git}
# Venus for the fallback, and native context for AMD's cards. Video stays off, as nothing here uses it.
VIRGL_OPTIONS='-Dplatforms=egl -Dvenus=true -Ddrm-renderers=amdgpu-experimental -Dvideo=false -Dtests=false
  -Dbuildtype=release'
VIRGL_SERVER=virgl_render_server
APP_ID=io.github.xnixy.Guesthouse
ARCH=x86_64
DISPLAY_DRIVER=${GUESTHOUSE_DISPLAY_DRIVER:-}
DRIVER_SOURCE=guest/drivers/windows-display
# What the app wants in the program folder, in app-core's display.rs.
DRIVER_FOLDER=display-driver
DRIVER_BUILT='GuesthouseDisplay.dll GuesthouseDisplay.inf'
DRIVER_SETUP='sign-and-install.ps1 install-take-over.ps1 take-over-display.ps1'
# The driver has code from Microsoft's sample as well as zako-vdd's, so both licences go wherever it does.
DRIVER_FILES='vdd_settings.xml LICENSE LICENSE-MS-PL'
# Microsoft's build of OpenSSH for Windows, which goes into a Windows guest whose SSH is on. The guest
# may have no internet, and Windows' own way of adding it downloads it, so the installer comes with the
# payload. Its licence and its notices are inside it, and it puts them beside the programs in the guest.
# OPENSSH_URL can name another copy of the same file, which the hash still checks.
OPENSSH_URL=${OPENSSH_URL:-https://github.com/PowerShell/Win32-OpenSSH/releases/download/10.4.0.0p1-Preview/OpenSSH-Win64-v10.4.0.0.msi}
OPENSSH_SHA256=788d8643464a01eb63640a2fd746f9ed23d106b5a901a35f88dc8cd40e446ac9
# Where the app looks for it in the program folder, in app-core's paths.rs.
OPENSSH_FILE=openssh/OpenSSH-Win64.msi

# What an AppImage starts with: the AppImage project's runtime, which mounts the squashed folder behind it
# with no FUSE library needed on the host. It's MIT, with libfuse (LGPL), squashfuse and zstd linked in,
# so the bundled source carries the runtime's source at its tag and theirs at the versions it builds with.
APPIMAGE_RUNTIME_TAG=20251108
APPIMAGE_RUNTIME_URL=https://github.com/AppImage/type2-runtime/releases/download/$APPIMAGE_RUNTIME_TAG/runtime-x86_64
APPIMAGE_RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
# Each: file name, URL, sha256. libfuse's hash is the one the runtime's own build checks.
APPIMAGE_RUNTIME_SOURCES="type2-runtime-$APPIMAGE_RUNTIME_TAG.tar.gz https://github.com/AppImage/type2-runtime/archive/refs/tags/$APPIMAGE_RUNTIME_TAG.tar.gz 4c4f6df4647c9f01f871d7edd3716d8aeeffda9d22ffbebe3fccd95a6ab52c95
fuse-3.15.0.tar.xz https://github.com/libfuse/libfuse/releases/download/fuse-3.15.0/fuse-3.15.0.tar.xz 70589cfd5e1cff7ccd6ac91c86c01be340b227285c5e200baa284e401eea2ca0
squashfuse-0.5.2.tar.gz https://github.com/vasi/squashfuse/archive/0.5.2.tar.gz db0238c5981dabbd80ee09ae15387f390091668ca060a7bc38047912491443d3"
# Where the install window gets GTK and libadwaita on a PC that has neither yet, a KDE desktop say:
# Ubuntu 24.04's, the oldest the app takes, for the build without HDR. Only the AppImage carries them.
# The install adds the PC's own, and the installed app uses those.
INSTALLER_LIBS=installer-libs
PIXBUF_FOLDER=/usr/lib/$ARCH-linux-gnu/gdk-pixbuf-2.0
# The SVG one draws the icons. Whichever of the others this gdk-pixbuf doesn't have built in go too.
PIXBUF_LOADERS='libpixbufloader-svg.so libpixbufloader-png.so libpixbufloader-jpeg.so'
# The loaders' list names each by its path, and AppRun puts the real one in, as the AppImage's folder
# isn't the same twice.
INSTALLER_LIBS_MARK=@INSTALLER_LIBS@

# These stay the host's. The C library can't be mixed, and PipeWire's client must match its server.
# OpenGL is the host's too: GBM and EGL are Mesa's, which has to match the host's kernel driver, and
# libdrm and libepoxy go with it. What these need in turn is theirs to find, so none of it is bundled.
HOST_LIBRARIES='^(linux-vdso|ld-linux|libc|libm|libdl|libpthread|librt|libresolv|libgcc_s|libpipewire-0\.3|libepoxy|libgbm|libdrm|libdrm_amdgpu|libEGL|libGL|libGLX|libGLdispatch|libOpenGL|libvulkan)[.-]'

REPO=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
OUT=$REPO/build/release

say() {
    printf '%s\n' "$*"
}

die() {
    printf 'build-release: %s\n' "$*" >&2
    exit 1
}

release_version() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/Cargo.toml" | head -n 1
}

# A release has to be something the tag can rebuild, so nothing uncommitted goes into one. An unpacked
# source bundle has no git to ask, and is what it is.
refuse_uncommitted() {
    [ -e "$REPO/.git" ] || return 0
    [ -z "$(git -C "$REPO" status --porcelain)" ] ||
        die "there are uncommitted changes, and a release must be rebuildable from its tag"
}

# A link that's bent still packs and unpacks, and only building from the archive would show it. So each
# archive gets unpacked here, and every link in it has to land on something inside it.
check_links() {
    unpacked=$(mktemp -d "$OUT/links.XXXXXX")
    tar -xf "$1" -C "$unpacked"
    bad=$(find "$unpacked" -type l | while read -r link; do
        case $(readlink -f "$link") in
            "$unpacked"/*) [ -e "$link" ] || printf '  %s\n' "${link#"$unpacked"/}" ;;
            *) printf '  %s\n' "${link#"$unpacked"/}" ;;
        esac
    done)
    rm -rf "${unpacked:?}"
    [ -z "$bad" ] || die "$1 has links that point nowhere:
$bad"
}

# Packs a folder under a top folder of the archive's own name. The S keeps that rename off link targets.
pack() { # folder, top folder, archive, tar's flags for it
    tar -C "$1" --transform "s,^\.,$2,S" --owner=0 --group=0 --sort=name "$4" "$3" .
    check_links "$3"
}

# The source bundle carries QEMU's tree, and a clone fetches one.
qemu_tree() {
    if [ -d "$REPO/qemu-source" ]; then
        printf '%s' "$REPO/qemu-source"
    else
        printf '%s' "$OUT/qemu-source"
    fi
}

# The source bundle carries virglrenderer's tree too, and a clone fetches one.
virgl_tree() {
    if [ -d "$REPO/virglrenderer-source" ]; then
        printf '%s' "$REPO/virglrenderer-source"
    else
        printf '%s' "$OUT/virglrenderer-source"
    fi
}

fetch_virgl() {
    VIRGL_TREE=$(virgl_tree)
    [ ! -d "$VIRGL_TREE" ] || return 0
    git clone -q --depth 1 --branch "$VIRGL_TAG" "$VIRGL_UPSTREAM" "$VIRGL_TREE.new"
    [ "$(git -C "$VIRGL_TREE.new" rev-parse HEAD)" = "$VIRGL_COMMIT" ] ||
        die "$VIRGL_TAG is not the commit it should be"
    mv "$VIRGL_TREE.new" "$VIRGL_TREE"
}

# Installed into a folder of the build's, which QEMU's configure finds it in. The library names the
# render server's place there, and the app tells it the real one.
build_virgl() {
    fetch_virgl
    VIRGL_PREFIX=$OUT/virgl
    [ -f "$VIRGL_PREFIX/lib/pkgconfig/virglrenderer.pc" ] && return 0
    build=$OUT/virgl-build
    [ ! -d "$build" ] || rm -rf "${build:?}"
    # shellcheck disable=SC2086 # the options are words on purpose
    meson setup "$build" "$VIRGL_TREE" --prefix="$VIRGL_PREFIX" --libdir=lib $VIRGL_OPTIONS
    ninja -C "$build" install
}

# From upstream, at the exact commit.
fetch_qemu() {
    QEMU_TREE=$(qemu_tree)
    [ ! -d "$QEMU_TREE" ] || return 0
    git clone -q --depth 1 --branch "$QEMU_TAG" "$QEMU_UPSTREAM" "$QEMU_TREE.new"
    [ "$(git -C "$QEMU_TREE.new" rev-parse HEAD)" = "$QEMU_COMMIT" ] ||
        die "$QEMU_TAG is not the commit it should be"
    git -C "$QEMU_TREE.new" -c user.name=release -c user.email=release@localhost \
        am -q "$REPO/qemu/qemu-gpupv.patch" "$REPO/qemu/qemu-host-loopback.patch"
    mv "$QEMU_TREE.new" "$QEMU_TREE"
}

# DT_RPATH and not the newer DT_RUNPATH, as only the older one also covers what the bundled libraries
# load themselves. That saves rewriting each of them with patchelf.
configure_qemu() {
    build=$OUT/qemu-build
    [ ! -f "$build/build.ninja" ] || return 0
    mkdir -p "$build"
    # shellcheck disable=SC2046 # the file holds one flag a line, and none has a space in it
    (cd "$build" && PKG_CONFIG_PATH="$VIRGL_PREFIX/lib/pkgconfig" \
        "$QEMU_TREE/configure" $(cat "$REPO/packaging/qemu-configure-flags") \
        '--extra-ldflags=-Wl,--disable-new-dtags,-rpath,$ORIGIN/lib')
}

# QEMU's bridge helper, which install.sh makes setuid root, is built on its own. It reads
# /etc/qemu/bridge.conf and nothing found from where it sits, as a user could write there. It gives up
# root for the one right it needs as it starts, which takes libcap-ng. And it has no rpath, as a setuid
# program ignores $ORIGIN, so it uses the system's glib and libcap-ng.
configure_bridge_helper() {
    build=$OUT/bridge-helper-build
    [ ! -f "$build/build.ninja" ] || return 0
    mkdir -p "$build"
    (cd "$build" && "$QEMU_TREE/configure" --prefix=/usr --sysconfdir=/etc --disable-relocatable \
        --without-default-features --disable-system --disable-user --enable-tools --enable-cap-ng \
        --disable-docs --disable-werror)
}

build_bridge_helper() {
    configure_bridge_helper
    ninja -C "$OUT/bridge-helper-build" qemu-bridge-helper
    helper=$OUT/bridge-helper-build/qemu-bridge-helper
    # Without libcap-ng it would keep root for everything it does.
    ldd "$helper" | grep -q 'libcap-ng' || die "the bridge helper was built without libcap-ng."
    ! readelf -d "$helper" | grep -Eq 'RPATH|RUNPATH' || die "the bridge helper has an rpath."
    cp "$helper" "$1/"
}

build_qemu() {
    targets=qemu-system-$ARCH
    # The Linux route's helper, once the patch has it.
    [ ! -d "$QEMU_TREE/hw/misc/nvfwd" ] || targets="$targets hw/misc/nvfwd/nvfwd-sensord"
    # shellcheck disable=SC2086 # two words on purpose
    ninja -C "$OUT/qemu-build" $targets
}

# Which package a library came from, and the source package that was built from.
library_package() {
    if command -v dpkg-query >/dev/null 2>&1; then
        package=$(dpkg-query -S "$(readlink -f "$1")" 2>/dev/null | head -n 1 | cut -d : -f 1)
        [ -z "$package" ] ||
            dpkg-query -W -f '${Package} ${Version} source ${source:Package} ${source:Version}' "$package"
    elif command -v rpm >/dev/null 2>&1; then
        rpm -qf "$1"
    fi
}

# What a program or library asks for by name.
needed_names() {
    readelf -d "$1" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'
}

# The libraries to bundle: what the program needs and what those need in turn, but nothing that only
# the host's own libraries need, as theirs must come from where they do. Prints a name and a path a line.
bundled_closure() {
    resolved=$OUT/stage/resolved.txt
    ldd "$1" | awk '$2 == "=>" && $3 ~ /^\// { print $1, $3 }' >"$resolved"
    queue=$(needed_names "$1")
    seen=" "
    while [ -n "$queue" ]; do
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

# Libraries are copied under their sonames, which is what the loader asks for. This reads the program
# where it was built, as a staged one finds the libraries of the last run through its rpath.
bundle_libraries() {
    program=$1
    folder=$2
    list=$OUT/stage/LIBRARIES.txt
    mkdir -p "$folder"
    cat >"$list" <<'EOF'
# The libraries in qemu/build/lib, and the firmware in qemu/pc-bios, aren't mine, and come with their
# licences. Each release has an asset called guesthouse-VERSION-bundled-source.tar with the exact
# source of all of them. Its SOURCES.txt says what's where.
#
# Library, package, its version, and the source package it was built from:
EOF
    bundled_closure "$program" | while read -r name path; do
        cp -L "$path" "$folder/$name"
        chmod u+w "$folder/$name"
        case $path in
            "$VIRGL_PREFIX"/*) printf '%s %s\n' "$name" "virglrenderer $VIRGL_TAG, built here from upstream" >>"$list" ;;
            *) printf '%s %s\n' "$name" "$(library_package "$path" || echo unknown)" >>"$list" ;;
        esac
    done
}

stage_qemu() {
    build_virgl
    fetch_qemu
    configure_qemu
    build_qemu
    target=$OUT/stage/qemu/build
    mkdir -p "$target" "$OUT/stage/qemu/pc-bios"
    cp "$OUT/qemu-build/qemu-system-$ARCH" "$target/"
    [ ! -f "$OUT/qemu-build/hw/misc/nvfwd/nvfwd-sensord" ] ||
        cp "$OUT/qemu-build/hw/misc/nvfwd/nvfwd-sensord" "$target/"
    bundle_libraries "$OUT/qemu-build/qemu-system-$ARCH" "$target/lib"
    # Venus starts it. It needs only the host's GBM and the C library.
    cp "$VIRGL_PREFIX/libexec/$VIRGL_SERVER" "$target/"
    build_bridge_helper "$target"
    # The build folder holds what was generated, like the keymaps, and the source the rest.
    cp -R "$QEMU_TREE/pc-bios/." "$OUT/stage/qemu/pc-bios/"
    cp -RL "$OUT/qemu-build/pc-bios/." "$OUT/stage/qemu/pc-bios/"
    say "QEMU is in $target"
}

# The files a .dsc lists with their sha256, one "SHA256 NAME" a line, from its Checksums-Sha256 block.
dsc_files() {
    awk '/^Checksums-Sha256:/ { block = 1; next } block && /^ / { print $1, $3; next } { block = 0 }' "$1"
}

# What the image's own apt lists say the .dsc's sha256 is. They come from the signed index, so a .dsc that
# matches can be trusted for the hashes it lists.
dsc_sha256() { # source package, version, .dsc file name
    apt-cache showsrc --only-source "$1" 2>/dev/null | awk -v version="$2" -v file="$3" '
        /^Package:/ { this = 0 } $1 == "Version:" { this = ($2 == version) }
        /^Checksums-Sha256:/ { block = this; next } block && /^ / { if ($3 == file) print $1; next } { block = 0 }' |
        head -n 1
}

has_sha256() { # file, sha256
    [ -f "$1" ] && [ "$(sha256sum "$1" | cut -d ' ' -f 1)" = "$2" ]
}

# A source package that's already in the folder, from an earlier build, needs no download. Every file has
# to be there with the hash the archive gave it.
sources_present() { # folder, source package, version
    dsc=${2}_${3#*:}.dsc
    want=$(dsc_sha256 "$2" "$3" "$dsc")
    [ -n "$want" ] && has_sha256 "$1/$dsc" "$want" || return 1
    dsc_files "$1/$dsc" | {
        listed=0
        while read -r sha256 file; do
            has_sha256 "$1/$file" "$sha256" || exit 1
            listed=1
        done
        [ "$listed" = 1 ]
    }
}

# apt-get says why a download failed. If it couldn't get through, the version may well still be there.
download_source() { # folder, package=version
    said=$(mktemp)
    if (cd "$1" && apt-get source -q --download-only --only-source "$2") >"$said" 2>&1; then
        cat "$said"
        rm -f "$said"
        return 0
    fi
    cat "$said"
    if grep -qiE 'unable to connect|could not connect|connection failed|connection timed out|temporary failure resolving|could not resolve' "$said"; then
        rm -f "$said"
        die "couldn't reach the archive for $2. It may still have it, so try again when the archive answers."
    fi
    rm -f "$said"
    die "the archive has no $2 any more. Remove the qemu image and build again."
}

# The exact versions the bundled libraries were built from, with Ubuntu's patches and packaging. An
# archive stops listing an old version soon after a new one, and then a newer image is the answer. What's
# already in the folder with the right hashes is used as it is, and anything else there is removed, so
# the bundle holds exactly these.
fetch_library_sources() {
    kept=$(mktemp)
    lists=$OUT/stage/LIBRARIES.txt
    [ ! -f "$OUT/$INSTALLER_LIBS/LIBRARIES.txt" ] || lists="$lists $OUT/$INSTALLER_LIBS/LIBRARIES.txt"
    # shellcheck disable=SC2086 # one or two paths, with no spaces
    awk '$3 != "" && $4 == "source" { print $5 "=" $6 }' $lists | sort -u |
        while read -r wanted; do
            name=${wanted%%=*}
            version=${wanted#*=}
            dsc=${name}_${version#*:}.dsc
            if sources_present "$1" "$name" "$version"; then
                say "reusing $wanted, as it's here already with the right hashes"
            else
                download_source "$1" "$wanted"
            fi
            [ -f "$1/$dsc" ] || die "$wanted didn't arrive."
            printf '%s\n' "$dsc" >>"$kept"
            dsc_files "$1/$dsc" | cut -d ' ' -f 2 >>"$kept"
        done
    for file in "$1"/* "$1"/.[!.]*; do
        [ -e "$file" ] || continue
        grep -qxF "${file##*/}" "$kept" || rm -rf "$file"
    done
    rm -f "$kept"
}

# Only the libraries' sources, into bundled-source/libraries, where the bundled stage will find them.
stage_library_sources() {
    command -v apt-get >/dev/null 2>&1 || die "this stage runs in the image from Containerfile.qemu."
    [ -f "$OUT/stage/LIBRARIES.txt" ] || die "the qemu stage comes first."
    mkdir -p "$OUT/bundled-source/libraries"
    fetch_library_sources "$OUT/bundled-source/libraries"
}

# Kept beside the build and used again while its hash is right, like OpenSSH.
fetch_kept() { # file name, URL, sha256
    kept=$OUT/downloads/$1
    has_sha256 "$kept" "$3" && return 0
    mkdir -p "$OUT/downloads"
    rm -f "$kept"
    curl -fsSL -o "$kept.part" "$2" || die "couldn't download $2"
    mv "$kept.part" "$kept"
    has_sha256 "$kept" "$3" || die "$2 isn't the file it should be."
}

fetch_appimage_runtime_source() {
    printf '%s\n' "$APPIMAGE_RUNTIME_SOURCES" | while read -r file url sha256; do
        fetch_kept "$file" "$url" "$sha256"
        cp "$OUT/downloads/$file" "$1/"
    done
}

fetch_firmware_source() {
    curl -fsSL -o "$1/qemu-${QEMU_TAG#v}.tar.xz" "$QEMU_RELEASE_URL"
    [ "$(sha256sum "$1/qemu-${QEMU_TAG#v}.tar.xz" | cut -d ' ' -f 1)" = "$QEMU_RELEASE_SHA256" ] ||
        die "$QEMU_RELEASE_URL isn't the file it should be."
}

write_sources_note() {
    cat <<EOF
The source of everything in a Guesthouse release that isn't mine and isn't in its source bundle.

libraries/  Ubuntu 24.04's source packages for the libraries in qemu/build/lib, and for the GTK and
            libadwaita the AppImage's install window can use, at the exact versions they were built from: each .dsc with its original source and Ubuntu's changes. debian/rules
            in each is how it was built. To unpack one: dpkg-source -x NAME_VERSION.dsc

virglrenderer/  Upstream's $VIRGL_TAG, unmodified, which libvirglrenderer.so.1 and virgl_render_server
            in qemu/build are built from, with the options in recipes/build-release.sh.

appimage-runtime/  The source of the runtime at the front of the AppImage: the AppImage project's
            type2-runtime at its tag $APPIMAGE_RUNTIME_TAG, unmodified, and the libfuse and squashfuse
            releases its scripts/common/install-dependencies.sh builds it with. libfuse gets the patch
            in the runtime's patches/libfuse. zstd, zlib, mimalloc and musl are Alpine 3.21's, as its
            scripts/docker/Dockerfile says.

firmware/   QEMU's own release of $QEMU_TAG, unmodified. Its pc-bios/ holds the firmware in qemu/pc-bios,
            and roms/ the source of it, with the recipes in roms/Makefile.

recipes/    How the libraries' image and QEMU itself were set up here.

Which library came from which source package:

EOF
    grep -v '^#' "$OUT/stage/LIBRARIES.txt"
}

# Separate from the source bundle, as it's large and building Guesthouse doesn't need it. It's a plain
# tar, as all it holds is packed already.
stage_bundled() {
    command -v apt-get >/dev/null 2>&1 || die "this stage runs in the image from Containerfile.qemu."
    [ -f "$OUT/stage/LIBRARIES.txt" ] || die "the qemu stage comes first."
    name=guesthouse-$(release_version)-bundled-source
    bundle=$OUT/bundled-source
    # The libraries' sources stay, as fetch_library_sources checks each one and reuses what's right.
    [ ! -d "$bundle" ] || find "$bundle" -mindepth 1 -maxdepth 1 ! -name libraries -exec rm -rf {} +
    mkdir -p "$bundle/libraries" "$bundle/firmware" "$bundle/appimage-runtime" "$bundle/recipes"
    fetch_library_sources "$bundle/libraries"
    fetch_firmware_source "$bundle/firmware"
    fetch_appimage_runtime_source "$bundle/appimage-runtime"
    fetch_virgl
    mkdir -p "$bundle/virglrenderer"
    tar -C "$VIRGL_TREE" --exclude=.git -cf - . | tar -C "$bundle/virglrenderer" -xf -
    cp "$REPO/packaging/Containerfile.qemu" "$REPO/packaging/qemu-configure-flags" \
        "$REPO/packaging/build-release.sh" "$bundle/recipes/"
    write_sources_note >"$bundle/SOURCES.txt"
    pack "$bundle" "$name" "$OUT/$name.tar" -cf
    rm -rf "${bundle:?}"
    say "the bundled source is $OUT/$name.tar"
}

# Each real backend the tree has goes in. With none, the daemon says it has none.
daemon_features() {
    features=""
    for vendor in nvidia amd intel; do
        [ ! -d "$REPO/linux/backend-$vendor" ] || features="$features${features:+,}guesthouse-daemon/$vendor"
    done
    [ -z "$features" ] || printf '%s' "--features $features"
}

# What the app calls in GTK and libadwaita. install.sh looks for each in the host's libraries, which
# says exactly whether this build can run there, with no list of versions to keep up.
write_symbol_needs() {
    nm -D --undefined-only "$1" | awk '{ print $2 }' | sed 's/@.*//' |
        grep -E '^(gtk|gdk|gsk|adw)_' | sort -u >"$OUT/stage/needs-symbols"
}

# What a tester of an Intel card sends with the daemon's log. The task program runs it, from the
# folder it is itself in.
stage_intel_probe() {
    [ -d "$REPO/linux/backend-intel" ] || return 0
    (cd "$REPO" && cargo build --release --locked -p guesthouse-backend-intel --bin intel-probe)
    cp "${CARGO_TARGET_DIR:-$REPO/target}/release/intel-probe" "$OUT/stage/"
}

stage_app() {
    target_folder=${CARGO_TARGET_DIR:-$REPO/target}
    # shellcheck disable=SC2046 # empty, or the flag and its value
    (cd "$REPO" && cargo build --release --locked -p guesthouse-app -p guesthouse-app-core \
        -p guesthouse-clock-helper -p guesthouse-daemon $(daemon_features))
    # The lab's stand-in card is never in a build for people. Its switch's name is only in a daemon
    # built with it.
    ! grep -q -a GUESTHOUSE_INTEL_LAB_FAKE_CARD "$target_folder/release/guesthouse-daemon" ||
        die "guesthouse-daemon was built with the lab's stand-in card, which is never in a release."
    mkdir -p "$OUT/stage"
    stage_intel_probe
    for program in guesthouse guesthouse-check guesthouse-daemon guesthouse-jail guesthouse-share guesthouse-task guesthouse-vm \
        guesthouse-report guesthouse-remote-view guesthouse-clock-helper; do
        cp "$target_folder/release/$program" "$OUT/stage/"
    done
    # install.sh makes the clock helper setuid root, and a setuid program ignores $ORIGIN.
    ! readelf -d "$OUT/stage/guesthouse-clock-helper" | grep -Eq 'RPATH|RUNPATH' || die "the clock helper has an rpath."
    # A build with HDR won't load on GTK older than 4.16, so a second one goes along for those, and
    # install.sh keeps the one that fits. It's the lower floor, so it's what the symbols are read from.
    (cd "$REPO" && cargo build --release --locked -p guesthouse-app --no-default-features)
    cp "$target_folder/release/guesthouse" "$OUT/stage/guesthouse-no-hdr"
    write_symbol_needs "$OUT/stage/guesthouse-no-hdr"
    mkdir -p "$OUT/stage/share"
    cp "$REPO/linux/app/data/$APP_ID.desktop" "$OUT/stage/share/"
    cp -R "$REPO/linux/app/data/icons" "$OUT/stage/share/"
    say "the app is in $OUT/stage"
}

# The highest glibc symbol version anything here asks for, which is what the host must have.
glibc_needed() {
    find "$OUT/stage" -type f \( -perm -u+x -o -name '*.so*' \) -exec grep -aoh 'GLIBC_2\.[0-9.]*' {} + |
        sed 's/GLIBC_//; s/\.$//' | sort -Vu | tail -n 1
}

# The folder names are the app's, in paths.rs and linux/mod.rs: the repo's guest/windows is the
# program folder's guest, and guest/linux its linux. The recipes are the app's own, and each is a
# file of text that a person can read before it is used.
copy_guest_files() {
    if [ -d "$REPO/guest/windows" ]; then
        mkdir -p "$OUT/stage/guest"
        cp -R "$REPO/guest/windows/." "$OUT/stage/guest/"
    fi
    if [ -d "$REPO/guest/linux" ]; then
        mkdir -p "$OUT/stage/linux"
        cp -R "$REPO/guest/linux/." "$OUT/stage/linux/"
    fi
    [ ! -d "$REPO/recipes" ] || cp -R "$REPO/recipes" "$OUT/stage/"
}

# The NVAPI wrapper a Windows guest gets in System32, so DLSS finds a paravirtualized card. It goes
# beside the guest's scripts, where the app takes it from.
stage_nvapi_shim() {
    "$REPO/guest/nvapi-shim/nvapi-shim.sh" "$OUT/stage/guest/nvapi64.dll" || die "The NVAPI wrapper didn't build."
}

# A build that isn't really one would only show up in a guest, much later.
check_display_driver() {
    for name in $DRIVER_BUILT; do
        [ -s "$DISPLAY_DRIVER/$name" ] || die "$DISPLAY_DRIVER has no $name. BUILDING.md says how to build it."
    done
    [ "$(head -c 2 "$DISPLAY_DRIVER/GuesthouseDisplay.dll")" = MZ ] ||
        die "$DISPLAY_DRIVER/GuesthouseDisplay.dll isn't a Windows program."
    ! grep -q '\$ARCH\$\|\$UMDFVERSION\$' "$DISPLAY_DRIVER/GuesthouseDisplay.inf" ||
        die "$DISPLAY_DRIVER/GuesthouseDisplay.inf is the source's, not the build's."
}

# Leaves the built driver with what the app needs to install it, and an id that tells builds apart.
copy_display_driver() {
    target=$OUT/stage/$DRIVER_FOLDER
    [ ! -d "$target" ] || rm -rf "${target:?}"
    if [ -z "$DISPLAY_DRIVER" ]; then
        say "there's no display driver in this build, so its Windows machines stay on the basic display"
        return 0
    fi
    check_display_driver
    mkdir -p "$target"
    for name in $DRIVER_BUILT; do
        cp "$DISPLAY_DRIVER/$name" "$target/"
    done
    for name in $DRIVER_SETUP; do
        cp "$REPO/$DRIVER_SOURCE/setup/$name" "$target/"
    done
    for name in $DRIVER_FILES; do
        cp "$REPO/$DRIVER_SOURCE/$name" "$target/"
    done
    (cd "$target" && cat GuesthouseDisplay.dll GuesthouseDisplay.inf | sha256sum | cut -d ' ' -f 1) >"$target/package.txt"
    say "the display driver is $(cat "$target/package.txt")"
}

# The download is kept beside the build and used again while its hash is right. A build that can't
# download it goes on without it, and its Windows machines can't have SSH, which the app says. A release
# can't go out that way, and the source stage stops it.
fetch_openssh() {
    target=$OUT/stage/${OPENSSH_FILE%/*}
    [ ! -d "$target" ] || rm -rf "${target:?}"
    kept=$OUT/downloads/${OPENSSH_FILE##*/}
    if ! has_sha256 "$kept" "$OPENSSH_SHA256"; then
        mkdir -p "$OUT/downloads"
        rm -f "$kept"
        if ! curl -fsSL -o "$kept.part" "$OPENSSH_URL"; then
            rm -f "$kept.part"
            say "OpenSSH for Windows couldn't be downloaded, so this build's Windows machines can't have SSH"
            return 0
        fi
        mv "$kept.part" "$kept"
        has_sha256 "$kept" "$OPENSSH_SHA256" || die "$OPENSSH_URL isn't the file it should be."
    fi
    mkdir -p "$target"
    cp "$kept" "$OUT/stage/$OPENSSH_FILE"
    say "OpenSSH for Windows is $OUT/stage/$OPENSSH_FILE"
}

release_parts() {
    parts=app
    [ ! -d "$OUT/stage/guest" ] || parts="$parts windows"
    [ ! -d "$OUT/stage/$DRIVER_FOLDER" ] || parts="$parts display"
    [ ! -d "$OUT/stage/linux" ] || parts="$parts linux"
    printf '%s' "$parts"
}

# Packs the stage under a folder of the archive's own name, without leaving a second copy behind.
pack_stage() {
    pack "$OUT/stage" "$1" "$OUT/$1.tar.xz" -cJf
}

stage_payload() {
    version=$(release_version)
    copy_guest_files
    stage_nvapi_shim
    copy_display_driver
    fetch_openssh
    cp "$REPO/install.sh" "$REPO/uninstall.sh" "$REPO/LICENSE" "$REPO/THIRD-PARTY.md" "$OUT/stage/"
    {
        printf 'version=%s\n' "$version"
        printf 'commit=%s\n' "$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo source-bundle)"
        printf 'glibc=%s\n' "$(glibc_needed)"
        printf 'parts=%s\n' "$(release_parts)"
    } >"$OUT/stage/RELEASE"
    pack_stage "guesthouse-$version-$ARCH"
    say "the payload is $OUT/guesthouse-$version-$ARCH.tar.xz"
}

# Ubuntu's libraries, from this image, under their sonames, with what the app and the loaders need.
stage_installer_libs() {
    command -v apt-get >/dev/null 2>&1 || die "this stage runs in the image from Containerfile.qemu."
    program=$OUT/stage/guesthouse-no-hdr
    [ -f "$program" ] || die "the install window is the app, so the app stage comes first."
    folder=$OUT/$INSTALLER_LIBS
    [ ! -d "$folder" ] || rm -rf "${folder:?}"
    # gio-modules stays empty, so GIO loads none of the PC's, which were built for another GLib.
    mkdir -p "$folder/lib" "$folder/loaders" "$folder/gio-modules"
    loaders=""
    for loader in $PIXBUF_LOADERS; do
        [ ! -f "$PIXBUF_FOLDER/2.10.0/loaders/$loader" ] || loaders="$loaders $PIXBUF_FOLDER/2.10.0/loaders/$loader"
    done
    list=$folder/LIBRARIES.txt
    {
        printf '%s\n' "# What the AppImage's install window uses on a PC with no GTK or libadwaita yet. They aren't mine,"
        printf '%s\n' "# and come with their licences. Their source is in guesthouse-VERSION-bundled-source.tar."
        printf '%s\n' "#" "# Library, package, its version, and the source package it was built from:"
    } >"$list"
    # shellcheck disable=SC2086 # paths with no spaces
    for needs in "$program" $loaders; do
        bundled_closure "$needs"
    done | sort -u | while read -r name path; do
        cp -L "$path" "$folder/lib/$name"
        chmod u+w "$folder/lib/$name"
        printf '%s %s\n' "$name" "$(library_package "$path" || echo unknown)" >>"$list"
    done
    for loader in $loaders; do
        cp "$loader" "$folder/loaders/"
        printf '%s %s\n' "${loader##*/}" "$(library_package "$loader" || echo unknown)" >>"$list"
    done
    "$PIXBUF_FOLDER/gdk-pixbuf-query-loaders" "$folder"/loaders/*.so | sed "s|$folder|$INSTALLER_LIBS_MARK|g" \
        >"$folder/loaders.cache"
    say "the install window's GTK is in $folder"
}

# The payload's stage in one file that installs itself: the AppImage runtime, then the folder squashed.
# packaging/appimage/AppRun.sh says what it does when it's opened.
stage_appimage() {
    [ -f "$OUT/stage/RELEASE" ] || die "the AppImage carries the payload's stage, so the payload stage comes first."
    command -v mksquashfs >/dev/null 2>&1 || die "this stage needs mksquashfs, from squashfs-tools."
    [ -f "$OUT/$INSTALLER_LIBS/loaders.cache" ] ||
        die "the AppImage carries the install window's GTK, so the installer-libs stage comes first."
    name=guesthouse-installer-$(release_version).AppImage
    fetch_kept "runtime-x86_64-$APPIMAGE_RUNTIME_TAG" "$APPIMAGE_RUNTIME_URL" "$APPIMAGE_RUNTIME_SHA256"
    folder=$OUT/Guesthouse.AppDir
    [ ! -d "$folder" ] || rm -rf "${folder:?}"
    mkdir -p "$folder"
    cp -R "$OUT/stage" "$folder/payload"
    cp -R "$OUT/$INSTALLER_LIBS" "$folder/$INSTALLER_LIBS"
    cp "$REPO/packaging/appimage/AppRun.sh" "$folder/AppRun"
    chmod 755 "$folder/AppRun"
    # The desktop file and the icon at the top are what tools that list AppImages show.
    cp "$REPO/linux/app/data/$APP_ID.desktop" "$folder/"
    cp "$REPO/linux/app/data/icons/hicolor/scalable/apps/$APP_ID.svg" "$folder/"
    ln -s "$APP_ID.svg" "$folder/.DirIcon"
    rm -f "$OUT/$name" "$OUT/$name.squashfs"
    mksquashfs "$folder" "$OUT/$name.squashfs" -root-owned -noappend -comp zstd -quiet -no-progress
    cat "$OUT/downloads/runtime-x86_64-$APPIMAGE_RUNTIME_TAG" "$OUT/$name.squashfs" >"$OUT/$name"
    chmod 755 "$OUT/$name"
    rm -rf "$OUT/$name.squashfs" "${folder:?}"
    say "the AppImage is $OUT/$name"
}

# Everything the GPL asks for in one file: this repo with the patch and these scripts, QEMU's whole tree
# with its subprojects, and the Rust crates, so that nothing depends on anywhere else staying as it is.
stage_source() {
    [ -f "$OUT/stage/$DRIVER_FOLDER/GuesthouseDisplay.dll" ] ||
        die "a release can't go out without the display driver. Name the build with --display-driver."
    [ -f "$OUT/stage/$OPENSSH_FILE" ] ||
        die "a release can't go out without OpenSSH for Windows, which the payload stage downloads."
    version=$(release_version)
    name=guesthouse-$version-source
    bundle=$OUT/source-bundle
    [ ! -d "$bundle" ] || rm -rf "${bundle:?}"
    mkdir -p "$bundle/qemu-source"
    git -C "$REPO" archive HEAD | tar -C "$bundle" -xf -
    tar -C "$(qemu_tree)" --exclude=.git --exclude=__pycache__ -cf - . | tar -C "$bundle/qemu-source" -xf -
    mkdir -p "$bundle/virglrenderer-source"
    tar -C "$(virgl_tree)" --exclude=.git -cf - . | tar -C "$bundle/virglrenderer-source" -xf -
    # The crates are linked into the programs, and carry their licences with them here.
    (cd "$REPO" && cargo vendor --locked --quiet "$bundle/vendor" >/dev/null)
    mkdir -p "$bundle/.cargo"
    printf '[source.crates-io]\nreplace-with = "vendored-sources"\n\n[source.vendored-sources]\ndirectory = "vendor"\n' \
        >"$bundle/.cargo/config.toml"
    pack "$bundle" "$name" "$OUT/$name.tar.xz" -cJf
    rm -rf "${bundle:?}"
    bundled=guesthouse-$version-bundled-source.tar
    [ -f "$OUT/$bundled" ] || bundled=""
    appimage=guesthouse-installer-$version.AppImage
    [ -f "$OUT/$appimage" ] || appimage=""
    # The payload and the source come first, as install.sh's hashes are read from the first two lines.
    # shellcheck disable=SC2086 # each empty, or one name with no spaces
    (cd "$OUT" && sha256sum "guesthouse-$version-$ARCH.tar.xz" "$name.tar.xz" $bundled $appimage >SHA256SUMS)
    say "the source is $OUT/$name.tar.xz"
    say "for RELEASE_VERSION, PAYLOAD_SHA256 and SOURCE_SHA256 in install.sh:"
    say "$version"
    head -n 2 "$OUT/SHA256SUMS" | cut -d ' ' -f 1
}

run_stage() {
    case $1 in
        qemu) stage_qemu ;;
        bundled) stage_bundled ;;
        app) stage_app ;;
        payload) stage_payload ;;
        installer-libs) stage_installer_libs ;;
        appimage) stage_appimage ;;
        source) stage_source ;;
        check-driver) check_display_driver ;;
        library-sources) stage_library_sources ;;
        *) die "there's no stage called $1" ;;
    esac
}

main() {
    if [ "${1:-}" = --out ]; then
        OUT=$2
        shift 2
    fi
    if [ "${1:-}" = --display-driver ]; then
        DISPLAY_DRIVER=$2
        shift 2
    fi
    [ $# -gt 0 ] || set -- qemu app installer-libs payload appimage source
    refuse_uncommitted
    mkdir -p "$OUT"
    for stage in "$@"; do
        run_stage "$stage"
    done
}

{ main "$@"; }
