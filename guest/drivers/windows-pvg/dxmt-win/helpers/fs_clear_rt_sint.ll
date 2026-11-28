; SPDX-License-Identifier: MIT
;
; fs_clear_rt_sint: one of DXMT's helper functions (dxmt_command.metal), written by hand
; as AIR, the LLVM form airconv writes, so that our own tools make its
; library (air2metallib.cpp) and no other compiler is needed. One function
; to a library: the device's function operation names no function.
source_filename = "ruxn_helpers"
target datalayout = "e-p:64:64:64-i1:8:8-i8:8:8-i16:16:16-i32:32:32-i64:64:64-f32:32:32-f64:64:64-v16:16:16-v24:32:32-v32:32:32-v48:64:64-v64:64:64-v96:128:128-v128:128:128-v192:256:256-v256:256:256-v512:512:512-v1024:1024:1024-n8:16:32"
target triple = "air64-apple-macosx15.0.0"

; The clear value from buffer 30 (kCustomBufferArgumentIndex0), as the
; target's type wants it.
define <{ <4 x i32> }> @fs_clear_rt_sint(<4 x float> addrspace(2)* noalias nocapture readonly %value) local_unnamed_addr {
entry:
  %v = load <4 x float>, <4 x float> addrspace(2)* %value, align 16
  %out = fptosi <4 x float> %v to <4 x i32>
  %r = insertvalue <{ <4 x i32> }> undef, <4 x i32> %out, 0
  ret <{ <4 x i32> }> %r
}

!llvm.module.flags = !{!0, !1, !2, !3, !4, !5, !6, !7, !8}
!air.version = !{!9}
!air.language_version = !{!10}
!0 = !{i32 2, !"SDK Version", [2 x i32] [i32 15, i32 0]}
!1 = !{i32 1, !"wchar_size", i32 4}
!2 = !{i32 7, !"frame-pointer", i32 2}
!3 = !{i32 7, !"air.max_device_buffers", i32 31}
!4 = !{i32 7, !"air.max_constant_buffers", i32 31}
!5 = !{i32 7, !"air.max_threadgroup_buffers", i32 31}
!6 = !{i32 7, !"air.max_textures", i32 128}
!7 = !{i32 7, !"air.max_read_write_textures", i32 8}
!8 = !{i32 7, !"air.max_samplers", i32 16}
!9 = !{i32 2, i32 7, i32 0}
!10 = !{!"Metal", i32 3, i32 2, i32 0}
!air.fragment = !{!11}
!11 = !{<{ <4 x i32> }> (<4 x float> addrspace(2)*)* @fs_clear_rt_sint, !12, !14}
!12 = !{!13}
!13 = !{!"air.render_target", i32 0, i32 0, !"air.arg_type_name", !"int4", !"air.arg_name", !"colour"}
!14 = !{!15}
!15 = !{i32 0, !"air.buffer", !"air.buffer_size", i32 16, !"air.location_index", i32 30, i32 1, !"air.read", !"air.address_space", i32 2, !"air.arg_type_size", i32 16, !"air.arg_type_align_size", i32 16, !"air.arg_type_name", !"float4", !"air.arg_name", !"clear_value"}
