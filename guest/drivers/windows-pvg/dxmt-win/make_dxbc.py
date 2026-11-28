#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
#
# Writes the two Direct3D shaders of the triangle as DXBC, by hand: no
# shader compiler is used. They do what triangle.vert and triangle.frag do
# (the pvgtri shaders), in Direct3D's clip space, which is Metal's: the
# corners are those of triangle.vert with y turned over, as airc --flip-y
# turns it over for Metal.
#
#   vs_4_0:  corner and colour from the vertex's index, no vertex buffer
#   ps_4_0:  the colour, alpha 1
#
# Every token is written below with what it means. The container's checksum
# is left zero: DXMT does not check it (Microsoft's runtime would). The
# bytes are checked on the Mac by dxbccheck, built from DXMT's own DXBC
# parser.
#
# Usage: make_dxbc.py OUT_DIR
# Writes OUT_DIR/triangle_dxbc.h (C arrays for d3d11tri and winemetal) and
# OUT_DIR/triangle-vs.dxbc, OUT_DIR/triangle-ps.dxbc.

import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from dxbc_checksum import dxbc_checksum  # noqa: E402

# Opcodes (D3D10_SB_OPCODE_TYPE), as d3d12tokenizedprogramformat.hpp numbers them.
IADD = 30
CUSTOMDATA = 53
MOV = 54
RET = 62
DCL_INPUT_SGV = 96
DCL_INPUT_PS = 98
DCL_OUTPUT = 101
DCL_OUTPUT_SIV = 103
DCL_TEMPS = 104

MAD = 50
SAMPLE = 69
DCL_RESOURCE = 88
DCL_CONSTANT_BUFFER = 89
DCL_SAMPLER = 90
DCL_INPUT = 95

RESOURCE_TEXTURE2D = 3 << 11   # dcl_resource's dimension, opcode bits 15:11
RETURN_FLOAT4 = 0x5555         # dcl_resource's return type: float in each of four
ICB_CLASS = 3             # D3D10_SB_CUSTOMDATA_DCL_IMMEDIATE_CONSTANT_BUFFER, opcode bits 31:11
INTERPOLATION_LINEAR = 2  # opcode bits 14:11 of dcl_input_ps

# System values (D3D10_SB_NAME, and D3D_NAME in signatures).
NAME_UNDEFINED = 0
NAME_POSITION = 1
NAME_VERTEX_ID = 6
NAME_TARGET = 64          # signatures only

# Operand types (D3D10_SB_OPERAND_TYPE).
TEMP = 0
INPUT = 1
OUTPUT = 2
IMMEDIATE32 = 4
SAMPLER = 6
RESOURCE = 7
CONSTANT_BUFFER = 8
ICB = 9


def opcode(op, length, extra=0):
    """Opcode token: bits 10:0 the opcode, 30:24 the instruction's length in tokens."""
    return op | extra | (length << 24)


def mask_operand(kind, mask):
    """A 4-component operand with a write mask (dest and declarations),
    one index of 32 bits: bits 1:0 = 2 (four components), 3:2 = 0 (mask mode),
    7:4 the mask (x=1 y=2 z=4 w=8), 19:12 the type, 21:20 = 1 (one index)."""
    return 2 | (mask << 4) | (kind << 12) | (1 << 20)


def swizzle_operand(kind, swizzle, relative=False):
    """A 4-component source with a swizzle: 3:2 = 1, 11:4 four 2-bit selects.
    With relative, index 0 is a register (bits 24:22 = 2) instead of a number."""
    sel = 0
    for i, c in enumerate(swizzle):
        sel |= 'xyzw'.index(c) << (2 * i)
    rep = (2 << 22) if relative else 0
    return 2 | (1 << 2) | (sel << 4) | (kind << 12) | (1 << 20) | rep


def select_operand(kind, comp):
    """A source of one component: 3:2 = 2 (select-1), 5:4 the component."""
    return 2 | (2 << 2) | ('xyzw'.index(comp) << 4) | (kind << 12) | (1 << 20)


IMM32 = 1 | (IMMEDIATE32 << 12)   # one component, no index; the value follows
IMM32X4 = 2 | (IMMEDIATE32 << 12)  # four components, no index; four values follow


def cb_operand(swizzle):
    """cb0[n] as a source: four components with a swizzle and two indices,
    the buffer's slot and the constant's (bits 21:20 = 2)."""
    return swizzle_operand(CONSTANT_BUFFER, swizzle) & ~(3 << 20) | (2 << 20)


