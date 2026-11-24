#!/bin/sh
# Run by guesthouse-shutdown.service for the whole life of the guest. HCS can't ask a Linux guest to
# shut down itself, as that takes Hyper-V's integration services, which a machine HCS makes doesn't
# have. So the host writes a line on the guest's serial port, and this waits for it, as report.ps1
# does in a Windows guest, and powers the machine off.
set -u
port=/dev/ttyS0
ask="guesthouse: shutdown"

# The port keeps the settings it was given when this stops, and other scripts write to it, so only
# what this needs is changed, and nothing about its output. -echo so the host's line isn't sent back
# to it as if the guest had said it. clocal so the host closing its end can't hang this up: opening
# the port may make it this service's controlling terminal, and a hangup would then kill it. The
# port stays in line mode, so each read is a whole line, with the host's newline as its end.
# guesthouse-shutdown.service masks any serial getty on it, so nothing else reads the line.
stty -F "$port" -echo clocal 2> /dev/null

while IFS= read -r line; do
    # A CR ends a line too if the host or the port's settings send one, and spaces don't count.
    line=$(printf '%s' "$line" | tr -d '\r' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
    if [ "$line" = "$ask" ]; then
        printf 'guesthouse: shutting down, as the host asked\n' > "$port"
        exec systemctl poweroff
    fi
done < "$port"
# The port closed under it, which nothing should do. A non-zero end has systemd start it again.
exit 1
