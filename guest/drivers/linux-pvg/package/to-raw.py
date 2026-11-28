#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Turns a qcow2 disk into a sparse raw one with QEMU itself, as there is no
qemu-img on this Mac: QEMU with no machine, the two disks as block nodes, and
QMP's blockdev-backup from one to the other. Zeros are left as holes.

Usage: to-raw.py QEMU SOURCE.qcow2 TARGET.img [SIZE_GIB]
  SIZE_GIB makes the raw disk larger than the image (cloud-init grows the
  root partition at the first start). TARGET must not be there.
"""

import json
import os
import socket
import subprocess
import sys
import tempfile
import time


def main():
    qemu, source, target = sys.argv[1:4]
    size_gib = int(sys.argv[4]) if len(sys.argv) > 4 else 0
    if os.path.exists(target):
        sys.exit(f"{target} is there already")
    work = tempfile.mkdtemp(prefix="to-raw.", dir="/tmp")
    sock_path = os.path.join(work, "qmp.sock")
    proc = subprocess.Popen([qemu, "-M", "none", "-nodefaults", "-display", "none",
                             "-qmp", f"unix:{sock_path},server,wait=off",
                             "-blockdev", f"driver=file,filename={source},node-name=srcf,read-only=on",
                             "-blockdev", "driver=qcow2,file=srcf,node-name=src,read-only=on"])
    try:
        qmp = connect(sock_path)
        virtual = query_size(qmp, "src")
        size = max(virtual, size_gib << 30)
        # The backup wants two disks of one size; the raw one grows after.
        with open(target, "wb") as f:
            f.truncate(virtual)
        call(qmp, "blockdev-add", {"driver": "file", "filename": target, "node-name": "dstf",
                                   "discard": "unmap", "detect-zeroes": "unmap"})
        call(qmp, "blockdev-add", {"driver": "raw", "file": "dstf", "node-name": "dst",
                                   "discard": "unmap", "detect-zeroes": "unmap"})
        call(qmp, "blockdev-backup", {"job-id": "copy", "device": "src", "target": "dst", "sync": "full"})
        deadline = time.monotonic() + 600
        while True:
            message = read(qmp, 5)
            if message and message.get("event") == "BLOCK_JOB_COMPLETED":
                data = message["data"]
                if data.get("error"):
                    sys.exit(f"the copy failed: {data['error']}")
                print(f"{target}: {data['offset']} of {virtual} bytes copied, raw size {size}")
                break
            if time.monotonic() > deadline:
                sys.exit("the copy took more than 10 minutes")
        call(qmp, "quit")
        proc.wait(30)
        with open(target, "r+b") as f:
            f.truncate(size)
    finally:
        if proc.poll() is None:
            proc.kill()
        try:
            os.unlink(sock_path)
            os.rmdir(work)
        except OSError:
            pass


class Conn:
    def __init__(self, sock):
        self.sock = sock
        self.buffer = b""


def connect(path):
    for _ in range(100):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            s.connect(path)
            break
        except OSError:
            s.close()
            time.sleep(0.1)
    else:
        sys.exit("QEMU's QMP socket did not come")
    conn = Conn(s)
    read(conn, 10)  # the greeting
    call(conn, "qmp_capabilities")
    return conn


def read(conn, timeout):
    conn.sock.settimeout(timeout)
    while b"\n" not in conn.buffer:
        try:
            data = conn.sock.recv(65536)
        except socket.timeout:
            return None
        if not data:
            raise EOFError
        conn.buffer += data
    line, conn.buffer = conn.buffer.split(b"\n", 1)
    return json.loads(line)


def call(conn, name, arguments=None):
    message = {"execute": name}
    if arguments:
        message["arguments"] = arguments
    conn.sock.sendall(json.dumps(message).encode() + b"\n")
    while True:
        answer = read(conn, 30)
        if answer is None:
            sys.exit(f"{name}: no answer")
        if "event" in answer:
            continue
        if "error" in answer:
            sys.exit(f"{name}: {answer['error']}")
        return answer.get("return")


def query_size(conn, node):
    for entry in call(conn, "query-named-block-nodes"):
        if entry["node-name"] == node:
            return entry["image"]["virtual-size"]
    sys.exit(f"no node {node}")


if __name__ == "__main__":
    main()
