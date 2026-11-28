#!/bin/sh
# Started by guesthouse-gpu.service at every start. It mounts the files disk and runs the gpu.sh on
# it, which boot writes afresh each time, so a host with a newer Guesthouse gives the guest its newer
# script with no reinstall. Without the files disk it runs the copy made at install.
set -u
files=/run/guesthouse/files
mkdir -p "$files"
# FAT keeps no modes, so every file shows as readable and runnable, and none as writable.
if mountpoint -q "$files" || mount -o ro,uid=0,gid=0,fmask=0222,dmask=0222 LABEL=CIDATA "$files"; then
    if [ -f "$files/guesthouse/gpu.sh" ]; then
        exec sh "$files/guesthouse/gpu.sh"
    fi
fi
exec sh /usr/lib/guesthouse/gpu.sh
