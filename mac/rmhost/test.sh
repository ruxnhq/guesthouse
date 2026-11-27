#!/bin/sh
# SPDX-License-Identifier: MIT
# Tests rmhost with no guest and no graphics device: a stand-in publisher, testsource, publishes a moving
# picture the way the device does; a reader (`rmhost stats`) must receive frames from it, and the
# attacker, a process that forges notices and messages while readers read, must not be able to
# drop a reader or free a name. It needs a Mac with Metal, and about 40 seconds.
#
#   sh mac/rmhost/test.sh [FOLDER]     FOLDER is where build.sh put the programs
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-$here/../build/rmhost}
[ -x "$out/rmhost" ] && [ -x "$out/testsource" ] && [ -x "$out/attacker" ] || sh "$here/build.sh" --out "$out"

echo "reader against testsource"
"$out/testsource" -t 12 >/dev/null 2>&1 &
publisher=$!
sleep 2
log=$(mktemp)
trap 'kill "$publisher" 2>/dev/null || true; rm -f "$log"' EXIT INT TERM HUP
"$out/rmhost" stats 5 >"$log" 2>&1 || { cat "$log"; exit 1; }
cat "$log"
received=$(sed -n 's/.*: \([0-9][0-9]*\) frames received.*/\1/p' "$log" | head -n 1)
[ -n "$received" ] && [ "$received" -gt 100 ] || { echo "FAIL: the reader got no frames" >&2; exit 1; }
echo "ok: the reader received $received frames"
wait "$publisher" 2>/dev/null || true

echo "attacker against testsource"
"$out/attacker" -s "$out/testsource"
echo "ok: the attacker found nothing to drop"
