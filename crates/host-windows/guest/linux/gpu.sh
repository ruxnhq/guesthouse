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

# The listener for the host's request to shut down is a service of its own, so it outlives this
# script. A guest installed before it existed has none, and this script comes from the files disk at
# every start, so it puts the listener in place and starts it here, before anything slow. A guest
# that has it gets its files refreshed, and the service is started only if it isn't running.
for file in shutdown.sh guesthouse-shutdown.service; do
    if [ -f "$here/$file" ] && ! cmp -s "$here/$file" "/usr/lib/guesthouse/$file"; then
        mkdir -p /usr/lib/guesthouse
        cp "$here/$file" "/usr/lib/guesthouse/$file"
    fi
done
if [ -f /usr/lib/guesthouse/guesthouse-shutdown.service ]; then
    systemctl enable /usr/lib/guesthouse/guesthouse-shutdown.service 2>&1 | say_each shutdown
    systemctl is-active -q guesthouse-shutdown.service || systemctl start guesthouse-shutdown.service 2>&1 | say_each shutdown
fi

# The serial port is the host's window on this guest, and dxgkrnl logs each driver request the host
# refuses as a kernel error, a handful per nvidia-smi, which is normal over GPU-PV. Only the kernel's
# critical messages and worse go to the console now. The rest are still in dmesg and the journal,
# and this script writes to the port itself, so its lines are unaffected.
printf 'kernel.printk = 3 4 1 3\n' > /etc/sysctl.d/90-guesthouse-console.conf
sysctl -q -p /etc/sysctl.d/90-guesthouse-console.conf

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
# WSL puts these on everyone's PATH, nvidia-smi among them, and so does this. The single quotes
# keep $PATH for the login shell to expand.
# shellcheck disable=SC2016
printf 'PATH="$PATH:/usr/lib/wsl/lib"\n' > /etc/profile.d/guesthouse-wsl.sh

# SSH over a Hyper-V socket, so the host reaches this guest's sshd with no network: a socket unit on
# vsock port 22 that hands each connection to sshd on the loopback. install.sh puts the units in
# a new guest, and this puts them in one installed before they existed, or updates them, as the
# files disk holds the host's own copies and the host writes them afresh at every start.
units=/etc/systemd/system
changed=no
for unit in guesthouse-ssh.socket guesthouse-ssh.service; do
    if [ -f "$files/guesthouse/$unit" ] && ! cmp -s "$files/guesthouse/$unit" "$units/$unit"; then
        install -m 644 "$files/guesthouse/$unit" "$units/$unit" && changed=yes
    fi
done
if [ -f "$files/guesthouse/guesthouse-hv_sock.conf" ] &&
    ! cmp -s "$files/guesthouse/guesthouse-hv_sock.conf" /etc/modules-load.d/guesthouse-hv_sock.conf; then
    install -m 644 "$files/guesthouse/guesthouse-hv_sock.conf" /etc/modules-load.d/guesthouse-hv_sock.conf
fi
if [ -f "$units/guesthouse-ssh.socket" ]; then
    modprobe hv_sock 2> /dev/null
    if [ "$changed" = yes ]; then
        systemctl daemon-reload
    fi
    if ! systemctl is-enabled --quiet guesthouse-ssh.socket 2> /dev/null; then
        systemctl enable guesthouse-ssh.socket 2>&1 | say_each ssh
    fi
    systemctl start guesthouse-ssh.socket 2>&1 | say_each ssh
    if systemctl is-active --quiet guesthouse-ssh.socket; then
        say "ssh: listening on vsock port 22"
    else
        say "ssh: the socket isn't listening, so the host's SSH port reaches nothing"
    fi
fi

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

# The disk can be made larger while the machine is stopped, so the root partition, the last on it,
# takes whatever room it has at every start. growpart says NOCHANGE and fails when there's none.
root=$(findmnt -no SOURCE /)
root_name=${root##*/}
parent=$(lsblk -no PKNAME "$root" 2> /dev/null | head -n 1)
number=$(cat "/sys/class/block/$root_name/partition" 2> /dev/null)
if [ -n "$parent" ] && [ -n "$number" ] && command -v growpart > /dev/null; then
    if grown=$(growpart "/dev/$parent" "$number" 2>&1); then
        say "disk: $grown"
        resize2fs "$root" 2>&1 | say_each disk
    fi
fi

# The host's shared folder, when it gives one, by 9p over a Hyper-V socket to the host's port 564,
# with the options hcsshim's Linux guests mount theirs with (internal/guest/storage/plan9). The host writes the file naming it at
# each boot, and takes the file away when the machine has no share, and then it's unmounted.
share=/mnt/shared
if [ -f "$files/guesthouse/share" ]; then
    read -r share_name share_mode < "$files/guesthouse/share"
    mkdir -p "$share"
    if ! mountpoint -q "$share"; then
        modprobe hv_sock 2> /dev/null
        # The socket's descriptor is handed to mount, which keeps the connection once it has it.
        python3 - "$share_name" "$share_mode" "$share" << 'EOF' 2>&1 | say_each share
import socket, subprocess, sys
name, mode, target = sys.argv[1:4]
packet = 65536
connection = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
connection.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, packet)
connection.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, packet)
connection.connect((socket.VMADDR_CID_HOST, 564))
fd = connection.fileno()
options = f"trans=fd,rfdno={fd},wfdno={fd},msize={packet}"
if mode == "ro":
    options += ",ro,noload"
options += f",aname={name}"
sys.exit(subprocess.run(["mount", "-t", "9p", "-o", options, target, target], pass_fds=[fd]).returncode)
EOF
    fi
    if mountpoint -q "$share"; then say "share: at $share"; else say "share: not mounted"; fi
elif mountpoint -q "$share"; then
    umount "$share"
fi

if [ -e /dev/dxg ]; then say "dxg: present"; else say "dxg: missing"; fi
for driver in /usr/lib/wsl/drivers/*; do
    [ -d "$driver" ] && say "host driver: ${driver##*/}"
done
if [ -x /usr/lib/wsl/lib/nvidia-smi ]; then
    /usr/lib/wsl/lib/nvidia-smi -L 2>&1 | say_each nvidia-smi
fi
say "report done"
