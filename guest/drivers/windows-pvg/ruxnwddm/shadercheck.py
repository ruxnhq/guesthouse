#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
#
# On the Mac: turns the shaders ruxnumd.dll captured in a run (umd_capture.c,
# C:\ruxnlog\sh-*.bin) into DXBC containers by the same rules as
# umd_dxbc.c, and runs each through airconvtest (dxmt-win/airconvtest.c,
# airconv built natively by airconv-build.sh), so that a run's shader set
# can be tried against the shader compiler without a guest.
#
# Usage: shadercheck.py CAPTURE_DIR OUT_DIR AIRCONVTEST

import os
import struct
import subprocess
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'dxmt-win'))
from dxbc_checksum import dxbc_checksum  # noqa: E402

STAGES = {0: 'vertex', 1: 'geometry', 2: 'pixel'}
ENTRY = struct.Struct('<IIBBxxII')   # D3D11_1DDIARG_SIGNATURE_ENTRY2, 20 bytes
SYSTEM_NAMES = [None, 'SV_Position', 'SV_ClipDistance', 'SV_CullDistance',
                'SV_RenderTargetArrayIndex', 'SV_ViewportArrayIndex', 'SV_VertexID',
                'SV_PrimitiveID', 'SV_InstanceID', 'SV_IsFrontFace', 'SV_SampleIndex']
TARGET = 64


def element_name(sysval, reg, pixel_output):
    if pixel_output and sysval == 0:
        return 'SV_Target', reg, TARGET
    if 0 < sysval < len(SYSTEM_NAMES):
        return SYSTEM_NAMES[sysval], 0, sysval
    return 'TEXCOORD', reg, 0


# The runtime leaves component types unknown; as umd_dxbc.c's component_type.
UINT_SYSTEM_VALUES = (4, 5, 6, 7, 8, 9, 10)


def component_type(sysval, ctype):
    if ctype:
        return ctype
    return 1 if sysval in UINT_SYSTEM_VALUES else 3


def signature(entries, output, pixel_output):
    names_at = 8 + 24 * len(entries)
    out = struct.pack('<II', len(entries), 8)
    names = b''
    for sysval, reg, mask, _stream, ctype, _prec in entries:
        name, index, sv = element_name(sysval, reg, pixel_output)
        out += struct.pack('<IIIIIBBH', names_at + len(names), index, sv,
                           component_type(sysval, ctype), reg, mask,
                           0 if output else mask, 0)
        names += name.encode() + b'\0'
    out += names
    while len(out) % 4:
        out += b'\xab'
    return out


def container(chunks):
    head = 32 + 4 * len(chunks)
    body = b''
    offsets = []
    for fourcc, data in chunks:
        offsets.append(head + len(body))
        body += fourcc + struct.pack('<I', len(data)) + data
    out = b'DXBC' + b'\0' * 16 + struct.pack('<III', 1, head + len(body), len(chunks))
    out += b''.join(struct.pack('<I', o) for o in offsets) + body
    return out[:4] + dxbc_checksum(out) + out[20:]


def read_capture(path):
    data = open(path, 'rb').read()
    magic, stage, count, n_in, n_out = struct.unpack_from('<5I', data)
    assert magic == 0x48535852, path
    at = 20
    tokens = data[at:at + 4 * count]
    at += 4 * count
    ins = [ENTRY.unpack_from(data, at + 20 * i) for i in range(n_in)]
    at += 20 * n_in
    outs = [ENTRY.unpack_from(data, at + 20 * i) for i in range(n_out)]
    return stage, tokens, ins, outs


def main():
    capture_dir, out_dir, airconvtest = sys.argv[1:4]
    os.makedirs(out_dir, exist_ok=True)
    good = bad = 0
    for name in sorted(os.listdir(capture_dir)):
        if not name.startswith('sh-'):
            continue
        stage, tokens, ins, outs = read_capture(os.path.join(capture_dir, name))
        major = (struct.unpack_from('<I', tokens)[0] >> 4) & 0xf
        pixel = stage == 2
        dxbc = container([(b'ISGN', signature(ins, False, False)),
                          (b'OSGN', signature(outs, True, pixel)),
                          (b'SHEX' if major >= 5 else b'SHDR', tokens)])
        base = os.path.join(out_dir, name[:-4])
        open(base + '.dxbc', 'wb').write(dxbc)
        r = subprocess.run([airconvtest, base + '.dxbc', base + '.metallib'], capture_output=True,
                           text=True)
        ok = r.returncode == 0 and os.path.exists(base + '.metallib')
        good += ok
        bad += not ok
        print('%s %-8s sm%d in %d out %d: %s' % (name, STAGES.get(stage, stage), major, len(ins),
                                                 len(outs), 'ok' if ok else 'FAILED ' +
                                                 (r.stdout + r.stderr).strip()[-200:]))
    print('shadercheck: %d compiled, %d failed' % (good, bad))


if __name__ == '__main__':
    main()
