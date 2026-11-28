#!/bin/sh
# SPDX-License-Identifier: MIT
#
# Builds DXMT's d3d11 and dxgi for Windows on arm64, under our own names
# ruxnd3d11.dll and ruxndxgi.dll, over our winemetal.dll (ruxnwddm's task
# escapes, pvg_wire, and airconv linked in), the test program d3d11tri.exe,
# and what checks them on the Mac; then gathers the files for the stick.
#
# Usage: build.sh ROOT
#   ROOT     a build folder, with
#            llvm-mingw unpacked under .tools/llvm-mingw, DXMT's clone under
#            .tools/src/dxmt (at faa73e4), meson in .venv, glslang under
#            build/shader-tools, src/air's airc under build/mesa-air, and
#            airconv and LLVM 15 for Windows on arm64 under .tools
#            (airconv-build.sh)
# The output goes to ROOT/build/dxmt-win/, the files for the stick to
# ROOT/build/dxmt-win/stick/.
#
# Nothing here is signed: Windows does not ask user-mode DLLs and programs
# for a signature (only the driver needs the test certificate).
#
# The driver's interface, pvg_wire and the triangle's shaders are taken from
# the lab repo as they are at PIN (driver 0.5.0.0, the stage E disk), not
# from the working tree, which another stage is changing.
set -eu

root=$1
src=$(cd "$(dirname "$0")" && pwd)
repo=$(git -C "$src" rev-parse --show-toplevel 2>/dev/null || true)
# The wire library and the triangle's inputs: from the folder WIRE when it
# is set, else from the lab repo's history at PIN.
wire=${WIRE:-$src/../wire}
# RUXN_STANDIN=0 builds winemetal.dll without the first slice's triangle
# stand-in (which needs glslang and the unpublished airc); CHECKS=0 leaves
# out the checks that run on the Mac (DXMT's parser, the wire bytes against
# pvgtri's, Metal's reference picture). Both are on by default, as the lab
# has always built.
standin=${RUXN_STANDIN:-1}
checks=${CHECKS:-1}
out=$root/build/dxmt-win
mingw=$root/.tools/llvm-mingw/bin
cc=$mingw/aarch64-w64-mingw32-clang
dxmt_clone=$root/.tools/src/dxmt
dxmt_commit=faa73e4
PIN=2c629031

mkdir -p "$out/pinned" "$out/gen" "$out/obj" "$out/host" "$out/stick"

# The pinned inputs.
for file in ruxnpvg_ioctl.h pvg_wire.h pvg_wire.c triangle.vert triangle.frag metalref.m \
		cube_math.h scene.h; do
	if [ -n "$wire" ]; then
		cp "$wire/$file" "$out/pinned/$file"
	else
		cp "$src/../wire/$file" "$out/pinned/$file"
	fi
done

# DXMT: a copy of the clone (never the clone itself) with our diff.
if [ ! -d "$out/src" ]; then
	test "$(git -C "$dxmt_clone" rev-parse --short=7 HEAD)" = "$dxmt_commit"
	cp -c -R "$dxmt_clone" "$out/src"
	git -C "$out/src" apply "$src/dxmt-faa73e4.diff"
	git -C "$out/src" add -N src/dxmt/pvg/dxmt_command.h build-aarch64-llvm-mingw.txt
fi
git -C "$out/src" diff "$dxmt_commit" > "$out/dxmt.diff.now"
# The diff's first line is its SPDX line, which git diff does not write.
tail -n +2 "$src/dxmt-faa73e4.diff" | cmp -s "$out/dxmt.diff.now" - || {
	echo "build.sh: the DXMT copy differs from dxmt-faa73e4.diff (see $out/dxmt.diff.now)" >&2
	exit 1
}

