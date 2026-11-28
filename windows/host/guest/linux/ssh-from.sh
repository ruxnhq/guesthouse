#!/bin/sh
# Prints the sshd settings that take a password only from this PC: the loopback, where the host's
# relay over a Hyper-V socket arrives, and this PC's addresses from the file the host writes before
# each start, as it does for a Windows guest: "shared" or "bridged" on its first line, then this PC's
# addresses, a line each. On the shared network the guest's default gateways are this PC too, and
# that is the address its port forwards come from. A bridged guest is on this PC's own network, and
# other devices there reach it directly, so it takes a password from its own networks too. From any
# other address sshd takes keys only. With no file, a network it doesn't know, or no address,
# passwords are taken from the loopback alone, and bridged from its own networks too.
# Usage: ssh-from.sh HOSTS_FILE "GATEWAY..." "NETWORK..."   (networks as 192.168.1.0/24)
set -u
file=$1
gateways=${2-}
networks=${3-}
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
                for address in $gateways; do
                    if ipv4 "$address" && [ "$address" != 0.0.0.0 ]; then allowed="$allowed,$address"; fi
                done
            else
                for net in $networks; do
                    prefix=${net##*/}
                    if [ "$prefix" != "$net" ] && ipv4 "${net%/*}" && printf '%s\n' "$prefix" | grep -Eqx '[0-9]{1,2}' &&
                        [ "$prefix" -ge 8 ] && [ "$prefix" -le 32 ]; then
                        allowed="$allowed,$net"
                    fi
                done
            fi
            ;;
    esac
fi
printf '# Written by Guesthouse at every start: passwords from this PC, and bridged its network.\n'
printf 'Match Address *,!%s\n' "$(printf '%s' "$allowed" | sed 's/,/,!/g')"
printf '    PasswordAuthentication no\n'
printf '    KbdInteractiveAuthentication no\n'
