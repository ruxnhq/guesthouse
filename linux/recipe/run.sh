#!/bin/sh
# Runs one step of a recipe in an Ubuntu guest, as root. The host copies this and the job into
# /var/lib/guesthouse-recipe and runs it with one word: "ready", "prepare", or "run" and a step's number.
#
# The job is data. A line of it is only ever read into variables: a package name is one argument after
# "--", and a script comes in as Base64 and is decoded into a file of its own, which bash runs.
set -eu
# Each of the first three can be set by a test, which has no guest.
folder=${GUESTHOUSE_RECIPE_FOLDER:-/var/lib/guesthouse-recipe}
marker=${GUESTHOUSE_FIRST_BOOT_MARKER:-/var/lib/nvfwd/first-boot-done}
update_pause=${GUESTHOUSE_UPDATE_PAUSE:-10}
job=$folder/job
# How long apt waits for another program that holds its lock, such as an upgrade.
lock_seconds=1800
update_tries=30
export DEBIAN_FRONTEND=noninteractive

# The first boot installs the desktop and then restarts the guest, so it is ready when its marker is
# older than this boot and no restart is on its way. A step started before then would be cut off.
# Nor while the GPU step of this boot is waiting or running: a step that installs GPU software, such
# as CUDA, must find it done, with the GPU on or off. A guest with no GPU step has no job for it.
ready() {
    [ -f "$marker" ] || exit 1
    booted=$(($(date +%s) - $(cut -d. -f1 /proc/uptime)))
    [ "$(stat -c %Y "$marker")" -lt "$booted" ] || exit 1
    if systemctl list-jobs 2>/dev/null | grep -Eq 'reboot|shutdown|poweroff|nvfwd-gpu\.service'; then
        exit 1
    fi
}

# Another program can hold the lists for a while, so a failed update is tried again.
prepare() {
    tries=0
    until apt-get update; do
        tries=$((tries + 1))
        [ "$tries" -lt "$update_tries" ] || exit 1
        sleep "$update_pause"
    done
}

install_package() {
    case $1 in
        '' | -* | *[!a-z0-9.+-]*)
            printf '%s is not a package name.\n' "$1" >&2
            exit 2
            ;;
    esac
    apt-get install -y -o DPkg::Lock::Timeout="$lock_seconds" -- "$1"
}

run_script() {
    file=$(mktemp "$folder/script.XXXXXX")
    printf '%s' "$1" | base64 -d >"$file"
    status=0
    bash "$file" || status=$?
    rm -f "$file"
    exit "$status"
}

step() {
    case $1 in
        '' | *[!0-9]*)
            echo "A step is a number." >&2
            exit 2
            ;;
    esac
    found=
    while IFS=' ' read -r word index kind rest; do
        if [ "$word" = step ] && [ "$index" = "$1" ]; then
            found=yes
            break
        fi
    done <"$job"
    if [ -z "$found" ]; then
        echo "The job has no step $1." >&2
        exit 2
    fi
    case $kind in
        apt) install_package "$rest" ;;
        script) run_script "$rest" ;;
        *)
            echo "The job's step $1 is of a kind this has not got." >&2
            exit 2
            ;;
    esac
}

case ${1:-} in
    ready) ready ;;
    prepare) prepare ;;
    run) step "${2:-}" ;;
    *)
        echo "usage: run.sh ready | prepare | run NUMBER" >&2
        exit 2
        ;;
esac