def object_operand(kind, swizzle=None):
    """t# or s#: a resource with a swizzle, or a sampler with no components."""
    if swizzle:
        return swizzle_operand(kind, swizzle)
    return (kind << 12) | (1 << 20)


# Compute (shader model 5): the opcodes and operand types it adds.
ISHL = 41
DCL_THREAD_GROUP = 155
DCL_UAV_RAW = 157
STORE_RAW = 166
UAV = 30
THREAD_ID = 32               # vThreadID, no index
GROUP_THREADS = 64


def compute_program():
    """Each thread stores its own number at byte offset number * 4 of the
    raw buffer u0: after Dispatch(n, 1, 1), word i of u0 is i."""
    thread_x_decl = 2 | (1 << 4) | (THREAD_ID << 12)        # vThreadID.x, a mask
    thread_x = 2 | (2 << 2) | (0 << 4) | (THREAD_ID << 12)  # vThreadID.x, selected
    body = [
        # dcl_uav_raw u0
        opcode(DCL_UAV_RAW, 3), (UAV << 12) | (1 << 20), 0,
        # dcl_input vThreadID.x
        opcode(DCL_INPUT, 2), thread_x_decl,
        # dcl_temps 1
        opcode(DCL_TEMPS, 2), 1,
        # dcl_thread_group 64, 1, 1
        opcode(DCL_THREAD_GROUP, 4), GROUP_THREADS, 1, 1,
        # ishl r0.x, vThreadID.x, l(2)
        opcode(ISHL, 6), mask_operand(TEMP, 0x1), 0, thread_x, IMM32, 2,
        # store_raw u0.x, r0.x, vThreadID.x
        opcode(STORE_RAW, 6), mask_operand(UAV, 0x1), 0, select_operand(TEMP, 'x'), 0, thread_x,
        opcode(RET, 1),
    ]
    return [0x00050050, 2 + len(body)] + body


def f32(x):
    return struct.unpack('<I', struct.pack('<f', x))[0]


# The corners of triangle.vert, y turned over (Direct3D's and Metal's clip
# space), and its colours.
CORNERS = [(0.0, 0.75), (0.75, -0.75), (-0.75, -0.75)]
COLOURS = [(1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0)]


def vertex_program():
    icb = []
    for x, y in CORNERS:              # icb[0..2]: corner, z 0, w 1
        icb += [f32(x), f32(y), f32(0.0), f32(1.0)]
    for r, g, b in COLOURS:           # icb[3..5]: colour, alpha 1
        icb += [f32(r), f32(g), f32(b), f32(1.0)]
    body = [
        # dcl_immediateConstantBuffer { 6 float4 }: class, then the length in
        # tokens of the whole declaration, then the data.
        CUSTOMDATA | (ICB_CLASS << 11), 2 + len(icb), *icb,
        # dcl_input_sgv v0.x, vertex_id
        opcode(DCL_INPUT_SGV, 4), mask_operand(INPUT, 0x1), 0, NAME_VERTEX_ID,
        # dcl_output_siv o0.xyzw, position
        opcode(DCL_OUTPUT_SIV, 4), mask_operand(OUTPUT, 0xF), 0, NAME_POSITION,
        # dcl_output o1.xyz
        opcode(DCL_OUTPUT, 3), mask_operand(OUTPUT, 0x7), 1,
        # dcl_temps 1
        opcode(DCL_TEMPS, 2), 1,
        # mov r0.x, v0.x
        opcode(MOV, 5), mask_operand(TEMP, 0x1), 0, select_operand(INPUT, 'x'), 0,
        # mov o0.xyzw, icb[r0.x].xyzw
        opcode(MOV, 6), mask_operand(OUTPUT, 0xF), 0,
        swizzle_operand(ICB, 'xyzw', relative=True), select_operand(TEMP, 'x'), 0,
        # iadd r0.x, r0.x, l(3)
        opcode(IADD, 7), mask_operand(TEMP, 0x1), 0, select_operand(TEMP, 'x'), 0, IMM32, 3,
        # mov o1.xyz, icb[r0.x].xyzw
        opcode(MOV, 6), mask_operand(OUTPUT, 0x7), 1,
        swizzle_operand(ICB, 'xyzw', relative=True), select_operand(TEMP, 'x'), 0,
        # ret
        opcode(RET, 1),
    ]
    # Version token: bits 3:0 minor, 7:4 major, 31:16 the kind (1 vertex).
    return [0x00010040, 2 + len(body)] + body


