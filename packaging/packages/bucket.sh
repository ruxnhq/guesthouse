#!/bin/sh
# Copies a part of pkg.ruxn.com down from its bucket, or up to it, for the workflows. The bucket is
# Cloudflare R2's ruxn-pkg, reached through its S3 API with the AWS command line the runners have.
#
# Usage: packaging/packages/bucket.sh pull PREFIX FOLDER
#        packaging/packages/bucket.sh push FOLDER PREFIX
#        packaging/packages/bucket.sh put FILE KEY
#        packaging/packages/bucket.sh has KEY
#        packaging/packages/bucket.sh remove PREFIX
#
# R2_ENDPOINT, AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY say where and who. A push goes in three
# steps: the packages, which never change once they're up, then the indexes, which name them, and
# last the removal of what the indexes no longer name. So a client that reads an index finds what it
# names, and a slow one that read the old index finds that too until the end.
set -eu

BUCKET=${BUCKET:-ruxn-pkg}
# Indexes and the landing page change with each publish, so Cloudflare keeps them a minute. Everything
# else never changes.
INDEX_CACHE='public, max-age=60'
FILE_CACHE='public, max-age=604800'
# R2 doesn't take the checksums newer AWS command lines send by default.
export AWS_REQUEST_CHECKSUM_CALCULATION=when_required AWS_RESPONSE_CHECKSUM_VALIDATION=when_required
export AWS_DEFAULT_REGION=auto AWS_EC2_METADATA_DISABLED=true

die() {
    printf 'bucket: %s\n' "$*" >&2
    exit 1
}

s3() {
    aws --endpoint-url "$R2_ENDPOINT" s3 "$@" --only-show-errors
}

# Only a plain key, with no way out of the prefix it names.
check_key() {
    case $1 in
        "" | /* | *..* | *//* | *[!A-Za-z0-9._+~/-]*) die "$1 isn't a key this would write." ;;
    esac
}

# What a client reads to find the packages.
INDEXES='--include dists/* --include repodata/* --include *.db --include *.db.* --include *.files
 --include *.files.* --include *.repo --include latest.json --include SHA256SUMS'
EXCLUDE_INDEXES='--exclude dists/* --exclude repodata/* --exclude *.db --exclude *.db.* --exclude *.files
 --exclude *.files.* --exclude *.repo --exclude latest.json --exclude SHA256SUMS'

[ -n "${R2_ENDPOINT:-}" ] || die "R2_ENDPOINT isn't set."
case ${1:-} in
    pull)
        check_key "$2"
        mkdir -p "$3"
        s3 sync "s3://$BUCKET/$2/" "$3/"
        ;;
    push)
        check_key "$3"
        [ -d "$2" ] || die "there's no folder called $2."
        # shellcheck disable=SC2086 # the patterns are flags and are quoted for aws, not the shell
        {
            set -f
            s3 sync "$2/" "s3://$BUCKET/$3/" $EXCLUDE_INDEXES --cache-control "$FILE_CACHE"
            s3 sync "$2/" "s3://$BUCKET/$3/" --exclude '*' $INDEXES --cache-control "$INDEX_CACHE"
            s3 sync "$2/" "s3://$BUCKET/$3/" --delete --cache-control "$FILE_CACHE"
            set +f
        }
        ;;
    put)
        check_key "$3"
        [ -f "$2" ] || die "there's no file called $2."
        cache=$FILE_CACHE
        case ${3##*/} in latest.json | SHA256SUMS | index.html) cache=$INDEX_CACHE ;; esac
        s3 cp "$2" "s3://$BUCKET/$3" --cache-control "$cache"
        ;;
    has)
        check_key "$2"
        aws --endpoint-url "$R2_ENDPOINT" s3api head-object --bucket "$BUCKET" --key "$2" >/dev/null 2>&1
        ;;
    remove)
        check_key "$2"
        s3 rm "s3://$BUCKET/$2/" --recursive
        ;;
    *) die "usage: bucket.sh pull PREFIX FOLDER | push FOLDER PREFIX | put FILE KEY | has KEY | remove PREFIX" ;;
esac
