#!/bin/bash
# Tests pkg.ruxn.com's landing page: that make-page.sh puts the docs' own setup commands in it, names
# the version in latest.json, and fills everything in. Needs no network and no bucket.
#
# Usage: tests/packages/page.sh
set -u

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$HERE/../.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
failed=0

fail() {
    printf 'FAILED: %s\n' "$*"
    failed=1
}

# The page's text with its HTML escapes undone and the pre tags off the first and last line of a block.
unescape() {
    sed -e 's/^ *<pre>//' -e 's#</pre>$##' -e 's/&lt;/</g' -e 's/&gt;/>/g' -e 's/&amp;/\&/g'
}

# The Nth sh block under a heading, as the docs have it.
docs_block() {
    awk -v title="$2" -v want="$3" '
        /^## / { in_section = ($0 == "## " title); next }
        in_section && /^```sh$/ { n++; show = (n == want); next }
        in_section && /^```/ { show = 0; next }
        in_section && show { print }
    ' "$1"
}

printf '{\n  "version": "1.2.3",\n  "notes": "https://example.test"\n}\n' >"$WORK/latest.json"
sh "$REPO/packaging/packages/make-page.sh" --latest "$WORK/latest.json" "$WORK/index.html" || fail "make-page.sh failed"
sh "$REPO/packaging/packages/make-page.sh" "$WORK/plain.html" || fail "make-page.sh failed with no latest.json"

grep -q '<b>1.2.3</b>' "$WORK/index.html" || fail "the page doesn't name the version"
grep -q 'Latest release' "$WORK/plain.html" && fail "the page names a release it wasn't given"
grep -q '{{' "$WORK/index.html" && fail "a placeholder is left in the page"

check_block() {
    local name=$1 file=$2 title=$3 n=$4 docs
    docs=$(docs_block "$REPO/docs/$file" "$title" "$n")
    [ -n "$docs" ] || { fail "$name: no block in docs/$file"; return; }
    # Compare as text, with the page's HTML escapes undone, so a changed command can't hide.
    unescape <"$WORK/index.html" | grep -qxF -- "$(printf '%s\n' "$docs" | head -n 1)" || fail "$name: the page lacks the docs' first line"
    local missing
    missing=$(printf '%s\n' "$docs" | while IFS= read -r line; do
        unescape <"$WORK/index.html" | grep -qxF -- "$line" || printf '%s\n' "$line"
    done)
    [ -z "$missing" ] || fail "$name: the page lacks lines the docs have: $missing"
}
check_block apt install.md Packages 1
check_block dnf install.md Packages 2
check_block pacman install.md Packages 3
check_block proxmox proxmox.md "The repository" 1

# The fingerprint on the page is the one of the key the bucket serves.
fingerprint=$(grep -Eo '<code class="key">[0-9A-F]{40}' "$WORK/index.html" | grep -Eo '[0-9A-F]{40}')
[ -n "$fingerprint" ] || fail "the page shows no fingerprint"
if command -v gpg >/dev/null 2>&1; then
    actual=$(gpg --batch --with-colons --import-options show-only --import "$REPO/packaging/packages/ruxn-pkg.asc" 2>/dev/null |
        awk -F: '$1 == "fpr" { print $10; exit }')
    [ "$fingerprint" = "$actual" ] || fail "the page's fingerprint $fingerprint isn't ruxn-pkg.asc's $actual"
fi

# Self-contained: no script, and nothing fetched from another host.
grep -qi '<script' "$WORK/index.html" && fail "the page has a script"
grep -Eqi '<(link|img|iframe|source)[^>]*(src|href)="https?://' "$WORK/index.html" && fail "the page loads something from another host"
grep -Eqi '@import|url\(https?:' "$WORK/index.html" && fail "the page's CSS loads something from another host"

[ "$failed" = 0 ] && echo "page: ok"
exit "$failed"
