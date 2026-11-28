#!/bin/sh
# SPDX-License-Identifier: GPL-2.0
#
# Builds ruxnwddm.sys, the WDDM display miniport, and wddmtool.exe for
# Windows on arm64 with clang and lld-link, as ruxnpvg/build.sh does: marks
# the driver's sections not paged, and signs what it builds when SIGN_CERT
# and SIGN_KEY name a certificate (PEM) and its key; otherwise the outputs
# are left unsigned under their usual names. With STICK=1 it also gathers
# what goes on the lab's USB stick, with the files STICK_EXTRA names.
# The lab builds through win/lab/build.sh, which sets all of these.
#
# Usage: build.sh ROOT
#   ROOT     a build folder, with
#            the WDK and SDK unpacked under .tools/wdk
# The output goes to ROOT/build/ruxnwddm/, the stick's files to
# ROOT/build/ruxnwddm/stick/.
set -eu

root=$1
src=$(cd "$(dirname "$0")" && pwd)
out=$root/build/ruxnwddm
lld_link=${LLD_LINK:-/opt/homebrew/bin/lld-link}

wdk=$root/.tools/wdk/microsoft.windows.wdk.arm64/c
sdk=$root/.tools/wdk/microsoft.windows.sdk.cpp/c
sdk_lib=$root/.tools/wdk/microsoft.windows.sdk.cpp.arm64/c/um/arm64
version=10.0.28000.0

mkdir -p "$out"

# Microsoft's C dialect for arm64, as for ruxnpvg. Their headers trip a few
# warnings our own code does not, and only those are silenced.
base="--target=aarch64-pc-windows-msvc -fms-extensions -fms-compatibility
	-fms-compatibility-version=19.40 -ffreestanding -nostdinc
	-fno-stack-protector
	-isystem $(clang -print-resource-dir)/include
	-D_ARM64_ -DARM64 -D_WIN64
	-DNTDDI_VERSION=0x0A000010 -D_WIN32_WINNT=0x0A00 -DWINVER=0x0A00
	-O2 -Wall -Wextra -Werror
	-Wno-unknown-pragmas -Wno-ignored-attributes -Wno-ignored-pragma-intrinsic
	-Wno-microsoft-anon-tag -Wno-pragma-pack"

# The driver: the kernel's headers, and the display headers, which are
# split between the WDK's shared folder (d3dkmddi.h) and the SDK's
# (d3dukmdt.h, d3dkmdt.h).
kernel="$base -D_KERNEL_MODE -mgeneral-regs-only
	-isystem $wdk/Include/$version/km/crt
	-isystem $wdk/Include/$version/km
	-isystem $wdk/Include/$version/shared
	-isystem $sdk/Include/$version/shared"

# wddmtool: the user-mode headers, the few C headers windows.h wants taken
# from the WDK's kernel CRT folder, as for pvgtest.
user="$base -DUNICODE -D_UNICODE -DWIN32_LEAN_AND_MEAN -fno-builtin
	-isystem $sdk/Include/$version/um
	-isystem $sdk/Include/$version/shared
	-isystem $wdk/Include/$version/km/crt"

sign() {
	rm -f "$2"
	if [ -n "${SIGN_CERT:-}" ] && [ -n "${SIGN_KEY:-}" ]; then
		osslsigncode sign -certs "$SIGN_CERT" -key "$SIGN_KEY" -h sha256 -in "$1" -out "$2" > /dev/null
	else
		cp "$1" "$2"
	fi
}

objects=
for file in driver driver_full adapter caps allocation paging render flip scanout vidpn present interrupt escape ring pages record blt_present alloc_list klog \
		pvg_client pvg_task pvg_object pvg_submit pvg_view pvg_bo pvg_mapper pvg_surface pvg_display; do
	# shellcheck disable=SC2086
	clang $kernel -c "$src/$file.c" -o "$out/$file.o"
	objects="$objects $out/$file.o"
	# With ANALYZE=1, clang's static analyzer looks at each file too.
	if [ "${ANALYZE:-0}" = 1 ]; then
		# shellcheck disable=SC2086
		clang $kernel --analyze -Xanalyzer -analyzer-output=text "$src/$file.c" -o /dev/null
	fi
done

# /tsaware:no matters: without it the kernel refuses the image (error 487).
# GsDriverEntry, in Microsoft's buffer overrun library, sets the security
# cookie the display library's code checks and then calls DriverEntry.
km=$wdk/Lib/$version/km/ARM64
# shellcheck disable=SC2086
"$lld_link" /nologo /machine:arm64 /driver /subsystem:native,10.00 \
	/osversion:10.0 /version:10.0 /nodefaultlib /release /tsaware:no \
	/entry:GsDriverEntry /map:"$out/ruxnwddm.map" /out:"$out/ruxnwddm-unsigned.sys" $objects \
	"$km/displib.lib" "$km/ntoskrnl.lib" "$km/hal.lib" "$km/bufferoverflowfastfailk.lib" \
	"$km/libcntpr.lib" "$km/cng.lib"

python3 "$src/nonpaged.py" "$out/ruxnwddm-unsigned.sys"
sign "$out/ruxnwddm-unsigned.sys" "$out/ruxnwddm.sys"

# shellcheck disable=SC2086
clang $user -c "$src/wddmtool.c" -o "$out/wddmtool.o"
"$lld_link" /nologo /machine:arm64 /subsystem:console /nodefaultlib /release \
	/entry:wddmtool_entry /out:"$out/wddmtool-unsigned.exe" "$out/wddmtool.o" \
	"$sdk_lib/kernel32.Lib" "$sdk_lib/Gdi32.Lib"
