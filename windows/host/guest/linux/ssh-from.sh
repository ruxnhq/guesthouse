#!/bin/sh
# Prints the sshd settings that take a password only from this PC: the loopback, where the host's
# relay over a Hyper-V socket arrives, and this PC's addresses from the file the host writes before
# each start, as it does for a Windows guest: "shared" or "bridged" on its first line, then this PC's
# addresses, a line each. On the shared network the guest's default gateways, given after the file,
# are this PC too, and that is the address its port forwards come from. From any other address sshd
# takes keys only, so a bridged guest is still reached by key from other devices. With no file, a
# network it doesn't know, or no address, passwords are taken from the loopback alone.
# Usage: ssh-from.sh HOSTS_FILE [GATEWAY...]
set -u
file=$1
shift
allowed="127.0.0.1,::1"
ipv4() {
    printf '%s\n' "$1" | grep -Eqx '[0-9]{1,3}(\.[0-9]{1,3}){3}'
}
if [ -f "$file" ]; then
    network=$(tr -d '\r' < "$file" | sed -n '1{s/[[:space:]]//g;p;}')
    case "$network" in
        shared | bridged)
            for address in $(tr -d '\r' < "$file" | tail -n +2); do
                if ipv4 "$address"; then allowed="$allowed,$address"; fi
            done
            if [ "$network" = shared ]; then
                for address in "$@"; do
                    if ipv4 "$address" && [ "$address" != 0.0.0.0 ]; then allowed="$allowed,$address"; fi
                done
            fi
            ;;
    esac
fi
printf '# Written by Guesthouse at every start: passwords from this PC alone.\n'
printf 'Match Address *,!%s\n' "$(printf '%s' "$allowed" | sed 's/,/,!/g')"
printf '    PasswordAuthentication no\n'
printf '    KbdInteractiveAuthentication no\n'
