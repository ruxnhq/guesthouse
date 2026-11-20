#!/bin/sh
# Runs the jail's parts by hand with the release's own programs, as the app puts them together for a
# GPU machine's QEMU, and says what came of each on a line starting with "result". It needs
# PROGRAM (the program folder), CODE and VARS_TEMPLATE (UEFI firmware), and PATH with qemu-img and bwrap.
# It runs as the user the machine would belong to, with /dev/kvm.
set -u

JAIL=$PROGRAM/guesthouse-jail
QEMU=$PROGRAM/qemu/build/qemu-system-x86_64
WORK=$(mktemp -d "$XDG_RUNTIME_DIR/jail-check.XXXXXX")
RUN=$WORK/run
mkdir -p "$RUN"
cp "$VARS_TEMPLATE" "$WORK/vars.fd"
chmod u+w "$WORK/vars.fd"
qemu-img create -f qcow2 "$WORK/disk.qcow2" 1M >/dev/null

result() {
    printf 'result %s %s\n' "$1" "$2"
}

# What qemu_jail.rs grants a machine's QEMU, in the order it does: the system's programs, the settings
# QEMU reads, its own folder, the firmware, the devices, the disk and the run folder.
GRANTS="--run /usr --run /nix/store --run /run/current-system/sw --run /run/opengl-driver
 --read /etc/ld.so.cache --read /etc/static --read /etc/nsswitch.conf --read /etc/passwd --read /etc/group
 --read /etc/hosts --read /etc/resolv.conf --read /etc/qemu --read /run/systemd/resolve --read /sys --read /proc
 --run $PROGRAM/qemu --read $PROGRAM/qemu/pc-bios --read $CODE --write $WORK/vars.fd
 --device /dev/kvm --device /dev/null --device /dev/zero --device /dev/random --device /dev/urandom
 --write $WORK/disk.qcow2 --own $RUN
 --tcp-bind none --tcp-connect none --udp-bind none --udp-send none"
# The same as the app had before NixOS: only /usr for programs, and no store.
OLD_GRANTS="--run /usr
 --read /etc/ld.so.cache --read /etc/nsswitch.conf --read /etc/passwd --read /etc/group
 --read /etc/hosts --read /etc/resolv.conf --read /etc/qemu --read /run/systemd/resolve --read /sys --read /proc
 --run $PROGRAM/qemu --read $PROGRAM/qemu/pc-bios --read $CODE --write $WORK/vars.fd
 --device /dev/kvm --device /dev/null --device /dev/zero --device /dev/random --device /dev/urandom
 --write $WORK/disk.qcow2 --own $RUN
 --tcp-bind none --tcp-connect none --udp-bind none --udp-send none"

# A small machine that runs the firmware and waits.
QEMU_WORDS="-L $PROGRAM/qemu/pc-bios -sandbox on,obsolete=deny,spawn=allow,resourcecontrol=deny -accel kvm -machine q35,smm=on -cpu host
 -m 128 -smp 1 -display none -nodefaults -monitor none -serial none
 -global driver=cfi.pflash01,property=secure,value=on
 -drive if=pflash,format=raw,unit=0,readonly=on,file=$CODE
 -drive if=pflash,format=raw,unit=1,file=$WORK/vars.fd
 -drive if=none,id=disk,file=$WORK/disk.qcow2 -device virtio-blk-pci,drive=disk"

# NAMESPACES of qemu_jail.rs, and a bind for each path of the plan, as namespace_words makes them.
namespace_words() { # the grants
    echo "--unshare-user --unshare-ipc --unshare-uts --unshare-cgroup-try --tmpfs /tmp --ro-bind /usr /usr
 --symlink usr/lib /lib --symlink usr/lib64 /lib64 --symlink usr/bin /bin --symlink usr/sbin /sbin
 --proc /proc --dev /dev --ro-bind /sys /sys --ro-bind $JAIL $JAIL"
    set -- $1
    while [ $# -ge 2 ]; do
        grant=$1 path=$2
        shift 2
        case $path in /usr | /usr/* | /proc | /proc/* | /sys | /sys/*) continue ;; esac
        case $grant in
            --read | --run) echo "--ro-bind-try $path $path" ;;
            --write | --own | --socket) echo "--bind-try $path $path" ;;
            --device) echo "--dev-bind-try $path $path" ;;
        esac
    done
}

# Runs a machine for a few seconds, looks at its process, and lets the timeout end it. A machine that
# was running comes out as 124, which is what timeout answers when it had to kill.
try_machine() { # name, how: landlock or namespaces, grants
    name=$1 how=$2 grants=$3
    log=$WORK/$name.log
    if [ "$how" = namespaces ]; then
        # shellcheck disable=SC2046
        timeout 12 bwrap $(namespace_words "$grants") -- $JAIL $grants -- $QEMU $QEMU_WORDS >"$log" 2>&1 &
    else
        # shellcheck disable=SC2086
        timeout 12 $JAIL $grants -- $QEMU $QEMU_WORDS >"$log" 2>&1 &
    fi
    timeout_pid=$!
    sleep 6
    pid=$(pgrep -u "$(id -u)" -x qemu-system-x86 | head -n 1)
    if [ -n "$pid" ]; then
        status=$(cat "/proc/$pid/status")
        nonewprivs=$(echo "$status" | sed -n 's/^NoNewPrivs:[[:space:]]*//p')
        seccomp=$(echo "$status" | sed -n 's/^Seccomp:[[:space:]]*//p')
        user_ns=different
        [ "$(readlink "/proc/$pid/ns/user")" != "$(readlink /proc/self/ns/user)" ] || user_ns=same
        result "$name.process" "running NoNewPrivs=$nonewprivs Seccomp=$seccomp user_ns=$user_ns"
    else
        result "$name.process" none
    fi
    wait "$timeout_pid"
    result "$name.exit" "$?"
    result "$name.log" "$(tr '\n' '|' <"$log" | cut -c1-600)"
    pkill -u "$(id -u)" -x qemu-system-x86
}

# The namespace probe as sandbox_question() in forwarding.rs makes it, and the one before it. true is
# looked for as the app looks, on its PATH, which starts with the package's tools, where it's a link to
# coreutils. The shell's own true would hide that. It's in the store, which is bound already.
TRUE=$(IFS=:; for folder in $PATH; do [ -x "$folder/true" ] && echo "$folder/true" && break; done)
bwrap --unshare-user --unshare-ipc --unshare-uts \
    --ro-bind-try /usr /usr --ro-bind-try /nix/store /nix/store \
    --ro-bind-try /run/current-system/sw /run/current-system/sw \
    --ro-bind-try /run/opengl-driver /run/opengl-driver \
    --symlink usr/lib64 /lib64 -- "$TRUE" >"$WORK/probe.log" 2>&1
result probe.new "$? $(tr '\n' '|' <"$WORK/probe.log")"
bwrap --unshare-user --unshare-ipc --unshare-uts --ro-bind /usr /usr --symlink usr/lib64 /lib64 \
    -- /usr/bin/true >"$WORK/probe.log" 2>&1
result probe.old "$? $(tr '\n' '|' <"$WORK/probe.log")"

try_machine landlock landlock "$GRANTS"
try_machine namespaces namespaces "$GRANTS"
try_machine old.landlock landlock "$OLD_GRANTS"
try_machine old.namespaces namespaces "$OLD_GRANTS"
exit 0
