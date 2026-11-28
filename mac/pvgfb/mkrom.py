#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Wraps a UEFI driver in a PCI expansion ROM for the PV graphics device.

Usage: mkrom.py IN.efi OUT.rom

The ROM holds one image: a PCI expansion ROM header with the EFI fields, a
PCI data structure (PCIR, revision 3) naming the device, and the driver
uncompressed, padded to a whole number of 512-byte blocks (in fact of 16 KiB,
see ROM_GRANULE). The layout is the
one in the PCI Firmware Specification 3.x and the UEFI specification's
chapter on PCI option ROMs.
"""
import struct
import sys

VENDOR_ID = 0x106B
DEVICE_ID = 0xEEEE
CLASS_CODE = 0x038000  # display controller, other

ROM_SIGNATURE = 0xAA55
EFI_SIGNATURE = 0x0EF1
MACHINE_AARCH64 = 0xAA64
NOT_COMPRESSED = 0
CODE_TYPE_EFI = 3
LAST_IMAGE = 0x80
BLOCK = 512

PCIR_REVISION = 3
PCIR_LENGTH = 0x1C
ROM_HEADER_LENGTH = 0x1A

# QEMU under Hypervisor.framework on Apple silicon cannot map a ROM BAR that
# is not a whole number of the host's 16 KiB pages, and stops with
# HV_BAD_ARGUMENT. So the ROM is padded to that size, and the image's
# length covers the padding.
ROM_GRANULE = 16 * 1024

PCIR_OFFSET = 0x1C  # the header, rounded up to four bytes
# The driver starts on a block boundary of its own.
IMAGE_OFFSET = BLOCK


def pe_fields(image):
    """The machine and the subsystem of a PE32+ image."""
    if image[:2] != b"MZ":
        raise SystemExit("not a PE image: no MZ header")
    (pe_offset,) = struct.unpack_from("<I", image, 0x3C)
    if image[pe_offset:pe_offset + 4] != b"PE\0\0":
        raise SystemExit("not a PE image: no PE signature")
    (machine,) = struct.unpack_from("<H", image, pe_offset + 4)
    optional = pe_offset + 24
    # The subsystem is at the same place in the PE32 and PE32+ headers.
    (subsystem,) = struct.unpack_from("<H", image, optional + 68)
    return machine, subsystem


def build(image):
    machine, subsystem = pe_fields(image)
    if machine != MACHINE_AARCH64:
        raise SystemExit(f"image is for machine {machine:#x}, not AArch64")

    total = IMAGE_OFFSET + len(image)
    total = (total + ROM_GRANULE - 1) // ROM_GRANULE * ROM_GRANULE
    blocks = total // BLOCK
    if blocks > 0xFFFF:
        raise SystemExit("image too big for one ROM image")

    rom = bytearray(total)
    struct.pack_into(
        "<HHIHHH8xHH", rom, 0,
        ROM_SIGNATURE,
        blocks,          # initialisation size, in blocks
        EFI_SIGNATURE,
        subsystem,
        machine,
        NOT_COMPRESSED,
        IMAGE_OFFSET,    # where the EFI image starts
        PCIR_OFFSET,
    )
    struct.pack_into(
        "<4sHHHHB3sHHBBHHH", rom, PCIR_OFFSET,
        b"PCIR",
        VENDOR_ID,
        DEVICE_ID,
        0,               # no device list
        PCIR_LENGTH,
        PCIR_REVISION,
        CLASS_CODE.to_bytes(3, "little"),
        blocks,          # image length, in blocks
        0,               # code revision
        CODE_TYPE_EFI,
        LAST_IMAGE,
        0,               # maximum run-time image length
        0,               # configuration utility code header
        0,               # DMTF CLP entry point
    )
    rom[IMAGE_OFFSET:IMAGE_OFFSET + len(image)] = image
    return bytes(rom)


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    with open(sys.argv[1], "rb") as f:
        image = f.read()
    rom = build(image)
    with open(sys.argv[2], "wb") as f:
        f.write(rom)


if __name__ == "__main__":
    main()
