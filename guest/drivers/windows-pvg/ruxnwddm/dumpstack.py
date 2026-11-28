#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
"""Reads a Windows kernel minidump (a small "triage" dump) on the Mac,
with no debugger: prints the bugcheck and, for every value on the crashing
thread's stack that falls inside a loaded driver, the driver and the
offset. With a link map of the same build (build.sh writes ruxnwddm.map)
our own offsets are named.

Usage: dumpstack.py DUMP.dmp [ruxnwddm.map]

Only the fields this needs are read: the bugcheck in the header, then the
triage block's offsets to the driver list, the string pool and the stack.
"""
import re
import struct
import sys

HEADER_BUGCHECK = 0x38          # code, then four 64-bit arguments
TRIAGE_AT = 0x2000              # the triage block follows the header
# Fields of the triage block, as 32-bit words, by index.
CALL_STACK, CALL_STACK_SIZE = 10, 11
DRIVER_LIST, DRIVER_COUNT, STRING_POOL = 12, 13, 14
# In a driver entry: the name's offset, then a loader entry whose image
# base and size are at these places.
LDR_AT, LDR_BASE, LDR_SIZE = 8, 0x30, 0x40
MAP_BASE = 0x140000000          # the preferred base lld-link puts in the map


def modules(dump):
    t = struct.unpack_from('<24I', dump, TRIAGE_AT)
    first, count, pool = t[DRIVER_LIST], t[DRIVER_COUNT], t[STRING_POOL]
    size = (pool - first) // count
    for i in range(count):
        entry = first + i * size
        name_at, = struct.unpack_from('<I', dump, entry)
        base, = struct.unpack_from('<Q', dump, entry + LDR_AT + LDR_BASE)
        length, = struct.unpack_from('<I', dump, entry + LDR_AT + LDR_SIZE)
        chars, = struct.unpack_from('<I', dump, name_at)
        name = dump[name_at + 4:name_at + 4 + chars * 2].decode('utf-16-le', 'replace')
        yield base, length, name


def symbols(map_path):
    found = []
    for line in open(map_path):
        m = re.match(r'\s*000[0-9]:[0-9a-f]{8}\s+(\S+)\s+([0-9a-f]{16})', line)
        if m:
            found.append((int(m.group(2), 16) - MAP_BASE, m.group(1)))
    return sorted(found)


def name_of(syms, rva):
    best = None
    for start, name in syms:
        if start <= rva:
            best = (start, name)
    return f'{best[1]}+{rva - best[0]:#x}' if best else ''


def main():
    dump = open(sys.argv[1], 'rb').read()
    syms = symbols(sys.argv[2]) if len(sys.argv) > 2 else []
    code, = struct.unpack_from('<I', dump, HEADER_BUGCHECK)
    args = struct.unpack_from('<4Q', dump, HEADER_BUGCHECK + 8)
    print(f'bugcheck {code:#x} ' + ' '.join(f'{a:#x}' for a in args))
    mods = list(modules(dump))
    t = struct.unpack_from('<24I', dump, TRIAGE_AT)
    stack, size = t[CALL_STACK], t[CALL_STACK_SIZE]
    for at in range(0, size, 8):
        value, = struct.unpack_from('<Q', dump, stack + at)
        for base, length, name in mods:
            if base <= value < base + length:
                rva = value - base
                where = name_of(syms, rva) if syms and name.startswith('ruxnwddm') else ''
                print(f'{at:#06x} {name} {rva:#x} {where}')
                break


if __name__ == '__main__':
    main()
