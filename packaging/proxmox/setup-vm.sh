#!/bin/sh
# Sets a Proxmox VE VM up to run a Guesthouse machine's Windows with the GPU. It takes the GPU's words from
# `guesthouse-vm command MACHINE`, puts them in the VM's args:, sets memory: to the machine's RAM, puts
# hookscript.sh in a snippets storage and names it as the VM's hookscript, and writes what the hookscript reads
# to /etc/guesthouse/proxmox/VMID.conf. docs/proxmox.md says what else the VM needs, and what has run.
#
# Usage: packaging/proxmox/setup-vm.sh [--user USER] [--slot N] [--ram SIZE] [--storage STORAGE]
#                                      [--render-address ADDRESS] [--dry-run] VMID MACHINE
#
# It runs as root on the Proxmox host, with the VM stopped. USER is the one that installed Guesthouse and made
# MACHINE, the user sudo was run by if it isn't given. N is the daemon's slot, 0 to 3, and the lowest of 1 to 3
# that no other VM here has if it isn't given, as Guesthouse's own machines take 0 first. SIZE is the guest's RAM,
# like 8G, the machine's if it isn't given. STORAGE holds snippets, local if it isn't given. ADDRESS is the render
# device's slot on pcie.0, 0x6 if it isn't given, or none for QEMU's choice. --dry-run says what it would do.
#
# It can run again, after an update of Guesthouse or with other options. It only changes the GPU's words in
# args:, and leaves the rest of the line as it was.
set -eu

CONF_DIR=/etc/guesthouse/proxmox
HOOK_NAME=guesthouse.sh
HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)

say() {
    printf '%s\n' "$*"
}

die() {
    printf 'setup-vm: %s\n' "$*" >&2
    exit 1
}

# The GPU's words, which the VM's args: may have from an earlier run. A flag and its value are one item here.
is_gpu_word() {
    case $1 in
        "-device vmbus-bridge,"* | "-device hv-timesync" | "-device gpupv-"* | "-chardev socket,id=gpg,"* | \
            "-chardev socket,id=gpv,"* | "-object memory-backend-file,id=gpio,"* | \
            "-object memory-backend-file,id=ram0,"* | "-machine memory-backend=ram0") return 0 ;;
    esac
    return 1
}

# Runs a command as USER in a session of its own, with no terminal and nothing to read, so it has no part in root's.
as_user() {
    as_user_name=$1
    shift
    setsid -w runuser -u "$as_user_name" -- "$@" </dev/null
}

# Whether VALUE, of one line, is wholly of the shape REGEX, an extended one.
shape() {
    case $1 in
        '' | *'
'*) return 1 ;;
    esac
    printf '%s\n' "$1" | grep -Eqx -e "$2"
}

# Replaces the first OLD in LINE with NEW, as plain text.
swap_first() {
    case $1 in
        *"$2"*) printf '%s%s%s' "${1%%"$2"*}" "$3" "${1#*"$2"}" ;;
        *) printf '%s' "$1" ;;
    esac
}

# What QEMU's line in the printed script has, one flag and its value on each line.
qemu_words() {
    awk '
        # The line of QEMU itself, after any of its environment, which the script prints first.
        /qemu-system-x86_64 \\$/ && !/^ *-/ && !/^ *#/ { inside = 1; next }
        inside && /^  -/ {
            line = $0
            sub(/^  /, "", line)
            sub(/ \\$/, "", line)
            print line
            if ($0 !~ / \\$/)
                inside = 0
            next
        }
        { inside = 0 }
    '
}

