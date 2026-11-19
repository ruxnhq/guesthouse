#!/usr/bin/env python3
"""Splits a Guesthouse command buffer dump (GUESTHOUSE_AMD_IB_DUMP) into one raw .bin file per
command buffer, which umr decodes with `umr -f <asic> -df <file>.bin`, and walks each one's PM4
headers to see that the packet lengths add up to the buffer.

    split.py ib-dump-hung-1.bin [out-folder]
"""

import sys
from pathlib import Path


def fields(line):
    words = line.split()
    return words[0], dict(word.split("=", 1) for word in words[1:] if "=" in word)


# A NOP with the largest count is one dword of padding, as the card reads it.
NOP_PAD = 0xFFFF1000


def walk(data):
    """Packets in a PM4 stream, and how far the walk got: stops at a type 0 or 1 header, which this
    driver's graphics and compute buffers don't use."""
    words = [int.from_bytes(data[at:at + 4], "little") for at in range(0, len(data) - 3, 4)]
    at, packets = 0, 0
    while at < len(words):
        kind = words[at] >> 30
        if words[at] == NOP_PAD:
            at += 1
        elif kind == 3:
            at += 2 + ((words[at] >> 16) & 0x3FFF)
        elif kind == 2:
            at += 1
        else:
            break
        packets += 1
    return packets, at, len(words)


def main():
    dump = Path(sys.argv[1])
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else dump.with_suffix("")
    out.mkdir(parents=True, exist_ok=True)
    data = dump.read_bytes()
    at, run, index = 0, None, []
    while at < len(data):
        end = data.index(b"\n", at)
        line = data[at:end].decode()
        at = end + 1
        kind, values = fields(line)
        if kind == "run":
            run = values
            number = sum(1 for entry in index if entry.startswith("run ")) + 1
            index.append(f"run {number}: {line[4:]}")
        elif kind == "ib":
            copied = int(values["copied"], 16)
            body = data[at:at + copied]
            at += copied + 1
            if not copied:
                index.append(f"  {line[3:]}  (not copied)")
                continue
            name = f"run{number:03}-q{run['queue'][2:]}-f{run['fence'][2:]}-{values['address'][2:]}.bin"
            (out / name).write_bytes(body)
            packets, reached, words = walk(body)
            whole = "lengths add up" if reached == words else f"walk stopped at dword {reached} of {words}"
            index.append(f"  {line[3:]}  {name}: {packets} packets, {whole}")
        elif kind not in ("end", ""):
            index.append(line)
    (out / "index.txt").write_text("\n".join(index) + "\n")
    print("\n".join(index))
    print(f"\nwritten to {out}; decode one with: umr -f <asic> -df {out}/<file>.bin")


if __name__ == "__main__":
    main()