def pixel_program():
    body = [
        # dcl_input_ps linear v1.xyz
        opcode(DCL_INPUT_PS, 3, INTERPOLATION_LINEAR << 11), mask_operand(INPUT, 0x7), 1,
        # dcl_output o0.xyzw
        opcode(DCL_OUTPUT, 3), mask_operand(OUTPUT, 0xF), 0,
        # mov o0.xyz, v1.xyzx
        opcode(MOV, 5), mask_operand(OUTPUT, 0x7), 0, swizzle_operand(INPUT, 'xyzx'), 1,
        # mov o0.w, l(1.0)
        opcode(MOV, 5), mask_operand(OUTPUT, 0x8), 0, IMM32, f32(1.0),
        # ret
        opcode(RET, 1),
    ]
    # Version token: kind 0, pixel.
    return [0x00000040, 2 + len(body)] + body


def quad_vertex_program():
    """The quad's vertex shader, shader model 4.0:
         o0 = float4(v0.xy, 0, 1); o1.xy = v1.xy * cb0[0].xy + cb0[0].zw"""
    body = [
        # dcl_constantbuffer cb0[1], immediateIndexed
        opcode(DCL_CONSTANT_BUFFER, 4), cb_operand('xyzw'), 0, 1,
        # dcl_input v0.xy; dcl_input v1.xy
        opcode(DCL_INPUT, 3), mask_operand(INPUT, 0x3), 0,
        opcode(DCL_INPUT, 3), mask_operand(INPUT, 0x3), 1,
        # dcl_output_siv o0.xyzw, position; dcl_output o1.xy
        opcode(DCL_OUTPUT_SIV, 4), mask_operand(OUTPUT, 0xF), 0, NAME_POSITION,
        opcode(DCL_OUTPUT, 3), mask_operand(OUTPUT, 0x3), 1,
        # mov o0.xy, v0.xyxx
        opcode(MOV, 5), mask_operand(OUTPUT, 0x3), 0, swizzle_operand(INPUT, 'xyxx'), 0,
        # mov o0.zw, l(0, 0, 0, 1)
        opcode(MOV, 8), mask_operand(OUTPUT, 0xC), 0, IMM32X4, f32(0.0), f32(0.0), f32(0.0), f32(1.0),
        # mad o1.xy, v1.xyxx, cb0[0].xyxx, cb0[0].zwzz
        opcode(MAD, 11), mask_operand(OUTPUT, 0x3), 1, swizzle_operand(INPUT, 'xyxx'), 1,
        cb_operand('xyxx'), 0, 0, cb_operand('zwzz'), 0, 0,
        opcode(RET, 1),
    ]
    return [0x00010040, 2 + len(body)] + body


def quad_pixel_program():
    """The quad's pixel shader: o0 = t0.Sample(s0, v1.xy)."""
    body = [
        # dcl_sampler s0, mode_default
        opcode(DCL_SAMPLER, 3), object_operand(SAMPLER), 0,
        # dcl_resource_texture2d (float,float,float,float) t0
        opcode(DCL_RESOURCE, 4, RESOURCE_TEXTURE2D), object_operand(RESOURCE), 0, RETURN_FLOAT4,
        # dcl_input_ps linear v1.xy
        opcode(DCL_INPUT_PS, 3, INTERPOLATION_LINEAR << 11), mask_operand(INPUT, 0x3), 1,
        # dcl_output o0.xyzw
        opcode(DCL_OUTPUT, 3), mask_operand(OUTPUT, 0xF), 0,
        # sample o0.xyzw, v1.xyxx, t0.xyzw, s0
        opcode(SAMPLE, 9), mask_operand(OUTPUT, 0xF), 0, swizzle_operand(INPUT, 'xyxx'), 1,
        object_operand(RESOURCE, 'xyzw'), 0, object_operand(SAMPLER), 0,
        opcode(RET, 1),
    ]
    return [0x00000040, 2 + len(body)] + body


def signature(elements):
    """ISGN or OSGN: count, 8 (where the elements start), then 24 bytes an
    element (name offset from the chunk's start, semantic index, system value,
    component type 1 uint 3 float, register, mask, read/write mask, two zero
    bytes), then the names."""
    head = 8 + 24 * len(elements)
    names = b''
    offsets = {}
    for e in elements:
        if e[0] not in offsets:
            offsets[e[0]] = head + len(names)
            names += e[0].encode() + b'\0'
    out = struct.pack('<II', len(elements), 8)
    for name, index, sysval, ctype, reg, mask, rw in elements:
        out += struct.pack('<IIIIIBBH', offsets[name], index, sysval, ctype, reg, mask, rw, 0)
    out += names
    while len(out) % 4:
        out += b'\xab'   # padding, as the compiler writes it
    return out