# The triangle's libraries, as ruxnpvg/build.sh makes them: glslang and our
# airc, nothing of the host vendor's.
glslang=$root/build/shader-tools/glslang/StandAlone/glslangValidator
airc=$root/build/mesa-air/src/air/airc
standin_flag=
if [ "$standin" = 1 ]; then
	"$glslang" -V -o "$out/gen/triangle.vert.spv" "$out/pinned/triangle.vert" > "$out/gen/glslang.log"
	"$glslang" -V -o "$out/gen/triangle.frag.spv" "$out/pinned/triangle.frag" >> "$out/gen/glslang.log"
	"$airc" --flip-y -o "$out/gen/tri-vert.metallib" "$out/gen/triangle.vert.spv"
	"$airc" -o "$out/gen/tri-frag.metallib" "$out/gen/triangle.frag.spv"
	(cd "$out/gen" && xxd -i -n tri_vert_metallib tri-vert.metallib > libraries.h &&
		xxd -i -n tri_frag_metallib tri-frag.metallib >> libraries.h)
else
	standin_flag=-DRUXN_NO_STANDIN
fi

# The shaders as DXBC, and DXMT's own parser reading them on the Mac.
python3 "$src/make_dxbc.py" "$out/gen"
mingw_include=$root/.tools/llvm-mingw/aarch64-w64-mingw32/include
if [ "$checks" = 1 ]; then
clang++ -std=c++20 -O1 -I"$out/src/include" -I"$out/src/libs" -I"$out/src/libs/DXBCParser" \
	-I"$out/src/include/native/windows" -idirafter "$mingw_include" \
	-Wno-unknown-pragmas -Wno-extern-c-compat \
	"$src/dxbccheck.cpp" "$out/src/libs/DXBCParser/"*.cpp -o "$out/host/dxbccheck"
"$out/host/dxbccheck" "$out/gen/triangle-vs.dxbc" > "$out/host/dxbccheck.txt"
"$out/host/dxbccheck" "$out/gen/triangle-ps.dxbc" >> "$out/host/dxbccheck.txt"

# The wire bytes against pvgtri's, on the Mac.
clang -std=gnu11 -O1 -Wall -Wextra -Wno-unused-parameter -I"$src" -I"$out/pinned" \
	-I"$out/src/src/winemetal" "$src/wiretest.c" "$src/translate.c" "$src/translate_blit.c" "$src/pvg_wire_more.c" "$out/pinned/pvg_wire.c" \
	-o "$out/host/wiretest"
"$out/host/wiretest" > "$out/host/wiretest.txt"

# The reference: the same two libraries drawn by the Mac's Metal, by
# ruxnpvg's metalref.m (public calls only).
if [ "$standin" = 1 ]; then
	clang -fobjc-arc -framework Metal -framework Foundation "$out/pinned/metalref.m" -o "$out/host/metalref"
	"$out/host/metalref" "$out/gen/tri-vert.metallib" "$out/gen/tri-frag.metallib" "$out/ref-tri.ppm" > /dev/null
fi
fi

# Our winemetal.dll, with stubs for the rest of winemetal.h.
# DXMT's helper functions we have, written as AIR (helpers/*.ll), each made
# into a library by our own air2metallib (airconv's library writer, on the
# Mac) and carried in winemetal as bytes (gen/helper_libraries.h).
llvm_mac=$root/.tools/llvm15-mac
# shellcheck disable=SC2046
clang++ -std=c++17 -O1 -w -I"$llvm_mac/include" -I"$root/.tools/airconv-src/src/airconv" \
	"$src/helpers/air2metallib.cpp" "$root/.tools/airconv-mac/lib/libairconv.a" -L"$llvm_mac/lib" \
	$("$llvm_mac/bin/llvm-config" --libs irreader bitwriter core support asmparser bitreader) \
	-lz -lcurses -lm -o "$out/host/air2metallib"
for helper in "$src"/helpers/*.ll; do
	"$out/host/air2metallib" "$helper" "$out/gen/$(basename "$helper" .ll).metallib"
done
python3 - "$out/gen" "$src"/helpers/*.ll <<'PY'
import os, sys
gen, sources = sys.argv[1], sys.argv[2:]
names = [os.path.basename(p)[:-3] for p in sources]
lines = ["/* Made by build.sh from the AIR in helpers through air2metallib. */", ""]
for i, n in enumerate(names):
    data = open(os.path.join(gen, n + ".metallib"), "rb").read()
    lines.append("static const unsigned char helper_%d[%d] = {%s};" % (i, len(data), ",".join(str(b) for b in data)))