main() {
    user=${SUDO_USER:-} slot='' ram='' storage=local render_address=0x6 dry_run=''
    while [ $# -gt 0 ]; do
        case $1 in
            --user) user=$2 && shift 2 ;;
            --slot) slot=$2 && shift 2 ;;
            --ram) ram=$2 && shift 2 ;;
            --storage) storage=$2 && shift 2 ;;
            --render-address) render_address=$2 && shift 2 ;;
            --dry-run) dry_run=1 && shift ;;
            -*) die "there's no option $1." ;;
            *) break ;;
        esac
    done
    [ $# -eq 2 ] || die "name the VM's ID and the Guesthouse machine, such as: setup-vm.sh 101 windows"
    vmid=$1
    machine=$2
    case $vmid in
        '' | *[!0-9]*) die "$vmid isn't a VM ID." ;;
    esac
    [ "$(id -u)" = 0 ] || die "this runs as root, on the Proxmox host."
    command -v qm >/dev/null || die "there's no qm, so this isn't a Proxmox VE host."
    [ -n "$user" ] || die "name the user that installed Guesthouse, with --user."
    [ "$user" != root ] || die "Guesthouse runs as an ordinary user, not as root."
    uid=$(id -u "$user" 2>/dev/null) || die "there's no user $user."
    home=$(getent passwd "$user" | cut -d: -f6)
    program=$home/.local/lib/guesthouse/current
    [ -x "$program/guesthouse-vm" ] || die "$user has no Guesthouse at $program."
    [ -f "$HERE/hookscript.sh" ] || die "hookscript.sh isn't beside this script."

    config=$(qm config "$vmid") || die "there's no VM $vmid."
    status=$(qm status "$vmid")
    [ "$status" = "status: stopped" ] || [ -n "$dry_run" ] || die "VM $vmid has to be stopped first."
    check_vm

    if [ -z "$slot" ]; then
        slot=$(free_slot)
    fi
    case $slot in
        0 | 1 | 2 | 3) ;;
        *) die "the slot is 0 to 3, not $slot." ;;
    esac
    case $render_address in
        none | 0x[0-9a-f] | 0x1[0-9a-f]) ;;
        *) die "the render device's address is a slot on pcie.0 such as 0x6, or none, not $render_address." ;;
    esac

    say "asking Guesthouse for $machine's words, as $user"
    printed=$(as_user "$user" env -i HOME="$home" USER="$user" LOGNAME="$user" PATH=/usr/local/bin:/usr/bin:/bin \
        XDG_RUNTIME_DIR="/run/user/$uid" GUESTHOUSE_PROGRAM="$program" "$program/guesthouse-vm" command "$machine") ||
        die "guesthouse-vm command $machine failed."
    make_words
    write_conf
    make_args

    hook=$(pvesm path "$storage:snippets/$HOOK_NAME") || die "there's no storage $storage."
    pvesm status --content snippets 2>/dev/null | awk 'NR > 1 { print $1 }' | grep -qx "$storage" ||
        die "storage $storage doesn't hold snippets. pvesm set $storage --content with snippets added turns it on."

    if [ -n "$dry_run" ]; then
        printf '\n%s\n\n%s\n\n%s\n\n%s\n' "it would put hookscript.sh at $hook, and write $CONF_DIR/$vmid.conf:" \
            "$conf_text" "and run:" \
            "qm set $vmid --memory $memory --balloon 0 --hookscript $storage:snippets/$HOOK_NAME --args '$args'"
        return 0
    fi
    mkdir -p "$(dirname -- "$hook")" "$CONF_DIR"
    install -m 0755 "$HERE/hookscript.sh" "$hook"
    printf '%s\n' "$conf_text" >"$CONF_DIR/$vmid.conf.new"
    # The config first, so a qm set that fails leaves the file and args: as they were, and agreeing.
    qm set "$vmid" --memory "$memory" --balloon 0 --hookscript "$storage:snippets/$HOOK_NAME" --args "$args" >/dev/null || {
        rm -f "$CONF_DIR/$vmid.conf.new"
        die "qm set failed, and VM $vmid is as it was."
    }
    mv "$CONF_DIR/$vmid.conf.new" "$CONF_DIR/$vmid.conf"
    say "VM $vmid is set up, with $ram in slot $slot. qm start $vmid starts it with the GPU."
}