sign "$out/wddmtool-unsigned.exe" "$out/wddmtool.exe"

# ruxnumd.dll, the user-mode driver of the full driver: the Direct3D DDI
# headers live in the WDK's um folder. No C runtime, as for wddmtool.
umd_objects=
for file in umd_adapter umd_device umd_format umd_resource umd_present umd_stubs umd_log umd_util \
		umd_inner umd_resource_inner umd_view umd_state umd_shader umd_draw umd_compute umd_blt umd_dxbc umd_md5 \
		umd_residency umd_video; do
	# shellcheck disable=SC2086
	clang $user -isystem "$wdk/Include/$version/um" -isystem "$wdk/Include/$version/shared" \
		-c "$src/$file.c" -o "$out/$file.o"
	umd_objects="$umd_objects $out/$file.o"
	if [ "${ANALYZE:-0}" = 1 ]; then
		# shellcheck disable=SC2086
		clang $user -isystem "$wdk/Include/$version/um" -isystem "$wdk/Include/$version/shared" \
			--analyze -Xanalyzer -analyzer-output=text "$src/$file.c" -o /dev/null
	fi
done
# A version resource with the package's own version (DriverVer of
# ruxnwddm-full.inf), so programs that ask the DLL (Edge's GPU page, dxdiag)
# see the driver's version rather than 0.0.0.0.
rc=${LLVM_RC:-$root/.tools/llvm-mingw/bin/llvm-rc}
umd_version=$(sed -n 's/^DriverVer *= *[^,]*,\([0-9.]*\).*/\1/p' "$src/ruxnwddm-full.inf")
umd_commas=$(echo "$umd_version" | tr . ,)
cat > "$out/ruxnumd.rc" <<EOF_RC
1 VERSIONINFO
FILEVERSION $umd_commas
PRODUCTVERSION $umd_commas
FILEOS 0x40004
FILETYPE 0x3
FILESUBTYPE 0x4
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "RUXN LLC"
      VALUE "FileDescription", "ruxn PV Graphics user-mode driver"
      VALUE "FileVersion", "$umd_version"
      VALUE "ProductName", "ruxn PV Graphics"
      VALUE "ProductVersion", "$umd_version"
      VALUE "OriginalFilename", "ruxnumd.dll"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
EOF_RC
# llvm-rc reads a leading slash as an option, so it runs in the output folder.
(cd "$out" && "$rc" /FO ruxnumd.res ruxnumd.rc)
umd_objects="$umd_objects $out/ruxnumd.res"
# Direct3D's headers define their numeric limits as plain const variables,
# which C (unlike C++) gives external linkage, so every file holds a copy of
# the same constants; /force:multiple keeps one.
# shellcheck disable=SC2086
"$lld_link" /nologo /machine:arm64 /dll /subsystem:windows /nodefaultlib /release /force:multiple \
	/entry:DllMain /export:OpenAdapter10_2 /out:"$out/ruxnumd-unsigned.dll" $umd_objects \
	"$sdk_lib/kernel32.Lib" "$sdk_lib/AdvAPI32.Lib" "$sdk_lib/dxguid.lib"
sign "$out/ruxnumd-unsigned.dll" "$out/ruxnumd.dll"

# umdtest: makes a Direct3D 11 device on our adapter, with the user-mode
# driver's log on.
# shellcheck disable=SC2086
clang $user -c "$src/umdtest.c" -o "$out/umdtest.o"
"$lld_link" /nologo /machine:arm64 /subsystem:console /nodefaultlib /release \
	/entry:umdtest_entry /out:"$out/umdtest-unsigned.exe" "$out/umdtest.o" \
	"$sdk_lib/kernel32.Lib" "$sdk_lib/Shell32.Lib" "$sdk_lib/dxgi.lib" "$sdk_lib/d3d11.lib"
sign "$out/umdtest-unsigned.exe" "$out/umdtest.exe"

# dispcfg: the display topology (display settings only), with llvm-mingw's
# C runtime, as the dxmt-win programs are built.
"$root/.tools/llvm-mingw/bin/aarch64-w64-mingw32-clang" -O2 -Wall -Wextra -Werror -mconsole \
	"$src/dispcfg.c" -o "$out/dispcfg.exe" -luser32

if [ "${STICK:-0}" = 1 ]; then
	mkdir -p "$out/stick"
	cp "$out/dispcfg.exe" "$out/umdtest.exe" "$out/ruxnwddm.sys" "$out/ruxnumd.dll" "$out/wddmtool.exe" "$src/ruxnwddm.inf" "$src/wddm.ps1" \
		"$src/wddm-install.cmd" "$src/wddm-check.cmd" "$src/wddm-ring.cmd" "$src/wddm-solo.cmd" "$src/wddm-full.cmd" "$src/wddm-dxdiag.cmd" "$src/wddm-umd.cmd" "$src/wddm-collect.cmd" "$src/wddm-dwm.cmd" "$src/wddm-trace.cmd" "$src/wddm-dwmrun.cmd" "$src/ruxnwddm-full.inf" \
		${STICK_EXTRA:-} "$out/stick/"
	echo "built $out/ruxnwddm.sys and wddmtool.exe; for the stick: $out/stick/"
else
	echo "built $out/ruxnwddm.sys and wddmtool.exe"
fi
