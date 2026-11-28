#!/bin/sh
# SPDX-License-Identifier: MIT
#
# Checks of the Linux graphics driver's sources that need no kernel.
#
# 1. A place whose unmap did not go through keeps its buffer object: the
#    failure branch of pvg_placement_put() returns before it lets go of it.
# 2. The buffer objects of the places left on a task's stale list are let go
#    when the task is freed.
#
# Usage: check-driver-sources.sh
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
file=$root/guest/drivers/linux-pvg/driver/pvg_task.c
fail=0

# The lines of a function: from its opening line to the closing brace at column 0.
body() {
	awk -v start="$1" 'index($0, start) == 1 {on = 1} on {print} on && /^}/ {exit}' "$file"
}

put=$(body 'static void pvg_placement_put(')
stale=$(printf '%s\n' "$put" | grep -n 'list_add(&placement->stale' | head -1 | cut -d: -f1)
ret=$(printf '%s\n' "$put" | awk -v from="${stale:-0}" 'NR > from && /return;/ {print NR; exit}')
release=$(printf '%s\n' "$put" | grep -n 'drm_gem_object_put(&bo->base)' | head -1 | cut -d: -f1)
if [ -z "$stale" ] || [ -z "$ret" ] || [ -z "$release" ] || [ "$ret" -ge "$release" ]; then
	echo "FAIL: a place that was not unmapped lets go of its buffer object"
	fail=1
fi

if ! body 'static void pvg_task_free(' | grep -q 'drm_gem_object_put(&placement->bo->base)'; then
	echo "FAIL: freeing a task does not let go of the buffer objects of its stale places"
	fail=1
fi

[ "$fail" = 0 ] && echo "ok"
exit "$fail"
