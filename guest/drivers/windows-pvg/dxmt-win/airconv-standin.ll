; SPDX-License-Identifier: MIT
;
; STAND-IN for DXMT's helper shaders src/airconv/shaders/air_msad.metal,
; air_samplepos.metal and air_tessellation.metal. Upstream compiles those with
; Xcode's Metal compiler into AIR bitcode that airconv links into every shader
; that needs them (airconv_context.cpp, linkShader with LinkOnlyNeeded).
; We use no Xcode tools, so this empty module is assembled with LLVM 15's
; llvm-as and embedded three times instead (air_msad.h, air_samplepos.h,
; air_tessellation.h). Ordinary shaders are unaffected. A shader that uses
; msad, sample positions or tessellation keeps an unresolved call to the
; missing helper, so Metal refuses to load that library: it fails, it is not
; silently wrong.
;
; Triple and datalayout are the ones airconv sets on its own modules
; (airconv_context.cpp, initializeModule), so linking raises no mismatch.

source_filename = "airconv_standin"
target datalayout = "e-p:64:64:64-i1:8:8-i8:8:8-i16:16:16-i32:32:32-i64:64:64-f32:32:32-f64:64:64-v16:16:16-v24:32:32-v32:32:32-v48:64:64-v64:64:64-v96:128:128-v128:128:128-v192:256:256-v256:256:256-v512:512:512-v1024:1024:1024-n8:16:32"
target triple = "air64-apple-macosx14.0.0"
