#!/bin/sh
# Puts pkg.ruxn.com's public key and its landing page in the bucket, for the workflows. The page is
# made by make-page.sh and goes up last of the two, with a short cache time like the other indexes.
#
# Usage: R2_ENDPOINT=... packaging/packages/publish-page.sh FOLDER [LATEST_JSON]
#
# FOLDER is the runner's own temporary folder. With no LATEST_JSON, the page names the version in
# the one the bucket serves now, and none if that can't be read. Only ruxn-pkg.asc, ruxn-pkg.gpg and
# index.html are written, and bucket.sh's rules about keys apply.
set -eu

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
folder=${1:?name the runner temporary folder}
latest=${2:-}

if [ -z "$latest" ]; then
    latest=$folder/latest.json
    curl -fsSL --max-time 30 https://pkg.ruxn.com/guesthouse/latest.json -o "$latest" || rm -f "$latest"
fi

gpg --batch --dearmor <"$HERE/ruxn-pkg.asc" >"$folder/ruxn-pkg.gpg"
sh "$HERE/bucket.sh" put "$HERE/ruxn-pkg.asc" ruxn-pkg.asc
sh "$HERE/bucket.sh" put "$folder/ruxn-pkg.gpg" ruxn-pkg.gpg

if [ -f "$latest" ]; then
    sh "$HERE/make-page.sh" --latest "$latest" "$folder/index.html"
else
    sh "$HERE/make-page.sh" "$folder/index.html"
fi
sh "$HERE/bucket.sh" put "$folder/index.html" index.html
