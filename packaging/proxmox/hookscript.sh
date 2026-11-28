#!/bin/sh
# A Proxmox VE hookscript for a VM with Guesthouse's GPU. Proxmox runs it as root around each start and stop,
# with the VM's ID and the phase. It starts the VM's daemon before QEMU and stops it after QEMU has gone, as
# docs/standalone.md says the two have to go. setup-vm.sh puts it in a snippets storage and names it in the
# VM's config, and docs/proxmox.md says how it's used.
#
# What it reads is /etc/guesthouse/proxmox/VMID.conf, which setup-vm.sh writes:
#
#   user=NAME        the user that installed Guesthouse and ran its setup. The daemon runs as that user.
#   slot=N           the daemon's slot, 0 to 3, which names its run folder.
#   ram=SIZE         the guest's RAM, like 8G. It has to match the VM's memory: and its RAM backend's size=.
#   daemon=PATH      the daemon, if it isn't the one in the user's install.
#   daemon_env=...   more of the daemon's environment, as NAME=VALUE words, such as GUESTHOUSE_AMD_CARD.
#
# A VM without the file doesn't start, as QEMU would wait for sockets nothing makes.
PATH=/usr/sbin:/usr/bin:/sbin:/bin
export PATH

vmid=${1:-}
phase=${2:-}
conf=/etc/guesthouse/proxmox/$vmid.conf
# What has to be left of the host's RAM once the guest's is taken.
RESERVE_MIB=2048

say() {
    printf 'guesthouse hookscript, VM %s, %s: %s\n' "$vmid" "$phase" "$*"
}

die() {
    say "$*" >&2
    exit 1
}

load() {
    case $vmid in
        '' | *[!0-9]*) die "Proxmox gave no VM ID." ;;
    esac
    [ -r "$conf" ] || die "$conf isn't there, so the VM isn't set up for the GPU. setup-vm.sh writes it."
    # Read, not sourced, as some of it came from what the user's own program printed.
    user='' slot='' ram='' daemon='' daemon_env=''
    while IFS= read -r line; do
        case $line in
            user=*) user=${line#user=} ;;
            slot=*) slot=${line#slot=} ;;
            ram=*) ram=${line#ram=} ;;
            daemon=*) daemon=${line#daemon=} ;;
            daemon_env=*) daemon_env=${line#daemon_env=} ;;
        esac
    done <"$conf"
    if [ -z "$user" ] || [ -z "$slot" ] || [ -z "$ram" ]; then
        die "$conf needs user, slot and ram."
    fi
    case $user in
        -* | *[!A-Za-z0-9_.-]*) die "$user isn't a user's name." ;;
    esac
    set -f
    for word in $daemon_env; do
        printf '%s\n' "$word" | grep -Eqx 'GUESTHOUSE_[A-Z0-9_]+=[A-Za-z0-9_@%+=:,./-]*' ||
            die "daemon_env has $word, and only GUESTHOUSE_NAME=VALUE words, with no quotes or spaces, go there."
    done
    set +f
    uid=$(id -u "$user" 2>/dev/null) || die "there's no user $user."
    gid=$(id -g "$user")
    home=$(getent passwd "$user" | cut -d: -f6)
    daemon=${daemon:-$home/.local/lib/guesthouse/current/guesthouse-daemon}
    base=/run/user/$uid/guesthouse
    case $slot in
        0) run=$base/run ;;
        1 | 2 | 3) run=$base/run-$slot ;;
        *) die "the slot is 0 to 3, not $slot." ;;
    esac
    case $ram in
        [1-9]*G) gib=${ram%G} ;;
        *) die "ram is a number of GiB, like 8G, not $ram." ;;
    esac
    case $gib in
        *[!0-9]*) die "ram is a number of GiB, like 8G, not $ram." ;;
    esac
    unit=guesthouse-daemon-$vmid
    ram_file=/dev/shm/gpupv-ram.$vmid
    window_file=/dev/shm/gpupv-window.$vmid
}

# The daemon goes, and so do its sockets and the two memory files. The RAM file still holds what the guest had.
clean_up() {
    systemctl stop "$unit" 2>/dev/null
    systemctl reset-failed "$unit" 2>/dev/null
    rm -f "$ram_file" "$window_file"
    # As the user, since the folder is the user's, and root follows no link of theirs.
    runuser -u "$user" -- rm -f "$run/vgpu.sock" "$run/global.sock" 2>/dev/null
}

