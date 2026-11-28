#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
#
# Lists what a refused command buffer held: the bytes wm_refused.c keeps in
# the guest, its resources, and for each
# segment its kind and its commands, each with its number, its length and
# its first words. The numbers are pvg_wire's: render 26 is a pass, 137
# useResource, 206 a compute texture binding, and so on (pvg_wire.c).
#
# Usage: refused.py FILE...
#        refused.py --klog KLOG.txt FOLDER
# The second form takes the "keptbin NAME OFFSET HEX" lines a sandboxed
# process sent to the driver's log (wddmtool klog) and writes each kept
# command buffer to FOLDER/NAME, ready for the first form.

import os
import struct
import sys

SEGMENT_KINDS = {0: "render", 1: "compute", 2: "blit"}
SEGMENT_HEAD = 8   # the length, then a byte of kind and three more
COMMAND_HEAD = 8   # the number, then the length with these counted
WORDS_SHOWN = 6


def words(data, at, count):
    count = min(count, (len(data) - at) // 4)
    return list(struct.unpack_from("<%dI" % count, data, at))


def segment(data, index):
    length = struct.unpack_from("<I", data, 0)[0]
    kind = SEGMENT_KINDS.get(data[4], "kind %d" % data[4])
    print("segment %d: %s, %d bytes (head says %d), byte 5 %d" % (index, kind, len(data), length, data[5]))
    at = SEGMENT_HEAD
    while at + COMMAND_HEAD <= len(data):
        number, size = struct.unpack_from("<II", data, at)
        if size < COMMAND_HEAD or at + size > len(data):
            print("  at %d: number %d, length %d does not fit; the rest is not read" % (at, number, size))
            return
        shown = " ".join("%x" % w for w in words(data, at + COMMAND_HEAD, WORDS_SHOWN))
        print("  at %5d: command %3d, %4d bytes: %s" % (at, number, size, shown))
        at += size
    if at != len(data):
        print("  %d bytes left over" % (len(data) - at))


def main(paths):
    for path in paths:
        data = open(path, "rb").read()
        if data[:4] != b"RXRF":
            print("%s: not a refused command buffer" % path)
            continue
        at = 4
        (count,) = struct.unpack_from("<I", data, at)
        at += 4
        resources = [struct.unpack_from("<II", data, at + 8 * i) for i in range(count)]
        at += 8 * count
        print("%s: %d resources: %s" % (path, count, " ".join(
            "%d%s" % (number, "w" if flags else "") for number, flags in resources)))
        (segments,) = struct.unpack_from("<I", data, at)
        at += 4
        for i in range(segments):
            (size,) = struct.unpack_from("<I", data, at)
            at += 4
            segment(data[at:at + size], i)
            at += size


def from_klog(klog, folder):
    """The kept command buffers in a driver log, one file each, keyed by the sending process too."""
    files = {}
    for line in open(klog, encoding="utf-8", errors="replace"):
        words = line.split()
        if len(words) == 5 and words[1] == "keptbin":
            pid = words[0].strip("[]")
            name = words[2] if pid in words[2] else "%s-%s" % (pid, words[2])
            files.setdefault(name, {})[int(words[3])] = bytes.fromhex(words[4])
    for name, parts in files.items():
        data = b"".join(parts[at] for at in sorted(parts))
        with open(os.path.join(folder, name), "wb") as out:
            out.write(data)
        print("%s: %d bytes" % (name, len(data)))


if __name__ == "__main__":
    if sys.argv[1:2] == ["--klog"]:
        from_klog(sys.argv[2], sys.argv[3])
    else:
        main(sys.argv[1:])
