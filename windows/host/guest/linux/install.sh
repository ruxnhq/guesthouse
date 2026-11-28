#!/bin/sh
# Run by Ubuntu's installer once it has installed: puts Guesthouse's files in the new system,
# makes its administrator, and turns on the service that gives it the host's GPU at every start, and the one that shuts it
# down when the host asks.
# Usage: install.sh FILES_DISK TARGET
set -eu
files=$1
target=$2

mkdir -p "$target/usr/lib/guesthouse"
cp -r "$files/guesthouse/." "$target/usr/lib/guesthouse/"
rm -f "$target/usr/lib/guesthouse/account"

# The account file holds "user password". The guest has the files disk read-only, so the host
# takes the file off it, with user-data, before the installed system first starts. The password is
# then kept nowhere but the guest's own shadow file and the host's account file beside the disk.
read -r user password < "$files/guesthouse/account"
chroot "$target" useradd --create-home --shell /bin/bash --groups adm,sudo "$user"
printf '%s:%s\n' "$user" "$password" | chroot "$target" chpasswd

chroot "$target" systemctl enable /usr/lib/guesthouse/guesthouse-gpu.service
chroot "$target" systemctl enable /usr/lib/guesthouse/guesthouse-shutdown.service

# SSH over a Hyper-V socket, for the host to reach this guest with no network. The units go where
# systemd looks for them, as the socket finds its service by name, and gpu.sh puts them there again
# at every start, so a guest installed before them has them too.
cp "$files/guesthouse/guesthouse-ssh.socket" "$files/guesthouse/guesthouse-ssh.service" \
    "$target/etc/systemd/system/"
cp "$files/guesthouse/guesthouse-hv_sock.conf" "$target/etc/modules-load.d/"
chroot "$target" systemctl enable guesthouse-ssh.socket
