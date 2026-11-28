; SPDX-License-Identifier: MIT
;
; vs_clear_rt: one of DXMT's helper functions (dxmt_command.metal), written by hand
; as AIR, the LLVM form airconv writes, so that our own tools make its
; library (air2metallib.cpp) and no other compiler is needed. One function
; to a library: the device's function operation names no function.
source_filename = "ruxn_helpers"
target datalayout = "e-p:64:64:64-i1:8:8-i8:8:8-i16:16:16-i32:32:32-i64:64:64-f32:32:32-f64:64:64-v16:16:16-v24:32:32-v32:32:32-v48:64:64-v64:64:64-v96:128:128-v128:128:128-v192:256:256-v256:256:256-v512:512:512-v1024:1024:1024-n8:16:32"
target triple = "air64-apple-macosx15.0.0"

; A triangle over the whole target (uv 0,0 2,0 0,2): float2 uv =
; float2((id << 1) & 2, id & 2), position uv * (2, -2) + (-1, 1). DXMT's
; also writes the instance as the slice (render_target_array_index); this
; one does not: the wire's pipeline has no input topology, and Metal will
; not make a pipeline whose vertex function writes that output without one
; (helper scene helper-clear); passes of more than one slice are refused
; before they reach here anyway (translate.c).
define <{ <4 x float> }> @vs_clear_rt(i32 %id, i32 %instance) local_unnamed_addr {
entry:
  %shifted = shl i32 %id, 1
  %ux = and i32 %shifted, 2
  %uy = and i32 %id, 2
  %fx = uitofp i32 %ux to float
  %fy = uitofp i32 %uy to float
  %x = fmul float %fx, 2.000000e+00
  %px = fadd float %x, -1.000000e+00
  %y = fmul float %fy, -2.000000e+00
  %py = fadd float %y, 1.000000e+00
  %p0 = insertelement <4 x float> <float 0.0, float 0.0, float 0.0, float 1.0>, float %px, i64 0
  %p1 = insertelement <4 x float> %p0, float %py, i64 1
  %r0 = insertvalue <{ <4 x float> }> undef, <4 x float> %p1, 0
  ret <{ <4 x float> }> %r0
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
!air.vertex = !{!11}
!11 = !{<{ <4 x float> }> (i32, i32)* @vs_clear_rt, !12, !15}
!12 = !{!13}
!13 = !{!"air.position", !"air.arg_type_name", !"float4", !"air.arg_name", !"position"}
!15 = !{!16, !17}
!16 = !{i32 0, !"air.vertex_id", !"air.arg_type_name", !"uint", !"air.arg_name", !"id"}
!17 = !{i32 1, !"air.instance_id", !"air.arg_type_name", !"uint", !"air.arg_name", !"instance_id"}
