#!/bin/sh
# SPDX-License-Identifier: MIT
#
# Checks of the Windows graphics driver's sources that need no Windows kit.
#
# 1. After the preprocessor, no source of the user-mode driver, winemetal or
#    the test programs names the C:\ruxnlog folder: no build writes there.
# 2. The ALLOCATIONS escape asks caller_is_admin() before it fills its
#    answer.
#
# Usage: check-driver-sources.sh
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
drv=$root/guest/drivers/windows-pvg
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail=0

# Headers the sources include that are not in the repo (Windows', the C
# library's) become empty files, so that the preprocessor can run without
# the kit. Only the text of the sources' own code is looked at.
sources=$(find "$drv/ruxnwddm" "$drv/dxmt-win" -name '*.c' -o -name '*.cpp' | sort)

mkdir "$work/stubs"
headers=$(find "$drv/ruxnwddm" "$drv/dxmt-win" -name '*.h' | sort)
# shellcheck disable=SC2086
grep -h '^[[:space:]]*#[[:space:]]*include' $sources $headers |
	sed 's/^[^<"]*[<"]\([^>"]*\)[>"].*/\1/' | sort -u | while read -r header; do
	[ -n "$header" ] || continue
	mkdir -p "$work/stubs/$(dirname "$header")"
	[ -e "$work/stubs/$header" ] || : > "$work/stubs/$header"
done

for file in $sources; do
	if ! cc -E -P -x c -nostdinc -I "$drv/ruxnwddm" -I "$drv/dxmt-win" -I "$work/stubs" \
		"$file" > "$work/pre.txt" 2> "$work/err.txt"; then
		echo "FAIL: cannot preprocess ${file#"$root"/}"
		head -3 "$work/err.txt"
		fail=1
	elif grep -qi 'ruxnlog' "$work/pre.txt"; then
		echo "FAIL: ${file#"$root"/} still names C:\\ruxnlog"
		fail=1
	fi
done

# The case's lines, up to the next case.
body=$(awk '/case RUXNWDDM_ESCAPE_ALLOCATIONS:/ {on = 1; next} on && /case RUXNWDDM_ESCAPE_/ {exit} on' \
	"$drv/ruxnwddm/escape.c")
admin=$(printf '%s\n' "$body" | grep -n 'caller_is_admin' | head -1 | cut -d: -f1)
fill=$(printf '%s\n' "$body" | grep -n 'ruxnwddm_alloc_list_fill' | head -1 | cut -d: -f1)
if [ -z "$admin" ] || [ -z "$fill" ] || [ "$admin" -ge "$fill" ]; then
	echo "FAIL: the ALLOCATIONS escape does not check for an administrator before it answers"
	fail=1
fi

[ "$fail" = 0 ] && echo "ok"
exit "$fail"
