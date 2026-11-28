#!/bin/sh
# SPDX-License-Identifier: MIT
#
# airconv-build.sh: build DXMT's shader converter airconv (DXBC to a Metal
# library, needs LLVM 15) two ways, from source, with no Metal tools from Xcode:
#
#   1. natively for macOS arm64: LLVM 15 static libs, the airconv CLI and
#      airconvtest (our driver over the exported SM50* C functions);
#   2. for Windows on arm64 (aarch64-w64-mingw32, llvm-mingw): LLVM 15 static
#      libs, libairconv.a (airconv + DXBCParser) and airconvtest.exe, so another
#      DLL can link airconv statically and call SM50Initialize/SM50Compile/...
#
# Usage: sh airconv-build.sh ROOT [step ...]
#   ROOT is a build folder that holds
#   .tools/llvm-mingw (llvm-mingw, LLVM 23), .venv/bin/{meson,cmake} and
#   .tools/src/dxmt (DXMT at faa73e4). Everything new goes under ROOT/.tools.
#   Steps (default: all, in this order): fetch llvm_mac llvm_win standin
#   airconv_mac airconv_win cleanup
#
# Inputs
#   LLVM 15.0.7 source, official release:
#     https://github.com/llvm/llvm-project/releases/download/llvmorg-15.0.7/llvm-project-15.0.7.src.tar.xz
#     110936452 bytes, SHA-256
#     8b5fcb24b4128cf04df1b0b9410ce8b1a729cb3c544e6da885d234280dedeac6
#     Only llvm/, cmake/ and third-party/ are unpacked.
#   DXMT faa73e4 (https://github.com/3Shain/dxmt), already cloned.
#   d3d11.h/d3d12.h for the native build: mingw-w64 headers shipped in
#   llvm-mingw (DXMT's include/native/directx submodule is not used).
#
# Patches (beside this script)
#   llvm15-mingw.diff    LLVM 15 against llvm-mingw's newer libc++
#   airconv-faa73e4.diff AIRCONV_STATIC: SM50* without dllimport/dllexport
#   airconv-standin.ll   empty AIR module standing in for the three helper
#                        shaders DXMT compiles with Xcode's Metal compiler
#
# Linking airconv into another Windows DLL (no SM50* dllexport; the code
# that includes airconv_public.h must define AIRCONV_STATIC):
#   aarch64-w64-mingw32-clang++ -shared -static ... \
#     -I.tools/airconv-win-arm64/include -DAIRCONV_STATIC \
#     -L.tools/airconv-win-arm64/lib -lairconv \
#     -L.tools/llvm15-win-arm64/lib -lLLVMPasses ... -lLLVMDemangle (LLVM_LIBS,
#     in that order) -lpsapi -lshell32 -lole32 -luuid -ladvapi32 -lntdll
#
# LLVM is built with no backend (LLVM_TARGETS_TO_BUILD=""): airconv only
# builds IR and writes AIR bitcode. Only the libraries airconv's meson.build
# lists (llvm_deps) are built. The native build compiles with the system
# clang; the Windows build with llvm-mingw's clang, libc++ linked statically.

set -eu

