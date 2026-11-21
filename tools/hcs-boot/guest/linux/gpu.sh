#!/bin/sh
# Runs at every start of a Linux guest on a Windows host. It makes sure the network card asks for an
# address, puts the host's GPU files from the files disk at /usr/lib/wsl, where WSL has them, loads
# dxgkrnl, the guest's half of GPU-PV, building it first if this kernel has none, and says on the
# serial port what came of each.
set -u
here=$(dirname "$0")
say() {
    printf 'guesthouse: %s\n' "$*" > /dev/ttyS0
}
say_each() {
    while IFS= read -r line; do say "$1: $line"; done
}

# Hyper-V's network card, by its driver, so it asks for an address whatever its MAC, which the
# installer's own settings may be tied to.
netplan=/etc/netplan/90-guesthouse.yaml
if [ ! -f "$netplan" ]; then
    printf 'network:\n  version: 2\n  ethernets:\n    guesthouse:\n      match:\n        driver: hv_netvsc\n      dhcp4: true\n' > "$netplan"
    chmod 600 "$netplan"
    netplan apply 2>&1 | say_each netplan
fi
# DHCP may still be answering, so the address gets a minute before anything needs the network.
waited=0
while [ "$waited" -lt 60 ] && [ -z "$(ip -4 -o address show scope global)" ]; do
    sleep 2
    waited=$((waited + 2))
done
ip -o link show | say_each link
ip -o address show | say_each address
networkctl --no-legend list 2>&1 | say_each networkctl
journalctl -b -u systemd-networkd --no-pager -n 15 2>&1 | say_each networkd
resolvectl dns 2>&1 | say_each dns

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
    if sh "$here/dxgkrnl/build.sh" > "$log" 2>&1 && modprobe dxgkrnl; then
        say "dxgkrnl: built"
    else
        say "dxgkrnl: the build failed, and its log's end follows"
        tail -n 30 "$log" | say_each build
    fi
fi

if [ -e /dev/dxg ]; then say "dxg: present"; else say "dxg: missing"; fi
for driver in /usr/lib/wsl/drivers/*; do
    [ -d "$driver" ] && say "host driver: ${driver##*/}"
done
if [ -x /usr/lib/wsl/lib/nvidia-smi ]; then
    /usr/lib/wsl/lib/nvidia-smi -L 2>&1 | say_each nvidia-smi
fi
say "report done"
