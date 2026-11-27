#!/usr/bin/env bash
# Sets the version a release needs, in the files RELEASING.md step 1 names and nothing else.
#   scripts/release-bump.sh linux   X.Y.Z   Cargo.toml's workspace version, and Cargo.lock to follow
#   scripts/release-bump.sh windows X.Y.Z   windows/version.txt
# Run from the repo root, or anywhere inside it. release.yml calls this.
set -euo pipefail

HOST=${1:-}
VERSION=${2:-}
case "$HOST" in linux | windows) ;; *) echo "usage: $0 linux|windows X.Y.Z" >&2; exit 2 ;; esac
if ! [[ "$VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
  echo "'$VERSION' isn't a version like 1.2.3." >&2
  exit 2
fi
cd "$(git rev-parse --show-toplevel)"

if [ "$HOST" = windows ]; then
  # No newline, as the file has always been.
  printf '%s' "$VERSION" >windows/version.txt
  exit 0
fi

# The workspace's version is the one line in [workspace.package]; every crate takes it from there.
OLD=$(awk '/^\[workspace\.package\]/ {on=1; next} /^\[/ {on=0} on && /^version = "/ {gsub(/^version = "|"$/, ""); print; exit}' Cargo.toml)
[ -n "$OLD" ] || { echo "Cargo.toml has no version in [workspace.package]." >&2; exit 1; }
# A crate with a version of its own wouldn't follow the workspace's. A dependency's
# `version = "0.61"` is in another table, so only a [package] version counts.
for F in crates/*/Cargo.toml; do
  if awk '/^\[package\]/ {on=1; next} /^\[/ {on=0} on && /^version = "/ {found=1} END {exit !found}' "$F"; then
    echo "$F has a version of its own, so it won't follow the workspace's." >&2
    exit 1
  fi
done

awk -v old="$OLD" -v new="$VERSION" '
  /^\[workspace\.package\]/ {on=1; print; next}
  /^\[/ {on=0}
  on && !done && $0 == "version = \"" old "\"" {print "version = \"" new "\""; done=1; next}
  {print}
' Cargo.toml >Cargo.toml.new
mv Cargo.toml.new Cargo.toml

# Cargo.lock: the workspace's own packages are the ones with no source line. A path package's
# entry is `name`, `version`, then `dependencies`; a registry or git one has a `source`.
python3 - "$OLD" "$VERSION" <<'PY'
import re, sys
old, new = sys.argv[1:]
text = open("Cargo.lock", encoding="utf-8").read()
blocks = text.split("\n\n")
n = 0
for i, b in enumerate(blocks):
    if b.startswith("[[package]]") and "\nsource = " not in b and f'\nversion = "{old}"\n' in b + "\n":
        blocks[i] = b.replace(f'\nversion = "{old}"', f'\nversion = "{new}"', 1)
        n += 1
if n == 0:
    sys.exit(f"Cargo.lock has no workspace package at {old}.")
open("Cargo.lock", "w", encoding="utf-8").write("\n\n".join(blocks))
print(f"Cargo.lock: {n} packages moved from {old} to {new}.")
PY
