#!/bin/bash
# Finds the network card by name where the install pinned it to QEMU's default address. The install
# left a netplan that matches that one address, so a machine given an address of its own would only
# have the network the initrd's catch-all gives it. gpu-up.sh runs this at each boot and it changes
# nothing once it has done its work. Only a file exactly as the installer wrote it is replaced: a netplan
# someone wrote or changed is theirs, and stays.
set -uo pipefail
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"
file=$root/etc/netplan/00-installer-config.yaml
default_mac=52:54:00:12:34:56

[ -f "$file" ] || exit 0
name=$(sed -n "s/^    \\([A-Za-z0-9_.-]*\\):\$/\\1/p" "$file" | head -n 1)
[ -n "$name" ] || exit 0
expected=$(cat <<EOF
# This is the network config written by 'subiquity'
network:
  ethernets:
    $name:
      dhcp4: true
      dhcp6: true
      match:
        macaddress: '$default_mac'
      set-name: $name
  version: 2
EOF
)
[ "$(cat "$file")" = "$expected" ] || exit 0

new=$file.new
cat > "$new" <<'EOF'
# The network card is found by its name, not its address, so the machine can have an address of its own.
network:
  version: 2
  ethernets:
    all-en:
      match:
        name: "en*"
      dhcp4: true
      dhcp6: true
EOF
chmod 600 "$new"
mv "$new" "$file"
echo "The guest's network card is now found by name, not by QEMU's default address."
if [ -z "$root" ] && command -v netplan > /dev/null; then
    timeout 60 netplan apply || true
fi
