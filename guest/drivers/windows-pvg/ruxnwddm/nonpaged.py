#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
"""Marks a driver image's sections not paged, as Microsoft's linker does.

Usage: nonpaged.py DRIVER.sys

link.exe sets IMAGE_SCN_MEM_NOT_PAGED on every section of a driver except
INIT, the PAGE ones and the discardable ones. lld-link does not, and it has
no /section switch for it, so this sets the flag after linking, then puts
the PE checksum right. It runs before signing, since signing covers the
section headers.
"""
import struct
import sys

SCN_MEM_DISCARDABLE = 0x02000000
SCN_MEM_NOT_PAGED = 0x08000000

PE_OFFSET_AT = 0x3C
FILE_HEADER_SIZE = 20
SECTION_HEADER_SIZE = 40
CHECKSUM_IN_OPTIONAL = 64


def stays_pageable(name, flags):
    return name == "INIT" or name.startswith("PAGE") or flags & SCN_MEM_DISCARDABLE


def checksum(image, checksum_at):
    """The PE checksum: a folded 16-bit sum of the file, plus its length."""
    data = image[:checksum_at] + b"\0\0\0\0" + image[checksum_at + 4:]
    if len(data) % 2:
        data += b"\0"
    total = 0
    for (word,) in struct.iter_unpack("<H", data):
        total += word
        total = (total & 0xFFFF) + (total >> 16)
    return ((total & 0xFFFF) + (total >> 16) + len(image)) & 0xFFFFFFFF


def main():
    path = sys.argv[1]
    image = bytearray(open(path, "rb").read())

    (pe,) = struct.unpack_from("<I", image, PE_OFFSET_AT)
    if image[pe:pe + 4] != b"PE\0\0":
        raise SystemExit(f"{path}: not a PE image")
    file_header = pe + 4
    sections, = struct.unpack_from("<H", image, file_header + 2)
    optional_size, = struct.unpack_from("<H", image, file_header + 16)
    optional = file_header + FILE_HEADER_SIZE
    table = optional + optional_size

    for i in range(sections):
        header = table + i * SECTION_HEADER_SIZE
        name = image[header:header + 8].rstrip(b"\0").decode()
        flags_at = header + 36
        flags, = struct.unpack_from("<I", image, flags_at)
        if not stays_pageable(name, flags):
            flags |= SCN_MEM_NOT_PAGED
            struct.pack_into("<I", image, flags_at, flags)
        state = "pageable" if stays_pageable(name, flags) else "not paged"
        print(f"{name:8} {flags:#010x} {state}")

    checksum_at = optional + CHECKSUM_IN_OPTIONAL
    struct.pack_into("<I", image, checksum_at, checksum(image, checksum_at))
    open(path, "wb").write(image)


if __name__ == "__main__":
    main()