lines.append("#define HELPER_COUNT %d" % len(names))
lines.append("static const char *const helper_names[HELPER_COUNT] = {%s};" % ",".join('"%s"' % n for n in names))
lines.append("static const unsigned char *const helper_bytes[HELPER_COUNT] = {%s};" % ",".join("helper_%d" % i for i in range(len(names))))
lines.append("static const unsigned long helper_sizes[HELPER_COUNT] = {%s};" % ",".join("sizeof(helper_%d)" % i for i in range(len(names))))
open(os.path.join(gen, "helper_libraries.h"), "w").write("\n".join(lines) + "\n")
PY

wm_parts="winemetal_pvg wm_objects wm_resources wm_views wm_pipelines wm_commands wm_memory"
# shellcheck disable=SC2086,SC2046
python3 "$src/gen_stubs.py" "$out/src/src/winemetal/winemetal.h" "$out/gen/stubs.c" \
	$(for part in $wm_parts; do echo "$src/$part.c"; done)
include="-I$src -I$src/../ruxnwddm -I$out/gen -I$out/pinned -I$out/src/src/winemetal -I$out/src/src/airconv"
for file in "$src/winemetal_pvg.c" "$src/wm_objects.c" "$src/wm_resources.c" "$src/wm_views.c" "$src/wm_pipelines.c" "$src/wm_commands.c" "$src/wm_memory.c" "$src/wm_helpers.c" "$src/translate.c" "$src/translate_blit.c" "$src/pvg_kmt.c" "$src/wm_shader.c" "$src/wm_surface.c" "$src/wm_place.c" "$src/wm_refused.c" "$src/pvg_wire_more.c" "$out/gen/stubs.c" \
		"$out/pinned/pvg_wire.c"; do
	# shellcheck disable=SC2086
	"$cc" -O2 -std=gnu11 -Wall -Wextra -Werror -Wno-unused-parameter $standin_flag $include -c "$file" \
		-o "$out/obj/$(basename "$file" .c).o"
done
# airconv, with its SM50* functions renamed airconv_SM50* so that
# wm_shader.c's exports can stand in front of them.
airconv_lib=$root/.tools/airconv-win-arm64/lib/libairconv.a
llvm_lib=$root/.tools/llvm15-win-arm64/lib
renames=
for name in Initialize Destroy Compile GetCompiledBitcode DestroyBitcode GetErrorMessage FreeError \
		CompileTessellationPipelineHull CompileTessellationPipelineDomain \
		CompileGeometryPipelineVertex CompileGeometryPipelineGeometry GetArgumentsInfo; do
	renames="$renames --redefine-sym SM50$name=airconv_SM50$name"
done
# shellcheck disable=SC2086
"$mingw/llvm-objcopy" $renames "$airconv_lib" "$out/obj/libairconv-renamed.a"
# The LLVM libraries airconv needs, as DXMT's src/airconv/meson.build lists them.
llvm_libs="-lLLVMPasses -lLLVMTarget -lLLVMObjCARCOpts -lLLVMCoroutines -lLLVMipo \
	-lLLVMInstrumentation -lLLVMVectorize -lLLVMLinker -lLLVMIRReader -lLLVMAsmParser -lLLVMFrontendOpenMP \
	-lLLVMScalarOpts -lLLVMInstCombine -lLLVMAggressiveInstCombine -lLLVMTransformUtils -lLLVMBitWriter \
	-lLLVMAnalysis -lLLVMProfileData -lLLVMSymbolize -lLLVMDebugInfoPDB -lLLVMDebugInfoMSF \
	-lLLVMDebugInfoDWARF -lLLVMObject -lLLVMTextAPI -lLLVMMCParser -lLLVMMC -lLLVMDebugInfoCodeView \
	-lLLVMBitReader -lLLVMCore -lLLVMRemarks -lLLVMBitstreamReader -lLLVMBinaryFormat -lLLVMSupport \
	-lLLVMDemangle"