[ $# -ge 1 ] || { echo "usage: sh $0 ROOT [step ...]" >&2; exit 2; }
M5=$(cd "$1" && pwd)
shift
HERE=$(cd "$(dirname "$0")" && pwd)
TOOLS=$M5/.tools
JOBS=$(sysctl -n hw.ncpu 2>/dev/null || nproc)
CMAKE=$M5/.venv/bin/cmake
MINGW=$TOOLS/llvm-mingw/bin
WINCC=$MINGW/aarch64-w64-mingw32-clang
WINCXX=$MINGW/aarch64-w64-mingw32-clang++

LLVM_URL=https://github.com/llvm/llvm-project/releases/download/llvmorg-15.0.7/llvm-project-15.0.7.src.tar.xz
LLVM_SHA256=8b5fcb24b4128cf04df1b0b9410ce8b1a729cb3c544e6da885d234280dedeac6
LLVM_DL=$TOOLS/llvm15-dl                 # the downloaded archive
LLVM_SRC=$TOOLS/llvm15-src               # unpacked + llvm15-mingw.diff
LLVM_MAC=$TOOLS/llvm15-mac               # native libs, headers, tools
LLVM_WIN=$TOOLS/llvm15-win-arm64         # Windows arm64 libs, headers
DXMT_SRC=$TOOLS/src/dxmt                 # read-only clone
AIRCONV_SRC=$TOOLS/airconv-src           # our copy + airconv-faa73e4.diff
AIRCONV_MAC=$TOOLS/airconv-mac
AIRCONV_WIN=$TOOLS/airconv-win-arm64

# The LLVM libraries airconv links, from DXMT src/airconv/meson.build.
LLVM_LIBS="LLVMPasses LLVMTarget LLVMObjCARCOpts LLVMCoroutines LLVMipo
LLVMInstrumentation LLVMVectorize LLVMLinker LLVMIRReader LLVMAsmParser
LLVMFrontendOpenMP LLVMScalarOpts LLVMInstCombine LLVMAggressiveInstCombine
LLVMTransformUtils LLVMBitWriter LLVMAnalysis LLVMProfileData LLVMSymbolize
LLVMDebugInfoPDB LLVMDebugInfoMSF LLVMDebugInfoDWARF LLVMObject LLVMTextAPI
LLVMMCParser LLVMMC LLVMDebugInfoCodeView LLVMBitReader LLVMCore LLVMRemarks
LLVMBitstreamReader LLVMBinaryFormat LLVMSupport LLVMDemangle"

# Windows system libraries LLVMSupport needs.
WIN_SYSLIBS="-lpsapi -lshell32 -lole32 -luuid -ladvapi32 -lntdll"

say() { printf '\n== %s\n' "$*"; }

# ---------------------------------------------------------------- fetch
step_fetch() {
  say "fetch and unpack LLVM 15.0.7"
  mkdir -p "$LLVM_DL"
  archive=$LLVM_DL/llvm-project-15.0.7.src.tar.xz
  [ -f "$archive" ] || curl -fL --retry 3 -o "$archive" "$LLVM_URL"
  echo "$LLVM_SHA256  $archive" | shasum -a 256 -c -
  if [ ! -d "$LLVM_SRC/llvm" ]; then
    mkdir -p "$LLVM_SRC"
    tar -xJf "$archive" -C "$LLVM_SRC" --strip-components=1 \
      llvm-project-15.0.7.src/llvm llvm-project-15.0.7.src/cmake \
      llvm-project-15.0.7.src/third-party
    patch -d "$LLVM_SRC" -p1 < "$HERE/llvm15-mingw.diff"
  fi
  if [ ! -d "$AIRCONV_SRC" ]; then
    cp -c -R "$DXMT_SRC" "$AIRCONV_SRC" 2>/dev/null || cp -R "$DXMT_SRC" "$AIRCONV_SRC"
    patch -d "$AIRCONV_SRC" -p1 < "$HERE/airconv-faa73e4.diff"
  fi
}

# Common LLVM configuration: libraries only, no backend, no optional deps.
llvm_cmake_args() {
  echo -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
    -DLLVM_TARGETS_TO_BUILD= -DLLVM_ENABLE_PROJECTS= \
    -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_EXAMPLES=OFF \
    -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_INCLUDE_DOCS=OFF \
    -DLLVM_BUILD_TOOLS=OFF -DLLVM_ENABLE_ZLIB=OFF -DLLVM_ENABLE_ZSTD=OFF \
    -DLLVM_ENABLE_LIBXML2=OFF -DLLVM_ENABLE_TERMINFO=OFF \
    -DLLVM_ENABLE_LIBEDIT=OFF -DLLVM_ENABLE_LIBPFM=OFF \
    -DLLVM_ENABLE_BINDINGS=OFF -DLLVM_ENABLE_ASSERTIONS=OFF
}

# Install what airconv needs from a build dir: headers (source + generated)
# and the listed static libraries.
llvm_install() { # build_dir prefix libsuffix
  mkdir -p "$2/include" "$2/lib"
  rsync -a --exclude=CMakeLists.txt "$LLVM_SRC/llvm/include/" "$2/include/"
  rsync -a --include='*/' --include='*.h' --include='*.def' --include='*.inc' \
    --exclude='*' --prune-empty-dirs "$1/include/" "$2/include/"
  find "$2/include" \( -name '*.cmake' -o -name '*.in' -o -name config.h \) -delete
  for l in $LLVM_LIBS; do cp "$1/lib/lib$l.a" "$2/lib/"; done
}

# ---------------------------------------------------------------- llvm_mac
step_llvm_mac() {
  say "LLVM 15 native (macOS arm64): libraries, llvm-tblgen, llvm-as, llvm-dis, llvm-config"
  b=$TOOLS/llvm15-mac-build
  # shellcheck disable=SC2046
  "$CMAKE" -S "$LLVM_SRC/llvm" -B "$b" $(llvm_cmake_args) \
    -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 \
    -DLLVM_INCLUDE_UTILS=ON -DLLVM_BUILD_UTILS=ON
  # shellcheck disable=SC2086
  ninja -C "$b" -j"$JOBS" $LLVM_LIBS llvm-tblgen llvm-as llvm-dis llvm-config
  llvm_install "$b" "$LLVM_MAC"
  mkdir -p "$LLVM_MAC/bin"
  for t in llvm-tblgen llvm-as llvm-dis llvm-config; do cp "$b/bin/$t" "$LLVM_MAC/bin/"; done
}

# ---------------------------------------------------------------- llvm_win
step_llvm_win() {
  say "LLVM 15 for aarch64-w64-mingw32 (llvm-mingw), host llvm-tblgen from llvm_mac"
  b=$TOOLS/llvm15-win-arm64-build
  # shellcheck disable=SC2046
  "$CMAKE" -S "$LLVM_SRC/llvm" -B "$b" $(llvm_cmake_args) \
    -DCMAKE_SYSTEM_NAME=Windows -DCMAKE_SYSTEM_PROCESSOR=ARM64 \
    -DCMAKE_C_COMPILER="$WINCC" -DCMAKE_CXX_COMPILER="$WINCXX" \
    -DCMAKE_RC_COMPILER="$MINGW/aarch64-w64-mingw32-windres" \
    -DCMAKE_AR="$MINGW/llvm-ar" -DCMAKE_RANLIB="$MINGW/llvm-ranlib" \
    -DCMAKE_FIND_ROOT_PATH="$TOOLS/llvm-mingw/aarch64-w64-mingw32" \
    -DCMAKE_FIND_ROOT_PATH_MODE_PROGRAM=NEVER \
    -DCMAKE_FIND_ROOT_PATH_MODE_LIBRARY=ONLY \
    -DCMAKE_FIND_ROOT_PATH_MODE_INCLUDE=ONLY \
    -DLLVM_HOST_TRIPLE=aarch64-w64-mingw32 \
    -DLLVM_TABLEGEN="$LLVM_MAC/bin/llvm-tblgen" \
    -DLLVM_INCLUDE_UTILS=OFF -DLLVM_BUILD_UTILS=OFF -DLLVM_ENABLE_DIA_SDK=OFF
  # shellcheck disable=SC2086
  ninja -C "$b" -j"$JOBS" $LLVM_LIBS
  llvm_install "$b" "$LLVM_WIN"
}

# ---------------------------------------------------------------- standin
# STAND-IN for the helper shaders (see airconv-standin.ll): one empty AIR
# module, assembled with our LLVM 15 and embedded as air_msad.h,
# air_samplepos.h and air_tessellation.h, the names DXMT's xxd step makes.
step_standin() {
  say "stand-in helper shader headers"
  gen=$TOOLS/airconv-gen
  mkdir -p "$gen"
  "$LLVM_MAC/bin/llvm-as" "$HERE/airconv-standin.ll" -o "$gen/standin.bc"
  for n in air_msad air_samplepos air_tessellation; do
    (cd "$gen" && cp standin.bc "$n" && xxd -i "$n" > "$n.h" && rm "$n")
  done
}

# Compile a list of sources in parallel into an object dir.
compile_all() { # cxx outdir "flags" src...
  c_cxx=$1 c_out=$2 c_flags=$3
  shift 3
  mkdir -p "$c_out"
  rm -f "$c_out/FAILED"
  c_n=0
  for c_src in "$@"; do
    c_obj=$c_out/$(basename "$(dirname "$c_src")")_$(basename "${c_src%.cpp}").o
    # shellcheck disable=SC2086
    { "$c_cxx" $c_flags -c "$c_src" -o "$c_obj" || echo "$c_src" >> "$c_out/FAILED"; } &
    c_n=$((c_n + 1))
    if [ $c_n -ge "$JOBS" ]; then wait; c_n=0; fi
  done
  wait
  if [ -f "$c_out/FAILED" ]; then echo "failed:" >&2; cat "$c_out/FAILED" >&2; exit 1; fi
}

# airconv sources from DXMT's meson.build (airconv_src).
airconv_sources() {
  sed -n '/^airconv_src = files/,/^\])/p' "$AIRCONV_SRC/src/airconv/meson.build" |
    grep -o "'[^']*\.cpp'" | tr -d "'" | sed "s|^|$AIRCONV_SRC/src/airconv/|"
}
DXBC_SOURCES="DXBCUtils BlobContainer ShaderBinary"
dxbc_sources() {
  for f in $DXBC_SOURCES; do echo "$AIRCONV_SRC/libs/DXBCParser/$f.cpp"; done
}

# Flags as in DXMT's meson files: airconv is built like LLVM (no exceptions,
# no RTTI; airconv_args), DXBCParser keeps C++ exceptions (it throws
# HRESULTs internally; dxbc_parser_args).
BASE_FLAGS="-std=c++20 -O2 -DNDEBUG -DDXMT_PAGE_SIZE=4096 -DAIRCONV_STATIC
-Wno-extern-c-compat -Wno-unknown-pragmas"
AIRCONV_FLAGS="$BASE_FLAGS -fno-exceptions -fno-rtti -funwind-tables
-D_FILE_OFFSET_BITS=64 -D__STDC_CONSTANT_MACROS -D__STDC_FORMAT_MACROS
-D__STDC_LIMIT_MACROS -Wno-unnecessary-virtual-specifier"

# Build libairconv.a (airconv + DXBCParser) into $1 with compiler $2, ar $3
# and the include/target flags $4.
build_libairconv() { # out cxx ar "flags"
  # shellcheck disable=SC2046
  compile_all "$2" "$1/obj" "$AIRCONV_FLAGS $4" $(airconv_sources)
  # shellcheck disable=SC2046
  compile_all "$2" "$1/obj" "$BASE_FLAGS $4" $(dxbc_sources)
  mkdir -p "$1/lib" "$1/bin" "$1/include"
  rm -f "$1/lib/libairconv.a"
  "$3" rcs "$1/lib/libairconv.a" "$1"/obj/*.o
  cp "$AIRCONV_SRC/src/airconv/airconv_public.h" "$1/include/"
}

# ---------------------------------------------------------------- airconv_mac
step_airconv_mac() {
  say "airconv native: libairconv.a, airconv (DXMT CLI), airconvtest"
  out=$AIRCONV_MAC
  # d3d11.h/d3d12.h/d3dcommon.h and what they include, from llvm-mingw's
  # mingw-w64 headers; DXMT's include/native/windows supplies windows.h.
  mkdir -p "$out/directx"
  gi=$TOOLS/llvm-mingw/generic-w64-mingw32/include
  for h in d3d11.h d3d12.h d3dcommon.h dxgi.h dxgicommon.h dxgiformat.h \
    dxgitype.h d3d12sdklayers.h d3d11sdklayers.h d3d10.h d3d10_1.h \
    d3d10sdklayers.h d3d10misc.h d3d10shader.h d3d10effect.h d3d10_1shader.h; do
    cp "$gi/$h" "$out/directx/"
  done
  incs="-mmacosx-version-min=14.0
    -I$AIRCONV_SRC/include -I$AIRCONV_SRC/libs
    -I$AIRCONV_SRC/include/native/windows -I$out/directx
    -I$AIRCONV_SRC/src/airconv -I$TOOLS/airconv-gen -I$LLVM_MAC/include"
  build_libairconv "$out" clang++ ar "$incs"
  llvm_link="-L$LLVM_MAC/lib $(for l in $LLVM_LIBS; do printf ' -l%s' "$l"; done)"
  # shellcheck disable=SC2086
  clang++ $AIRCONV_FLAGS $incs -c "$AIRCONV_SRC/src/airconv/airconv_cli.cpp" -o "$out/obj-cli.o"
  # shellcheck disable=SC2086
  clang++ -mmacosx-version-min=14.0 -o "$out/bin/airconv" "$out/obj-cli.o" \
    "$out/lib/libairconv.a" $llvm_link
  clang -std=c11 -O2 -mmacosx-version-min=14.0 -DAIRCONV_STATIC \
    -I"$out/include" -c "$HERE/airconvtest.c" -o "$out/airconvtest.o"
  # shellcheck disable=SC2086
  clang++ -mmacosx-version-min=14.0 -o "$out/bin/airconvtest" "$out/airconvtest.o" \
    "$out/lib/libairconv.a" $llvm_link
}

# ---------------------------------------------------------------- airconv_win
step_airconv_win() {
  say "airconv for aarch64-w64-mingw32: libairconv.a, airconvtest.exe"
  out=$AIRCONV_WIN
  incs="-DNOMINMAX -D_WIN32_WINNT=0xa00
    -I$AIRCONV_SRC/include -I$AIRCONV_SRC/libs -I$AIRCONV_SRC/src/airconv
    -I$TOOLS/airconv-gen -I$LLVM_WIN/include"
  build_libairconv "$out" "$WINCXX" "$MINGW/llvm-ar" "$incs"
  "$WINCC" -std=c11 -O2 -DAIRCONV_STATIC -I"$out/include" \
    -c "$HERE/airconvtest.c" -o "$out/airconvtest.o"
  # -static: libc++ and libunwind linked in; the C runtime is the UCRT.
  # -s: no symbol table.
  # shellcheck disable=SC2086,SC2046
  "$WINCXX" -static -s -o "$out/bin/airconvtest.exe" "$out/airconvtest.o" \
    -L"$out/lib" -lairconv -L"$LLVM_WIN/lib" \
    $(for l in $LLVM_LIBS; do printf ' -l%s' "$l"; done) $WIN_SYSLIBS
}

# ---------------------------------------------------------------- cleanup
step_cleanup() {
  say "remove build trees and objects (installed libs and headers stay)"
  rm -rf "$TOOLS/llvm15-mac-build" "$TOOLS/llvm15-win-arm64-build" \
    "$AIRCONV_MAC/obj" "$AIRCONV_WIN/obj" "$AIRCONV_MAC"/*.o "$AIRCONV_WIN"/*.o
}

[ $# -gt 0 ] || set -- fetch llvm_mac llvm_win standin airconv_mac airconv_win cleanup
for s in "$@"; do
  start=$(date +%s)
  "step_$s"
  echo "== $s took $(( $(date +%s) - start )) s"
done
