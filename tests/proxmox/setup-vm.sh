#!/bin/bash
# Tests the functions of packaging/proxmox/setup-vm.sh that read what guesthouse-vm prints, on made-up
# printed scripts, with no root and no Proxmox. The script is read in without its last line, which runs main.
#
# Usage: tests/proxmox/setup-vm.sh. SHELL_UNDER_TEST names the shell, sh if not, as Proxmox's sh is dash.
set -u

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$HERE/../.." && pwd)
SCRIPT=$REPO/packaging/proxmox/setup-vm.sh
SHELL_UNDER_TEST=${SHELL_UNDER_TEST:-sh}
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

passed=0
failed=0

# The functions only.
grep -v '^main "\$@"$' "$SCRIPT" >"$T/functions.sh"

# A printed script as Guesthouse makes it for slot 1 of the user with uid 1000, but for the three values
# the tests change: the run folder, and the RAM file and the window file.
printed_script() {
    cat <<EOS
#!/bin/sh
mkdir -p $1
# The daemon.
GUESTHOUSE_RAM_FILE=$2 \\
GUESTHOUSE_WINDOW_FILE=$3 \\
GUESTHOUSE_RAM=8G \\
GUESTHOUSE_AMD_CARD=0000:c4:00.0 \\
  guesthouse-daemon
# QEMU connects to both sockets, so the daemon starts first.
/usr/bin/qemu-system-x86_64 \\
  -device vmbus-bridge,mmio-base=0x10000000000,mmio-size=0x1000000000 \\
  -device hv-timesync \\
  -chardev socket,id=gpg,path=$1/global.sock,reconnect-ms=500 \\
  -object memory-backend-file,id=gpio,size=64G,mem-path=$3,share=on \\
  -device gpupv-global,chardev=gpg,instanceid=711dad3a-73ce-468b-90a9-ede6906841b2,iomem=gpio,iomem-base=0x10000000000 \\
  -chardev socket,id=gpv,path=$1/vgpu.sock,reconnect-ms=500 \\
  -device gpupv-vgpu,chardev=gpv,instanceid=7a1e2f30-4c55-4d6e-9f10-0123456789ab \\
  -object memory-backend-file,id=ram0,size=8G,mem-path=$2,share=on \\
  -machine memory-backend=ram0 \\
  -device gpupv-render,channel=7a1e2f30-4c55-4d6e-9f10-0123456789ab,hostcaps=1
EOS
}

# Runs make_words on the printed script PRINTED in a shell of its own, as setup-vm.sh's main would have it,
# and says what it printed. $status is the shell's.
make_words_of() {
    status=0
    "$SHELL_UNDER_TEST" -c '
        . "$1"
        printed=$2 uid=1000 slot=1 vmid=101 ram='' render_address=0x6
        make_words
        printf "%s\n" "$gpu_words"
        printf "daemon_env=%s\n" "$daemon_env"
    ' sh "$T/functions.sh" "$1" >"$T/out" 2>&1 || status=$?
}

# What a value, if it were run as a command of sed's, would do.
printf '#!/bin/sh\n: >"%s/marker"\n' "$T" >"$T/make-marker"
chmod +x "$T/make-marker"

check() {
    local name=$1
    shift
    rm -f "$T"/marker*
    printf '%s\n' "$name"
    if "$@"; then
        passed=$((passed + 1))
    else
        failed=$((failed + 1))
        sed 's/^/    | /' "$T/out" 2>/dev/null
    fi
}

expect() {
    printf '  FAILED: %s\n' "$*"
    return 1
}

