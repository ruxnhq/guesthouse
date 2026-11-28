#!/bin/sh
# SPDX-License-Identifier: MIT
#
# Builds our Mesa in the build VM, for Ubuntu 26.04's glibc, from the tree
# in ~/mesa-src (build-mesa.sh puts it there): KosmicKrisp with the bridge
# pvg, Zink, EGL, GBM, GLES 2 and GL with GLX, the Wayland and X11
# platforms, through glvnd with the vendor name pvg, prefix /opt/pvg-mesa,
# installed into ~/mesa-stage by DESTDIR.
#
# mesa_clc and vtn_bindgen2 come first from a build of their own with LLVM
# (Ubuntu's LLVM 21), and the build that is packaged takes them as
# -Dmesa-clc=system with LLVM off, so nothing it installs links LLVM.
#
# The libraries get the run path /opt/pvg-mesa/lib, so that what glvnd,
# libgbm and the Vulkan loader open by its full path finds libgallium
# beside it with no ld.so.conf.d file. Our libgbm.so.1 is not packaged:
# every process uses Ubuntu's, which opens our backend (make-package.sh).
set -eu
src=$HOME/mesa-src
clc=$HOME/build-clc
build=$HOME/build-mesa
stage=$HOME/mesa-stage
start=$(date +%s)
mkdir -p "$src/subprojects/packagecache"
cp "$HOME"/dl/*.tar.* "$src/subprojects/packagecache/" 2>/dev/null || true

if [ ! -f "$clc/build.ninja" ]; then
	meson setup "$clc" "$src" --prefix="$HOME/clc" --libdir=lib \
		-Dbuildtype=release -Dgallium-drivers= -Dvulkan-drivers= -Dplatforms= \
		-Dglx=disabled -Degl=disabled -Dgbm=disabled -Dopengl=false -Dgles1=disabled \
		-Dgles2=disabled -Dvideo-codecs= -Dvalgrind=disabled -Dlibunwind=disabled \
		-Dlmsensors=disabled -Dmesa-clc=enabled -Dinstall-mesa-clc=true \
		-Dllvm=enabled -Dshared-llvm=enabled > "$HOME/clc-setup.log" 2>&1 ||
		{ tail -40 "$HOME/clc-setup.log"; echo "vm-mesa: FAILED: meson setup of mesa_clc"; exit 1; }
fi
ninja -C "$clc" -j4 src/compiler/clc/mesa_clc src/compiler/spirv/vtn_bindgen2 > "$HOME/clc-ninja.log" 2>&1 ||
	{ grep -E "error|FAILED" "$HOME/clc-ninja.log" | head -30; echo "vm-mesa: FAILED: mesa_clc"; exit 1; }
mkdir -p "$HOME/clc-bin"
ln -sf "$clc/src/compiler/clc/mesa_clc" "$HOME/clc-bin/mesa_clc"
ln -sf "$clc/src/compiler/spirv/vtn_bindgen2" "$HOME/clc-bin/vtn_bindgen2"
export PATH=$HOME/clc-bin:$PATH
echo "vm-mesa: mesa_clc after $(($(date +%s) - start)) s"

if [ ! -f "$build/build.ninja" ]; then
	meson setup "$build" "$src" --prefix=/opt/pvg-mesa --libdir=lib \
		-Dbuildtype=release -Dgallium-drivers=zink -Dvulkan-drivers=kosmickrisp \
		-Dkosmickrisp-bridge=pvg -Dplatforms=x11,wayland -Degl=enabled -Dgbm=enabled \
		-Dopengl=true -Dgles1=disabled -Dgles2=enabled -Dglx=dri -Dglvnd=enabled \
		-Dglvnd-vendor-name=pvg -Dllvm=disabled -Dmesa-clc=system -Dvideo-codecs= \
		-Dvalgrind=disabled -Dlibunwind=disabled -Dlmsensors=disabled \
		-Dc_link_args=-Wl,-rpath,/opt/pvg-mesa/lib -Dcpp_link_args=-Wl,-rpath,/opt/pvg-mesa/lib \
		"$@" > "$HOME/mesa-setup.log" 2>&1 ||
		{ tail -40 "$HOME/mesa-setup.log"; echo "vm-mesa: FAILED: meson setup"; exit 1; }
	sed -n '/^Message\|^  /p' "$HOME/mesa-setup.log" | tail -40
fi
ninja -C "$build" -j4 > "$HOME/mesa-ninja.log" 2>&1 ||
	{ grep -E "error|FAILED" "$HOME/mesa-ninja.log" | head -40; echo "vm-mesa: FAILED: ninja"; exit 1; }
tail -2 "$HOME/mesa-ninja.log"
rm -rf "$stage"
DESTDIR=$stage ninja -C "$build" install > "$HOME/mesa-install.log" 2>&1 ||
	{ tail -20 "$HOME/mesa-install.log"; echo "vm-mesa: FAILED: install"; exit 1; }
# The libraries' names have no spaces.
# shellcheck disable=SC2044
for lib in $(find "$stage" -name "*.so*" -type f); do
	readelf -d "$lib" | grep -q "libLLVM" && echo "vm-mesa: $lib links LLVM"
	readelf -d "$lib" | grep -q "RUNPATH.*/opt/pvg-mesa/lib" || echo "vm-mesa: $lib has no run path"
done
find "$stage" -name "*.so*" -type f | sed "s|^$stage||; s/^/vm-mesa: /"
echo "vm-mesa: ok after $(($(date +%s) - start)) s"
