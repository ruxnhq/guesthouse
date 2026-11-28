# SPDX-License-Identifier: MIT
# The DXBC container's checksum: MD5's compression function over the
# container after its first 20 bytes, with the length placed differently
# from MD5's own padding (the first word of the last block holds the length
# in bits, the last word (length >> 2) | 1).
import struct, math

_S = [7,12,17,22]*4 + [5,9,14,20]*4 + [4,11,16,23]*4 + [6,10,15,21]*4
_K = [int(abs(math.sin(i + 1)) * 2**32) & 0xffffffff for i in range(64)]

def _rotl(x, c):
    return ((x << c) | (x >> (32 - c))) & 0xffffffff

def _compress(state, block):
    a, b, c, d = state
    m = struct.unpack('<16I', block)
    for i in range(64):
        if i < 16:
            f, g = (b & c) | (~b & d), i
        elif i < 32:
            f, g = (d & b) | (~d & c), (5 * i + 1) % 16
        elif i < 48:
            f, g = b ^ c ^ d, (3 * i + 5) % 16
        else:
            f, g = c ^ (b | (~d & 0xffffffff)), (7 * i) % 16
        f = (f + a + _K[i] + m[g]) & 0xffffffff
        a, d, c = d, c, b
        b = (b + _rotl(f, _S[i])) & 0xffffffff
    return [(x + y) & 0xffffffff for x, y in zip(state, (a, b, c, d))]

def dxbc_checksum(container):
    data = container[20:]
    state = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476]
    full = len(data) & ~63
    for off in range(0, full, 64):
        state = _compress(state, data[off:off + 64])
    left = data[full:]
    bits = len(data) * 8
    if len(left) >= 56:
        block = left + b'\x80' + b'\0' * (63 - len(left))
        state = _compress(state, block)
        last = struct.pack('<I', bits) + b'\0' * 56 + struct.pack('<I', (bits >> 2) | 1)
    else:
        last = struct.pack('<I', bits) + left + b'\x80'
        last += b'\0' * (60 - len(last)) + struct.pack('<I', (bits >> 2) | 1)
    state = _compress(state, last)
    return struct.pack('<4I', *state)