# What Windows on GPU-PV needs of the VM, that this script doesn't set itself.
check_vm() {
    printf '%s\n' "$config" | grep -q '^bios: ovmf$' || die "VM $vmid needs bios: ovmf, the UEFI firmware."
    printf '%s\n' "$config" | grep -Eq '^machine: (pc-)?q35' || die "VM $vmid needs a q35 machine type."
    printf '%s\n' "$config" | grep -Eq '^ostype: win(10|11)$' ||
        say "note: VM $vmid's ostype isn't win10 or win11, and Proxmox gives Windows its Hyper-V flags by that."
    printf '%s\n' "$config" | grep -Eq '^cpu: host' || say "note: VM $vmid's cpu isn't host, and only host has run."
    case $(printf '%s\n' "$config" | sed -n 's/^args: //p') in
        *[\'\"\\]*) die "VM $vmid's args: has quotes or backslashes, so this can't tell its words apart. Edit it by hand." ;;
    esac
}

# The lowest of 1 to 3 that no other VM's file names.
free_slot() {
    for n in 1 2 3; do
        taken=''
        for file in "$CONF_DIR"/*.conf; do
            [ -f "$file" ] && [ "$file" != "$CONF_DIR/$vmid.conf" ] || continue
            grep -qx "slot=$n" "$file" && taken=1
        done
        [ -n "$taken" ] || {
            say "$n"
            return
        }
    done
    die "slots 1 to 3 are taken. Name one with --slot."
}

# The GPU's words from the printed script, with this VM's run folder and memory files in place of the printed ones.
make_words() {
    printed_run=$(printf '%s\n' "$printed" | sed -n 's|^mkdir -p \(/.*/guesthouse/run[-0-9]*\)$|\1|p' | head -n 1)
    printed_ram_file=$(printf '%s\n' "$printed" | sed -n 's/^ *GUESTHOUSE_RAM_FILE=\([^ ]*\) \\$/\1/p')
    printed_window_file=$(printf '%s\n' "$printed" | sed -n 's/^ *GUESTHOUSE_WINDOW_FILE=\([^ ]*\) \\$/\1/p')
    printed_ram=$(printf '%s\n' "$printed" | sed -n 's/^ *GUESTHOUSE_RAM=\([^ ]*\) \\$/\1/p')
    # They are only looked for in the words, as text. Each has to be what Guesthouse prints, and nothing else.
    shape "$printed_run" "/run/user/$uid/guesthouse/run(-[0-9]+)?" && shape "$printed_ram_file" "(/[A-Za-z0-9_.-]+)+" &&
        shape "$printed_window_file" "(/[A-Za-z0-9_.-]+)+" && shape "$printed_ram" "[1-9][0-9]*G" ||
        die "guesthouse-vm command printed a script this doesn't know. Its version may be newer than this script."
    ram=${ram:-$printed_ram}
    case $ram in
        [1-9]*G) ;;
        *) die "the RAM is a number of GiB, like 8G, not $ram." ;;
    esac
    case ${ram%G} in
        *[!0-9]*) die "the RAM is a number of GiB, like 8G, not $ram." ;;
    esac
    memory=$((${ram%G} * 1024))
    case $slot in
        0) run=/run/user/$uid/guesthouse/run ;;
        *) run=/run/user/$uid/guesthouse/run-$slot ;;
    esac
    gpu_words=$(printf '%s\n' "$printed" | qemu_words | while IFS= read -r word; do
        if is_gpu_word "$word"; then
            printf '%s\n' "$word"
        fi
    done)
    printf '%s\n' "$gpu_words" | grep -q '^-device gpupv-render,' ||
        die "guesthouse-vm command $machine printed no GPU words. Its notes, from setup-vm.sh --dry-run's run of it, may say why."
    gpu_words=$(printf '%s\n' "$gpu_words" | while IFS= read -r word; do
        word=$(swap_first "$word" "path=$printed_run/" "path=$run/")
        word=$(swap_first "$word" "mem-path=$printed_ram_file," "mem-path=/dev/shm/gpupv-ram.$vmid,")
        word=$(swap_first "$word" "mem-path=$printed_window_file," "mem-path=/dev/shm/gpupv-window.$vmid,")
        case $word in
            "-object memory-backend-file,id=ram0,size="*)
                rest=${word#-object memory-backend-file,id=ram0,size=}
                word="-object memory-backend-file,id=ram0,size=$ram,${rest#*,}"
                ;;
            "-device gpupv-render,"*)
                [ "$render_address" = none ] || word="$word,bus=pcie.0,addr=$render_address"
                ;;
            # QEMU runs as root here, so each channel only takes a daemon that runs as the user.
            "-device gpupv-global,"* | "-device gpupv-vgpu,"*)
                word="$word,owner=$uid"
                ;;
        esac
        printf '%s\n' "$word"
    done)
    check_gpu_words
    # The daemon's words for the card, and any others, except those the hookscript sets for this VM.
    # Root reads them at each start, so only plain words, and the script stops at anything else.
    daemon_env=''
    daemon_lines=$(printf '%s\n' "$printed" | sed -n '/^# The daemon\./,/^# QEMU connects/ s/^ *\(GUESTHOUSE_.*\) \\$/\1/p')
    while IFS= read -r word; do
        [ -n "$word" ] || continue
        printf '%s\n' "$word" | grep -Eqx 'GUESTHOUSE_[A-Z0-9_]+=[A-Za-z0-9_@%+=:,./-]*' ||
            die "the daemon's environment has $word, which this can't pass on, as it has quotes or a space."
        case $word in
            GUESTHOUSE_RAM=* | GUESTHOUSE_RAM_FILE=* | GUESTHOUSE_WINDOW_FILE=* | GUESTHOUSE_SLOT=*) ;;
            *) daemon_env="${daemon_env:+$daemon_env }$word" ;;
        esac
    done <<EOF
