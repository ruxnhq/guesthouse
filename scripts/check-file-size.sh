#!/bin/sh
# Fails when a change pushes a source file past the size limit, or makes a file
# that is already past it longer. Files over the limit before the rule came in
# are left alone until someone changes them. CONTRIBUTING.md has the rule.
#
# Usage: scripts/check-file-size.sh BASE   (BASE is the commit to compare with)
set -eu

LIMIT=1000
base=${1:?usage: check-file-size.sh BASE}

# Code we don't write, or keep as one piece: other people's code, lock files,
# patches against QEMU, licence texts, and the installer people fetch as one file.
exempt() {
	case $1 in
	vendor/* | *.lock | *package-lock.json | *.patch | *LICENSE* | *License* | install.sh) return 0 ;;
	esac
	return 1
}

lines_at() {
	git show "$1:$2" 2>/dev/null | wc -l | tr -d ' '
}

report=$(mktemp)
trap 'rm -f "$report"' EXIT

# A moved file counts as a new one, so its size is checked like any other.
# --numstat prints "-" for binary files; those are skipped.
git diff --numstat --no-renames --diff-filter=AM "$base" HEAD |
	while IFS="$(printf '\t')" read -r added _ path; do
		[ "$added" = "-" ] && continue
		exempt "$path" && continue
		now=$(lines_at HEAD "$path")
		[ "$now" -le "$LIMIT" ] && continue
		before=$(lines_at "$base" "$path")
		if [ "$before" -le "$LIMIT" ]; then
			echo "$path: $now lines, past the limit of $LIMIT. Split it by job."
		elif [ "$now" -gt "$before" ]; then
			echo "$path: was $before lines, now $now. It's past $LIMIT already, so split it rather than add to it."
		fi
	done >"$report"

if [ -s "$report" ]; then
	cat "$report"
	exit 1
fi
echo "No file goes past $LIMIT lines."