# The config's lines that have to agree with this file. Without them QEMU would start on other files or sockets.
check_config() {
    config=$(qm config "$vmid" --current) || die "qm config failed."
    memory=$(printf '%s\n' "$config" | sed -n 's/^memory: //p')
    [ "$memory" = $((gib * 1024)) ] || die "memory: is ${memory:-unset}, and ram=$ram needs $((gib * 1024))."
    args=$(printf '%s\n' "$config" | sed -n 's/^args: //p')
    for word in "id=ram0,size=$ram,mem-path=$ram_file,share=on" "id=gpio,size=64G,mem-path=$window_file,share=on" \
        "path=$run/global.sock," "path=$run/vgpu.sock," "iomem-base=" ",owner=$uid "; do
        case " $args " in
            *"$word"*) ;;
            *) die "args: has no $word. Run setup-vm.sh again." ;;
        esac
    done
}

# Another daemon in the same slot, Guesthouse's own or another VM's, would have the same sockets.
check_slot() {
    for pid in $(pgrep -u "$user" -x guesthouse-daem); do
        grep -q "/$unit.service" "/proc/$pid/cgroup" 2>/dev/null && continue
        other=$(tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | sed -n 's/^GUESTHOUSE_SLOT=//p')
        [ "${other:-0}" != "$slot" ] || die "slot $slot is taken, by the daemon with PID $pid."
    done
}

check_room() {
    available=$(awk '/^MemAvailable:/ { print int($2 / 1024) }' /proc/meminfo)
    [ "$available" -ge $((gib * 1024 + RESERVE_MIB)) ] ||
        die "$available MiB of RAM is available, and the guest needs $((gib * 1024)) and $RESERVE_MIB to spare."
    shm=$(df -Pm /dev/shm | awk 'NR == 2 { print $4 }')
    [ "$shm" -ge $((gib * 1024)) ] || die "/dev/shm has $shm MiB free, and the guest's RAM needs $((gib * 1024))."
}

start_daemon() {
    [ -x "$daemon" ] || die "there's no daemon at $daemon."
    # At boot, with onboot: 1, the user's runtime folder can come a little after Proxmox starts the VM.
    tries=0
    until mountpoint -q "/run/user/$uid"; do
        tries=$((tries + 1))
        [ "$tries" -le 90 ] || die "/run/user/$uid isn't there. Is lingering on for $user?"
        sleep 1
    done
    check_config
    check_slot
    clean_up
    check_room
    runuser -u "$user" -- mkdir -p "$run" || die "$run can't be made."
    set --
    set -f
    for word in $daemon_env; do
        set -- "$@" "--setenv=$word"
    done
    set +f
    # systemd expands $ in a command, so it gets the paths already in it, and they can't have a space.
    for path in "$daemon" "$run"; do
        case $path in
            *[!A-Za-z0-9_./:-]*) die "$path can only have letters, digits and _ . / : -" ;;
        esac
    done
    # Before pve-guests, so a shutdown of the host stops the daemon after the guest. After the user's manager,
    # so the run folder is still there for it.
    systemd-run --quiet --unit="$unit" --description="Guesthouse daemon for VM $vmid" --uid="$uid" --gid="$gid" \
        --property=Before=pve-guests.service --property="After=user@$uid.service" \
        --setenv="HOME=$home" --setenv="USER=$user" --setenv="LOGNAME=$user" --setenv="XDG_RUNTIME_DIR=/run/user/$uid" \
        --setenv=PATH=/usr/local/bin:/usr/bin:/bin --setenv="GUESTHOUSE_SLOT=$slot" --setenv="GUESTHOUSE_RAM=$ram" \
        --setenv="GUESTHOUSE_RAM_FILE=$ram_file" --setenv="GUESTHOUSE_WINDOW_FILE=$window_file" "$@" \
        /bin/sh -c "exec $daemon $base >$run/daemon.out 2>&1" || die "systemd-run couldn't start the daemon."
    # QEMU connects to both sockets as a client, so both have to be there before it starts.
    tries=0
    until [ -S "$run/vgpu.sock" ] && [ -S "$run/global.sock" ]; do
        tries=$((tries + 1))
        if [ "$tries" -gt 150 ] || { [ "$tries" -gt 5 ] && ! systemctl is-active --quiet "$unit"; }; then
            # As the user, since the folder is the user's.
            runuser -u "$user" -- tail -n 15 "$run/daemon.out" >&2
            clean_up
            die "the daemon didn't come up, and $run/daemon.out has the rest."
        fi
        sleep 0.2
    done
    say "the daemon is up in slot $slot, as $unit, with $ram for the guest."
}

case $phase in
    pre-start)
        load
        start_daemon
        ;;
    post-stop)
        load
        clean_up
        say "the daemon has stopped, and the memory files are gone."
        ;;
esac
exit 0
