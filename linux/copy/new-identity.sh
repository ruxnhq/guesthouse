#!/bin/sh
# Gives the guest of a copy of a machine identities of its own: new SSH host keys and a new machine
# id, and restarts it, as the machine id is read once at boot by systemd and by what it starts. The
# host sends this on stdin to `sh -s` over SSH, as root, at the copy's first start.
#
# The host name stays: every guest Guesthouse installs has the same one.
set -eu
# Each of these can be set by a test, which has no guest.
root=${GUESTHOUSE_GUEST_ROOT:-}
uuid_source=${GUESTHOUSE_UUID_SOURCE:-/proc/sys/kernel/random/uuid}
marker=$root/var/lib/nvfwd/first-boot-done
# What the host takes as "not yet", and asks again.
not_yet=3
# Long enough for the host to read the answer before SSH goes down.
restart_after=3s

# The first boot restarts the guest by itself, and a restart now would cut it short. So would one
# while the GPU step builds its driver, or while another restart is on its way.
[ -f "$marker" ] || {
    echo "Its first boot has not finished."
    exit "$not_yet"
}
if systemctl list-jobs 2>/dev/null | grep -Eq 'reboot|shutdown|poweroff|nvfwd-gpu\.service'; then
    echo "Its GPU step or a restart is still under way."
    exit "$not_yet"
fi

# Made beside the old ones and then moved in, so a guest cut off here still has a whole set.
made=$(mktemp -d "$root/etc/ssh/.guesthouse-keys.XXXXXX")
mkdir -p "$made/etc/ssh"
ssh-keygen -A -f "$made" >/dev/null
rm -f "$root"/etc/ssh/ssh_host_*_key "$root"/etc/ssh/ssh_host_*_key.pub
mv "$made"/etc/ssh/ssh_host_* "$root/etc/ssh/"
rm -rf "$made"

# Written whole and then put in place. A full id, not an empty file: an empty one makes the next
# boot a first boot, which applies systemd's presets to every unit.
tr -d '-' <"$uuid_source" >"$root/etc/machine-id.new"
chmod 444 "$root/etc/machine-id.new"
mv "$root/etc/machine-id.new" "$root/etc/machine-id"
# D-Bus keeps its own where it isn't a link to systemd's, as on older systems.
dbus=$root/var/lib/dbus/machine-id
if [ -f "$dbus" ] && [ ! -L "$dbus" ]; then
    ln -sf /etc/machine-id "$dbus"
fi

echo renewed
systemd-run --quiet --on-active="$restart_after" systemctl reboot
