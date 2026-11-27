#!/bin/sh
# Puts the signing key where publish-repo.sh takes it from, for a workflow, and checks it's the key
# whose public half is in this repo as packaging/packages/ruxn-pkg.asc. That's the half the docs
# give and pkg.ruxn.com serves, so a secret with another key in it stops here and signs nothing.
#
# Usage: SIGNING_KEY_SECRET=... packaging/packages/signing-key.sh FOLDER
#
# FOLDER is the runner's own temporary folder. It writes FOLDER/signing-key.asc, readable only by
# this user, and adds SIGNING_KEY and SIGNING_KEY_FILE to $GITHUB_ENV. The key is never printed.
set -eu

PUBLIC_KEY=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)/ruxn-pkg.asc

die() {
    printf 'signing-key: %s\n' "$*" >&2
    exit 1
}

fingerprint() {
    gpg --batch --with-colons --import-options show-only --import "$1" 2>/dev/null |
        awk -F: '$1 == "fpr" { print $10; exit }'
}

folder=${1:?name the runner temporary folder}
[ -n "${SIGNING_KEY_SECRET:-}" ] || die "SIGNING_KEY_SECRET is empty, so there's no key to sign with."
[ -f "$PUBLIC_KEY" ] || die "there's no $PUBLIC_KEY yet, so there's nothing to check the key against."
key_file=$folder/signing-key.asc
(
    umask 077
    printf '%s\n' "$SIGNING_KEY_SECRET" >"$key_file"
)
secret=$(fingerprint "$key_file")
public=$(fingerprint "$PUBLIC_KEY")
[ -n "$secret" ] || die "the secret holds no key gpg can read."
[ "$secret" = "$public" ] || die "the secret's key is $secret, and the repo's public key is $public."
{
    printf 'SIGNING_KEY=%s\n' "$secret"
    printf 'SIGNING_KEY_FILE=%s\n' "$key_file"
} >>"${GITHUB_ENV:?}"
printf 'signing with %s\n' "$secret"
