#!/bin/sh
# Runs at every start of a Linux guest on a Windows host. It puts the host's GPU files from the
# files disk at /usr/lib/wsl, where WSL has them, loads dxgkrnl, the guest's half of GPU-PV, building
# it first if this kernel has none, and says on the serial port what came of it.
set -u
say() {
    printf 'guesthouse: %s\n' "$*" > /dev/ttyS0
}

files=/run/guesthouse/files
mkdir -p "$files" /usr/lib/wsl
# FAT keeps no modes, so every file shows as readable and runnable, and none as writable.
if ! mountpoint -q "$files"; then
    mount -o ro,uid=0,gid=0,fmask=0222,dmask=0222 LABEL=CIDATA "$files" || say "files disk: not found"
fi
if [ -d "$files/wsl" ] && ! mountpoint -q /usr/lib/wsl; then
    mount --bind "$files/wsl" /usr/lib/wsl
fi
echo /usr/lib/wsl/lib > /etc/ld.so.conf.d/ld.wsl.conf
ldconfig 2> /dev/null

if ! modprobe dxgkrnl 2> /dev/null; then
    say "dxgkrnl: building it for $(uname -r)"
    log=/var/log/guesthouse-dxgkrnl.log
    if sh /usr/lib/guesthouse/dxgkrnl/build.sh > "$log" 2>&1 && modprobe dxgkrnl; then
        say "dxgkrnl: built"
    else
        say "dxgkrnl: the build failed, and its log's end follows"
        tail -n 30 "$log" | while IFS= read -r line; do say "build: $line"; done
    fi
fi

if [ -e /dev/dxg ]; then say "dxg: present"; else say "dxg: missing"; fi
for driver in /usr/lib/wsl/drivers/*; do
    [ -d "$driver" ] && say "host driver: ${driver##*/}"
done
if [ -x /usr/lib/wsl/lib/nvidia-smi ]; then
    /usr/lib/wsl/lib/nvidia-smi -L 2>&1 | while IFS= read -r line; do say "nvidia-smi: $line"; done
fi
ip -4 -o address show scope global | while read -r _ name _ address _; do
    say "address: $address on $name"
done
say "report done"
