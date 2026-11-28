#!/bin/sh
# Says whether a pull request's author has signed the contributor licence agreement:
# whether the pull request leaves CLA/signers/LOGIN.md, with their line for this version in it.
# The file is named after the login in lower case, and the line has to be the author's own.
# Usage: cla-check.sh LOGIN [SIGNERS_DIR], or cla-check.sh --exempt LOGIN
set -eu

VERSION=1
# Accounts that sign nothing: the bots, and RUXN LLC's own.
EXEMPT='dependabot[bot] github-actions[bot] xnixy'

# cla-check.sh --exempt LOGIN only says, by its exit status, whether LOGIN is one of them.
if [ "${1:-}" = --exempt ]; then
    for name in $EXEMPT; do
        [ "${2:-}" = "$name" ] && exit 0
    done
    exit 1
fi

login=${1:?usage: cla-check.sh LOGIN [SIGNERS_DIR]}
signers=${2:-CLA/signers}

for name in $EXEMPT; do
    if [ "$login" = "$name" ]; then
        echo "$login signs nothing."
        exit 0
    fi
done

# A GitHub login is letters, digits and hyphens, so it is safe inside the pattern below.
case $login in
    *[!A-Za-z0-9-]* | '')
        echo "Not a login this check knows: $login" >&2
        exit 1
        ;;
esac

name=$(printf '%s' "$login" | tr 'A-Z' 'a-z').md
file=$signers/$name

if [ -f "$file" ] && grep -Eiq "^- .+, @$login, [0-9]{4}-[0-9]{2}-[0-9]{2}, version $VERSION\$" "$file"; then
    echo "@$login has signed version $VERSION."
    exit 0
fi

cat >&2 <<MESSAGE
@$login hasn't signed the contributor licence agreement, version $VERSION.
Read CLA.md, then add the file CLA/signers/$name to this pull request, with this one line in it:

    - Your Name, @$login, $(date -u +%F), version $VERSION
MESSAGE
exit 1