$daemon_lines
EOF
}

# QEMU runs these as root, so each has to be exactly the shape Guesthouse prints today, with this VM's paths.
check_gpu_words() {
    hex='0x[0-9a-f]+'
    id='[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}'
    while IFS= read -r word; do
        printf '%s\n' "$word" | grep -Eqx \
            -e "-device vmbus-bridge,mmio-base=$hex,mmio-size=$hex" \
            -e "-device hv-timesync" \
            -e "-chardev socket,id=gpg,path=$run/global\\.sock,reconnect-ms=[0-9]+" \
            -e "-chardev socket,id=gpv,path=$run/vgpu\\.sock,reconnect-ms=[0-9]+" \
            -e "-object memory-backend-file,id=gpio,size=64G,mem-path=/dev/shm/gpupv-window\\.$vmid,share=on" \
            -e "-device gpupv-global,chardev=gpg,instanceid=$id,iomem=gpio,iomem-base=$hex,owner=$uid" \
            -e "-device gpupv-vgpu,chardev=gpv,instanceid=$id,owner=$uid" \
            -e "-object memory-backend-file,id=ram0,size=${ram%G}G,mem-path=/dev/shm/gpupv-ram\\.$vmid,share=on" \
            -e "-machine memory-backend=ram0" \
            -e "-device gpupv-render,channel=$id,hostcaps=[0-9]+(,vendor-id=$hex,device-id=$hex)?(,bus=pcie\\.0,addr=$hex)?" ||
            die "guesthouse-vm command printed $word, which this doesn't know. Guesthouse may be newer than this script."
    done <<EOF
$gpu_words
EOF
    [ "$(printf '%s\n' "$gpu_words" | wc -l)" -eq 10 ] ||
        die "guesthouse-vm command printed other GPU words than the ten this knows. Guesthouse may be newer than this script."
}

write_conf() {
    conf_text="# Written by Guesthouse's packaging/proxmox/setup-vm.sh for VM $vmid, from the machine $machine.
# The hookscript reads it. Run setup-vm.sh again rather than changing it, as args: has to agree.
user=$user
slot=$slot
ram=$ram
daemon_env=$daemon_env"
}

# The GPU's words first, in the printed order, and then whatever else args: had.
make_args() {
    rest=''
    set -f
    # shellcheck disable=SC2046
    set -- $(printf '%s\n' "$config" | sed -n 's/^args: //p')
    set +f
    while [ $# -gt 0 ]; do
        if [ $# -ge 2 ] && is_gpu_word "$1 $2"; then
            shift 2
            continue
        fi
        rest="$rest $1"
        shift
    done
    args=$(printf '%s\n' "$gpu_words" | tr '\n' ' ' | sed 's/ $//')$rest
}

main "$@"