test_a_script_as_guesthouse_prints_it_gives_this_vms_words() {
    make_words_of "$(printed_script /run/user/1000/guesthouse/run /dev/shm/gpupv-ram /dev/shm/gpupv-window)"
    [ "$status" -eq 0 ] || { expect "exit $status, not 0"; return 1; }
    grep -qx -e '-chardev socket,id=gpg,path=/run/user/1000/guesthouse/run-1/global.sock,reconnect-ms=500' "$T/out" ||
        { expect "the run folder isn't this slot's"; return 1; }
    grep -qx -e '-object memory-backend-file,id=gpio,size=64G,mem-path=/dev/shm/gpupv-window.101,share=on' "$T/out" ||
        { expect "the window file isn't this VM's"; return 1; }
    grep -qx -e '-object memory-backend-file,id=ram0,size=8G,mem-path=/dev/shm/gpupv-ram.101,share=on' "$T/out" ||
        { expect "the RAM file isn't this VM's"; return 1; }
    grep -qx -e '-device gpupv-render,channel=7a1e2f30-4c55-4d6e-9f10-0123456789ab,hostcaps=1,bus=pcie.0,addr=0x6' "$T/out" ||
        { expect "the render device has no address"; return 1; }
    grep -qx 'daemon_env=GUESTHOUSE_AMD_CARD=0000:c4:00.0' "$T/out" || { expect "the daemon's environment is wrong"; return 1; }
    grep -qx -e '-device gpupv-vgpu,chardev=gpv,instanceid=7a1e2f30-4c55-4d6e-9f10-0123456789ab,owner=1000' "$T/out" ||
        { expect "the vGPU channel doesn't name its owner"; return 1; }
    grep -qx -e '-device gpupv-global,chardev=gpg,instanceid=711dad3a-73ce-468b-90a9-ede6906841b2,iomem=gpio,iomem-base=0x10000000000,owner=1000' "$T/out" ||
        { expect "the global channel doesn't name its owner"; return 1; }
}

# Each of the three values with something in it that is no part of a path, which would end a pattern of sed's
# and begin a command of its own.
test_a_run_folder_with_more_in_it_is_refused() {
    make_words_of "$(printed_script "/run/user/1000/guesthouse/run|x|;e$T/make-marker;#/guesthouse/run" /dev/shm/r /dev/shm/w)"
    refused_and_nothing_ran
}

test_a_ram_file_with_more_in_it_is_refused() {
    make_words_of "$(printed_script /run/user/1000/guesthouse/run "/dev/shm/r|x|;e$T/make-marker;#" /dev/shm/w)"
    refused_and_nothing_ran
}

test_a_window_file_with_more_in_it_is_refused() {
    make_words_of "$(printed_script /run/user/1000/guesthouse/run /dev/shm/r "/dev/shm/w|x|;e$T/make-marker;#")"
    refused_and_nothing_ran
}

test_another_users_run_folder_is_refused() {
    make_words_of "$(printed_script /run/user/0/guesthouse/run /dev/shm/r /dev/shm/w)"
    [ "$status" -ne 0 ] || { expect "it took a run folder of another user"; return 1; }
}

refused_and_nothing_ran() {
    [ ! -e "$T/marker" ] || { expect "the value was run as a command"; return 1; }
    [ "$status" -ne 0 ] || { expect "it took the value"; return 1; }
    grep -q "printed a script this doesn't know" "$T/out" || { expect "it didn't say why"; return 1; }
}

# The user's program gets a session of its own, so root's terminal isn't its terminal, and nothing to read.
test_the_users_program_has_a_session_and_input_of_its_own() {
    mkdir -p "$T/bin"
    printf '#!/bin/sh\nshift 3\nexec "$@"\n' >"$T/bin/runuser"
    cat >"$T/bin/program" <<'EOS'
#!/bin/sh
# Its session, and whether anything is there to read.
sed 's/^[^)]*) //' /proc/self/stat | cut -d' ' -f4 | tr -d '\n'
printf ' '
[ -t 0 ] && echo terminal || cat | wc -c
EOS
    chmod +x "$T/bin/runuser" "$T/bin/program"
    status=0
    : >"$T/out"
    mine=$(sed 's/^[^)]*) //' /proc/$$/stat | cut -d' ' -f4)
    PATH="$T/bin:$PATH" "$SHELL_UNDER_TEST" -c '. "$1"; as_user someone "$2"' sh "$T/functions.sh" "$T/bin/program" \
        >"$T/out" 2>&1 <<<"input" || status=$?
    read -r session input <"$T/out"
    [ "$status" -eq 0 ] || { expect "exit $status, not 0"; return 1; }
    [ -n "$session" ] && [ "$session" != "$mine" ] || { expect "it ran in this session, $mine"; return 1; }
    [ "$input" = 0 ] || { expect "it had input to read"; return 1; }
}

for name in $(declare -F | awk '{ print $3 }' | grep '^test_'); do
    check "$name" "$name"
done
printf '\n%s passed, %s failed\n' "$passed" "$failed"
[ "$failed" -eq 0 ]