def container(chunks):
    """DXBC: 'DXBC', 16 bytes of checksum, version 1, the total size, the
    chunk count and each chunk's offset; a chunk is its four letters, its
    size and its data. Windows' Direct3D runtime refuses a container whose
    checksum is wrong (DXMT does not look at it)."""
    head = 32 + 4 * len(chunks)
    body = b''
    offsets = []
    for fourcc, data in chunks:
        offsets.append(head + len(body))
        body += fourcc + struct.pack('<I', len(data)) + data
    total = head + len(body)
    out = b'DXBC' + b'\0' * 16 + struct.pack('<III', 1, total, len(chunks))
    out += b''.join(struct.pack('<I', o) for o in offsets)
    out += body
    return out[:4] + dxbc_checksum(out) + out[20:]


def tokens(words):
    return b''.join(struct.pack('<I', w) for w in words)


UINT, FLOAT = 1, 3

VERTEX = container([
    (b'ISGN', signature([('SV_VertexID', 0, NAME_VERTEX_ID, UINT, 0, 0x1, 0x1)])),
    (b'OSGN', signature([('SV_Position', 0, NAME_POSITION, FLOAT, 0, 0xF, 0x0),
                         ('COLOR', 0, NAME_UNDEFINED, FLOAT, 1, 0x7, 0x8)])),
    (b'SHDR', tokens(vertex_program())),
])

PIXEL = container([
    (b'ISGN', signature([('SV_Position', 0, NAME_POSITION, FLOAT, 0, 0xF, 0x0),
                         ('COLOR', 0, NAME_UNDEFINED, FLOAT, 1, 0x7, 0x7)])),
    (b'OSGN', signature([('SV_Target', 0, NAME_TARGET, FLOAT, 0, 0xF, 0x0)])),
    (b'SHDR', tokens(pixel_program())),
])


QUAD_VERTEX = container([
    (b'ISGN', signature([('POSITION', 0, NAME_UNDEFINED, FLOAT, 0, 0x3, 0x3),
                         ('TEXCOORD', 0, NAME_UNDEFINED, FLOAT, 1, 0x3, 0x3)])),
    (b'OSGN', signature([('SV_Position', 0, NAME_POSITION, FLOAT, 0, 0xF, 0x0),
                         ('TEXCOORD', 0, NAME_UNDEFINED, FLOAT, 1, 0x3, 0xC)])),
    (b'SHDR', tokens(quad_vertex_program())),
])

QUAD_PIXEL = container([
    (b'ISGN', signature([('SV_Position', 0, NAME_POSITION, FLOAT, 0, 0xF, 0x0),
                         ('TEXCOORD', 0, NAME_UNDEFINED, FLOAT, 1, 0x3, 0x3)])),
    (b'OSGN', signature([('SV_Target', 0, NAME_TARGET, FLOAT, 0, 0xF, 0x0)])),
    (b'SHDR', tokens(quad_pixel_program())),
])


COMPUTE = container([
    (b'ISGN', signature([])),
    (b'OSGN', signature([])),
    (b'SHEX', tokens(compute_program())),
])


def c_array(name, data):
    lines = ['static const unsigned char %s[%d] = {' % (name, len(data))]
    for i in range(0, len(data), 12):
        lines.append('\t' + ' '.join('0x%02x,' % b for b in data[i:i + 12]))
    lines.append('};')
    return '\n'.join(lines)


def main():
    out = sys.argv[1]
    with open(out + '/triangle-vs.dxbc', 'wb') as f:
        f.write(VERTEX)
    with open(out + '/triangle-ps.dxbc', 'wb') as f:
        f.write(PIXEL)
    with open(out + '/triangle_dxbc.h', 'w') as f:
        f.write('/* Made by make_dxbc.py: the triangle\'s shaders as DXBC. */\n')
        f.write(c_array('triangle_vs_dxbc', VERTEX) + '\n')
        f.write(c_array('triangle_ps_dxbc', PIXEL) + '\n')
        f.write(c_array('quad_vs_dxbc', QUAD_VERTEX) + '\n')
        f.write(c_array('quad_ps_dxbc', QUAD_PIXEL) + '\n')
        f.write(c_array('compute_cs_dxbc', COMPUTE) + '\n')
    with open(out + '/quad-vs.dxbc', 'wb') as f:
        f.write(QUAD_VERTEX)
    with open(out + '/quad-ps.dxbc', 'wb') as f:
        f.write(QUAD_PIXEL)
    with open(out + '/compute-cs.dxbc', 'wb') as f:
        f.write(COMPUTE)


if __name__ == '__main__':
    main()
