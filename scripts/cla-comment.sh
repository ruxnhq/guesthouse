#!/bin/sh
# Keeps one comment on a pull request that says what its author still has to add for the CLA, and
# takes it away once the check passes. It runs on pull_request_target, so with the base branch's
# files and a token that can write, for pull requests from forks too. The pull request's own code
# is never checked out or run here. All it takes from the pull request is the text of one file,
# read through the API and only searched by scripts/cla-check.sh, which does the checking.
# Needs GH_TOKEN and REPO, PR (its number), LOGIN (its author), HEAD_REPO and HEAD_SHA (its branch).
set -eu

MARKER='<!-- cla-check -->'

# Accounts that sign nothing get no comment. Their logins can hold brackets, so this comes first.
if sh scripts/cla-check.sh --exempt "$LOGIN"; then
    echo "$LOGIN signs nothing."
    exit 0
fi

# Checked before it goes in a path or a pattern. The same rule as scripts/cla-check.sh.
case $LOGIN in
    *[!A-Za-z0-9-]* | '')
        echo "Not a login this check knows: $LOGIN" >&2
        exit 1
        ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/signers"
# The signers who are in the base branch already, and then what the pull request has for its author.
cp CLA/signers/*.md "$work/signers/" 2>/dev/null || true
name=$(printf '%s' "$LOGIN" | tr 'A-Z' 'a-z').md
if gh api -H 'Accept: application/vnd.github.raw+json' \
    "repos/$HEAD_REPO/contents/CLA/signers/$name?ref=$HEAD_SHA" >"$work/head.md" 2>/dev/null; then
    cp "$work/head.md" "$work/signers/$name"
fi

if sh scripts/cla-check.sh "$LOGIN" "$work/signers" >/dev/null 2>"$work/why"; then
    passed=1
else
    passed=
fi

existing=$(gh api --paginate "repos/$REPO/issues/$PR/comments" \
    --jq ".[] | select(.user.login == \"github-actions[bot]\" and (.body | contains(\"$MARKER\"))) | .id" | head -n 1)

if [ -n "$passed" ]; then
    if [ -n "$existing" ]; then
        gh api -X DELETE "repos/$REPO/issues/comments/$existing"
    fi
    exit 0
fi

{
    printf '%s\n' "$MARKER"
    cat "$work/why"
} >"$work/body"

if [ -n "$existing" ]; then
    old=$(gh api "repos/$REPO/issues/comments/$existing" --jq .body)
    if [ "$old" = "$(cat "$work/body")" ]; then
        exit 0
    fi
    gh api -X PATCH "repos/$REPO/issues/comments/$existing" -F "body=@$work/body" >/dev/null
else
    gh api "repos/$REPO/issues/$PR/comments" -F "body=@$work/body" >/dev/null
fi
