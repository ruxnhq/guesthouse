#!/bin/sh
# Writes the landing page for pkg.ruxn.com from page/index.html. The setup commands on it are the ones
# in docs/install.md's Packages section and docs/proxmox.md's repository section, copied out of the
# docs' own code blocks at publish time, so the page and the docs can't say different things.
#
# Usage: packaging/packages/make-page.sh [--latest latest.json] OUTPUT
#
# With --latest, the page names the version in that file. Without it, or if the file names none, it
# names no version.
set -eu

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$HERE/../.." && pwd)

die() {
    printf 'make-page: %s\n' "$*" >&2
    exit 1
}

# The Nth ```sh block under the heading "## TITLE" of a Markdown file, escaped for HTML.
block() {
    awk -v title="$2" -v want="$3" '
        /^## / { in_section = ($0 == "## " title); next }
        in_section && /^```sh$/ { n++; show = (n == want); next }
        in_section && /^```/ { show = 0; next }
        in_section && show { print }
    ' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}

latest=
if [ "${1:-}" = --latest ]; then
    latest=${2:?name the latest.json file}
    shift 2
fi
out=${1:?name the file to write}

version=
if [ -n "$latest" ]; then
    [ -f "$latest" ] || die "there's no file called $latest."
    version=$(sed -n 's/^ *"version": *"\([0-9][0-9.]*\)".*/\1/p' "$latest" | head -n 1)
fi
if [ -n "$version" ]; then
    latest_text="Latest release <b>$version</b>"
else
    latest_text="Signed packages"
fi

install_md=$REPO/docs/install.md
proxmox_md=$REPO/docs/proxmox.md
apt=$(block "$install_md" Packages 1)
dnf=$(block "$install_md" Packages 2)
pacman=$(block "$install_md" Packages 3)
proxmox=$(block "$proxmox_md" "The repository" 1)
for part in "$apt" "$dnf" "$pacman" "$proxmox"; do
    [ -n "$part" ] || die "a setup block is missing from the docs."
done
fingerprint=$(grep -Eo '`[0-9A-F]{40}`' "$install_md" | head -n 1 | tr -d '`')
[ -n "$fingerprint" ] || die "docs/install.md names no key fingerprint."

# Each {{NAME}} in the template is replaced by the value of the same name. awk's ENVIRON is used, as
# the commands hold quotes and backslashes that sed's replacement would read as its own.
tmp=$out.tmp
APT=$apt DNF=$dnf PACMAN=$pacman PROXMOX=$proxmox LATEST=$latest_text FINGERPRINT=$fingerprint awk '
    BEGIN { n = split("APT DNF PACMAN PROXMOX LATEST FINGERPRINT", names, " ") }
    {
        for (k = 1; k <= n; k++) {
            token = "{{" names[k] "}}"
            i = index($0, token)
            if (i) $0 = substr($0, 1, i - 1) ENVIRON[names[k]] substr($0, i + length(token))
        }
        print
    }
' "$HERE/page/index.html" >"$tmp"
! grep -q '{{' "$tmp" || die "the template has a placeholder nothing fills."
mv "$tmp" "$out"
