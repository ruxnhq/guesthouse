#!/usr/bin/env bash
# Publishes release files to files.ruxn.com from a GitHub workflow, at /p/PROJECT/NAME, which is the
# bucket's pub/PROJECT/. Used by release-files.yml once a release is published.
#
# Usage: scripts/files-publish.sh [--replace] PROJECT FILE...
#
# It never gets a key to the bucket, which also holds what testers send. It speaks to the Worker's
# publish address, /ci/PROJECT/{start,part,finish}, the same three steps bin/files sends through a
# publish link, with the workflow's GitHub OIDC token (audience https://files.ruxn.com) as a bearer
# token, a fresh one for each request as they last minutes. The Worker checks that token is GitHub's,
# for this repo and release-files.yml, and writes only under pub/PROJECT/. The job needs `permissions: id-token: write`. FILES_PUBLISH_TOKEN, if set, is
# sent in its place, for a Worker that takes a shared secret instead.
#
# A published file never changes. So a file that's up already with the same bytes is skipped, and one
# with other bytes stops this. --replace is for the one file that's meant to change, a latest file.
# Each file is downloaded back at the end and checked.
set -euo pipefail

SITE=${FILES_SITE:-https://files.ruxn.com}
AUDIENCE=https://files.ruxn.com
PART_BYTES=$((16 * 1024 * 1024)) # the Worker's src/limits.js

die() {
    printf 'files-publish: %s\n' "$*" >&2
    exit 1
}

token() {
    if [ -n "${FILES_PUBLISH_TOKEN:-}" ]; then
        printf '%s' "$FILES_PUBLISH_TOKEN"
        return
    fi
    [ -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ] ||
        die "there's no OIDC token to ask for. The job needs permissions: id-token: write."
    curl -fsS -H "Authorization: bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN" \
        "$ACTIONS_ID_TOKEN_REQUEST_URL&audience=$AUDIENCE" | jq -er .value
}

auth() {
    printf 'Authorization: Bearer %s' "$(token)"
}

sha256() {
    sha256sum "$1" | cut -d ' ' -f 1
}

# The hash of what's published under that name now, or nothing if there's nothing.
published_sha256() {
    local url=$1 back
    back=$(mktemp)
    if curl -fsSL -o "$back" "$url" 2>/dev/null; then
        sha256 "$back"
    fi
    rm -f "$back"
}

send() {
    local base=$1 file=$2
    local name size started stored upload_id part_count parts='[]' part answer query_name
    name=$(basename "$file")
    size=$(stat -c %s "$file")
    started=$(curl -fsS -X POST -H "$(auth)" -H 'content-type: application/json' "$base/start" \
        -d "$(jq -n --arg name "$name" --argjson size "$size" '{name: $name, size: $size}')")
    stored=$(jq -r .name <<<"$started")
    [ "$stored" = "$name" ] || die "the Worker would store $name as $stored."
    upload_id=$(jq -r .uploadId <<<"$started")
    query_name=$(jq -rn --arg name "$name" '$name | @uri')
    part_count=$(((size + PART_BYTES - 1) / PART_BYTES))
    for ((part = 1; part <= part_count; part++)); do
        answer=$(dd if="$file" bs=$PART_BYTES skip=$((part - 1)) count=1 status=none |
            curl -fsS --retry 4 --retry-all-errors -X PUT -H "$(auth)" --data-binary @- \
                -H 'content-type: application/octet-stream' \
                "$base/part?name=$query_name&uploadId=$upload_id&part=$part")
        parts=$(jq -c --argjson answer "$answer" '. + [$answer]' <<<"$parts")
    done
    curl -fsS -X POST -H "$(auth)" -H 'content-type: application/json' "$base/finish" \
        -d "$(jq -n --arg name "$name" --arg uploadId "$upload_id" --argjson parts "$parts" \
            '{name: $name, uploadId: $uploadId, parts: $parts}')" >/dev/null
}

main() {
    local replace=false
    if [ "${1:-}" = --replace ]; then
        replace=true
        shift
    fi
    [ $# -ge 2 ] || die "usage: files-publish.sh [--replace] PROJECT FILE..."
    local project=$1
    shift
    case $project in
        [a-z0-9]*) ;;
        *) die "$project isn't a project name." ;;
    esac
    local file name url want got
    token >/dev/null # stops here, before anything is sent, if there's no token to be had
    for file in "$@"; do
        [ -s "$file" ] || die "$file is missing or empty."
        name=$(basename "$file")
        url="$SITE/p/$project/$(jq -rn --arg name "$name" '$name | @uri')"
        want=$(sha256 "$file")
        got=$(published_sha256 "$url")
        if [ "$got" = "$want" ]; then
            echo "$url is up already, with the same bytes."
            continue
        fi
        if [ -n "$got" ] && [ "$replace" != true ]; then
            die "$url is up already with other bytes ($got, not $want), and a published file never changes."
        fi
        send "$SITE/ci/$project" "$file"
        got=$(published_sha256 "$url")
        [ "$got" = "$want" ] || die "$url came back as ${got:-nothing}, not $want."
        echo "Published $url ($want)."
    done
}

main "$@"
