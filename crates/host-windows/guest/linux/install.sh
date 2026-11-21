#!/bin/sh
# Run by Ubuntu's installer once it has installed: puts Guesthouse's files in the new system,
# makes its administrator, and turns on the service that gives it the host's GPU at every start.
# Usage: install.sh FILES_DISK TARGET
set -eu
files=$1
target=$2

mkdir -p "$target/usr/lib/guesthouse"
cp -r "$files/guesthouse/." "$target/usr/lib/guesthouse/"
rm -f "$target/usr/lib/guesthouse/account"

# The account file holds "user password". It's deleted once read, so the password is kept nowhere
# but the guest's own shadow file and the host's account file beside the disk.
read -r user password < "$files/guesthouse/account"
chroot "$target" useradd --create-home --shell /bin/bash --groups adm,sudo "$user"
printf '%s:%s\n' "$user" "$password" | chroot "$target" chpasswd
rm -f "$files/guesthouse/account" "$files/user-data"

chroot "$target" systemctl enable /usr/lib/guesthouse/guesthouse-gpu.service