# shellcheck disable=SC2086
"$cc++" -shared -static -s -o "$out/obj/winemetal.dll" "$out/obj/winemetal_pvg.o" "$out/obj/wm_objects.o" \
	"$out/obj/wm_resources.o" "$out/obj/wm_views.o" "$out/obj/wm_pipelines.o" "$out/obj/wm_commands.o" "$out/obj/wm_memory.o" "$out/obj/wm_helpers.o" "$out/obj/translate.o" "$out/obj/translate_blit.o" \
	"$out/obj/pvg_kmt.o" "$out/obj/wm_shader.o" "$out/obj/wm_surface.o" "$out/obj/wm_place.o" "$out/obj/wm_refused.o" "$out/obj/pvg_wire_more.o" "$out/obj/stubs.o" "$out/obj/pvg_wire.o" \
	"$out/obj/libairconv-renamed.a" -L"$llvm_lib" $llvm_libs -lpsapi -lshell32 -lole32 -luuid -ladvapi32 \
	-lntdll -Wl,--out-implib,"$out/obj/libwinemetal.dll.a"

# DXMT's d3d11.dll and dxgi.dll, linked against it.
if [ ! -f "$out/meson/build.ninja" ]; then
	(cd "$out/src" && PATH=$mingw:$PATH "$root/.venv/bin/meson" setup "$out/meson" \
		--cross-file build-aarch64-llvm-mingw.txt --buildtype release \
		-Dpvg_winemetal="$out/obj/libwinemetal.dll.a" -Dwine_builtin_dll=false)
fi
if grep -q xcrun "$out/meson/build.ninja"; then
	echo "build.sh: the build would run Xcode's shader compiler" >&2
	exit 1
fi
PATH=$mingw:$PATH ninja -C "$out/meson" src/d3d11/ruxnd3d11.dll src/dxgi/ruxndxgi.dll

# The test program.
"$cc" -O2 -Wall -Wextra -Werror -mconsole -I"$out/gen" "$src/d3d11tri.c" -o "$out/obj/d3d11tri.exe" \
	-ldxgi -ld3d11 -ldxguid
"$cc" -O2 -Wall -Wextra -Werror -mconsole "$src/spin.c" -o "$out/obj/spin.exe" -ldxgi -ld3d11 -ldxguid -luser32
"$cc" -O2 -Wall -Wextra -Werror -I"$src/../ruxnwddm" -c "$src/pvg_kmt.c" -o "$out/obj/memgrow_kmt.o"
"${cc}++" -O2 -Wall -Wextra -Werror -mconsole -I"$src/../ruxnwddm" "$src/memgrow.cpp" "$out/obj/memgrow_kmt.o" \
	-o "$out/obj/memgrow.exe" -ld3d11 -ldxguid
"${cc}++" -O2 -Wall -Wextra -Werror -mconsole "$src/videoprobe.cpp" -o "$out/obj/videoprobe.exe" -ld3d11 -ldxguid
"${cc}++" -O2 -Wall -Wextra -Werror -mconsole "$src/outprobe.cpp" -o "$out/obj/outprobe.exe" -ldxgi -ld3d11 -ldxguid -luser32
# kmtprobe: d3dkmthk.h from the SDK's shared headers, searched after llvm-mingw's own.
"${cc}++" -O2 -Wall -Wextra -Werror -mconsole -idirafter "$root/.tools/wdk/microsoft.windows.sdk.cpp/c/Include/10.0.28000.0/shared" \
	"$src/kmtprobe.cpp" -o "$out/obj/kmtprobe.exe" -ldxgi -ld3d11 -ldxguid -luser32 -lgdi32

cp "$out/meson/src/d3d11/ruxnd3d11.dll" "$out/meson/src/dxgi/ruxndxgi.dll" "$out/obj/winemetal.dll" \
	"$out/obj/d3d11tri.exe" "$out/obj/spin.exe" "$out/obj/memgrow.exe" "$out/obj/videoprobe.exe" "$out/obj/outprobe.exe" "$out/obj/kmtprobe.exe" \
	"$out/stick/"
# The lab's run step for the stick, where the tree has one.
if [ -f "$src/dxmttri.cmd" ]; then cp "$src/dxmttri.cmd" "$out/stick/"; fi
if [ "$checks" = 1 ]; then
	sed -n '$p' "$out/host/wiretest.txt"
	grep -c "parsed to the end" "$out/host/dxbccheck.txt" | sed 's/^/dxbccheck: shaders parsed: /'
	[ "$standin" = 1 ] && shasum -a 256 "$out/ref-tri.ppm"
fi
echo "built; for the stick: $out/stick/"
