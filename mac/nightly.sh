#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# Builds the Mac app as a nightly: from a clean checkout, in Release, stamped with the date and the
# commit, every program in it signed inside-out with the hardened runtime, checked, made into a
# compressed disk image that is signed too, and, when notary keys are given, notarised and stapled.
# It writes the image, NAME.sha256 and guesthouse-macos-nightly.json into the output folder.
#
#   sh mac/nightly.sh [--out FOLDER] [--helpers FOLDER]
#
# Everything private comes from the environment, and none of it is ever printed or kept:
#   GUESTHOUSE_SIGN_IDENTITY       the identity to sign with (a name or a SHA-1), or `-` for ad hoc.
#   GUESTHOUSE_SIGN_P12            a .p12 file holding the identity. It is imported into a keychain
#   GUESTHOUSE_SIGN_P12_PASSWORD   made here with a random password, which is deleted on the way out;
#                                  the login keychain is never touched.
#   GUESTHOUSE_KEYCHAIN            instead, a keychain that already holds the identity (and its
#   GUESTHOUSE_KEYCHAIN_PASSWORD   password, to unlock it, if it needs one).
#   GUESTHOUSE_NOTARY_KEY_ID, GUESTHOUSE_NOTARY_ISSUER_ID, GUESTHOUSE_NOTARY_KEY_PATH
#                                  the App Store Connect key that notarises. With none of the three
#                                  the build is not notarised, and says so.
#   GUESTHOUSE_HELPERS_DIR         (or --helpers) a folder of programs to put in Contents/Helpers,
#                                  such as QEMU, the graphics process and rmhost, each signed with
#                                  Nightly/entitlements/NAME.entitlements if there is one.
#   GUESTHOUSE_VERSION_DATE, GUESTHOUSE_COMMIT   override the date (YYYYMMDD) and the short commit,
#                                  for tests.
# With no identity at all it signs ad hoc, and says so.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
out=$here/build/nightly
helpers=${GUESTHOUSE_HELPERS_DIR:-}
while [ $# -gt 0 ]; do
    case $1 in
        --out) out=${2:?--out wants a folder}; shift 2 ;;
        --helpers) helpers=${2:?--helpers wants a folder}; shift 2 ;;
        -h | --help) sed -n 2,28p "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "nightly.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done

work=$(mktemp -d)
keychain=
cleanup() {
    status=$?
    if [ -n "$keychain" ]; then security delete-keychain "$keychain" >/dev/null 2>&1 || true; fi
    rm -rf "$work"
    exit "$status"
}
trap cleanup EXIT INT TERM HUP

say() { printf '%s\n' "$*"; }

# --- The name: version, date and commit ---------------------------------------------------
date_stamp=${GUESTHOUSE_VERSION_DATE:-$(date -u +%Y%m%d)}
commit=${GUESTHOUSE_COMMIT:-$(git -C "$here" rev-parse --short=7 HEAD 2>/dev/null || echo unknown)}
base=$(sed -n 's/^ *MARKETING_VERSION: "\([0-9]*\.[0-9]*\)\.[0-9]*".*/\1/p' project.yml | head -n 1)
minimum=$(sed -n 's/^ *macOS: "\([0-9.]*\)".*/\1/p' project.yml | head -n 1)
[ -n "$base" ] && [ -n "$minimum" ] || { echo "nightly.sh: project.yml has no version or deployment target" >&2; exit 1; }
# The release's major and minor, and the date as the patch: newer than the release it follows
# and older than the next one, and three numbers, as the update check reads them.
version=$base.$date_stamp
name=Guesthouse-nightly-$date_stamp-$commit
iso_date=$(printf '%s' "$date_stamp" | sed 's/\(....\)\(..\)\(..\)/\1-\2-\3/')

# --- The identity -------------------------------------------------------------------------
identity=${GUESTHOUSE_SIGN_IDENTITY:-}
kc_args=
if [ -n "${GUESTHOUSE_SIGN_P12:-}" ]; then
    keychain=$work/nightly.keychain-db
    pass=$(openssl rand -hex 24)
    security create-keychain -p "$pass" "$keychain"
    security set-keychain-settings -lut 7200 "$keychain"
    security unlock-keychain -p "$pass" "$keychain"
    security import "$GUESTHOUSE_SIGN_P12" -k "$keychain" -P "${GUESTHOUSE_SIGN_P12_PASSWORD:-}" \
        -T /usr/bin/codesign -T /usr/bin/security >/dev/null
    security set-key-partition-list -S apple-tool:,apple: -s -k "$pass" "$keychain" >/dev/null
    if [ -z "$identity" ]; then
        identity=$(security find-identity -p codesigning "$keychain" | awk '/^ *1\)/ { print $2 }')
    fi
    kc_args=$keychain
elif [ -n "${GUESTHOUSE_KEYCHAIN:-}" ]; then
    kc_args=$GUESTHOUSE_KEYCHAIN
    if [ -n "${GUESTHOUSE_KEYCHAIN_PASSWORD:-}" ]; then
        security unlock-keychain -p "$GUESTHOUSE_KEYCHAIN_PASSWORD" "$kc_args"
    fi
fi
if [ -z "$identity" ]; then
    identity=-
    say "signing: ad hoc (no identity was given), so this build is for trying on this Mac only"
else
    say "signing: identity $identity"
fi

# codesign, with the identity and the keychain if there is one, and a timestamp for a real one.
sign() {
    entitlements=$1
    shift
    if [ "$identity" = - ]; then
        stamp=--timestamp=none
    else
        stamp=--timestamp
    fi
    if [ -n "$kc_args" ]; then
        codesign --force --options runtime "$stamp" --keychain "$kc_args" -s "$identity" \
            ${entitlements:+--entitlements "$entitlements"} "$@"
    else
        codesign --force --options runtime "$stamp" -s "$identity" \
            ${entitlements:+--entitlements "$entitlements"} "$@"
    fi
}

notary=
if [ -n "${GUESTHOUSE_NOTARY_KEY_ID:-}" ] && [ -n "${GUESTHOUSE_NOTARY_ISSUER_ID:-}" ] \
    && [ -n "${GUESTHOUSE_NOTARY_KEY_PATH:-}" ]; then
    notary=yes
    say "notarising: yes"
else
    say "notarising: no (the notary key id, issuer id and key path were not all given)"
fi
notarise() {
    xcrun notarytool submit "$1" --key "$GUESTHOUSE_NOTARY_KEY_PATH" \
        --key-id "$GUESTHOUSE_NOTARY_KEY_ID" --issuer "$GUESTHOUSE_NOTARY_ISSUER_ID" --wait
}

# --- Build ---------------------------------------------------------------------------------
say "building $name (version $version)"
GUESTHOUSE_XCODEBUILD_SETTINGS="MARKETING_VERSION=$version CURRENT_PROJECT_VERSION=$date_stamp" \
    sh ./build.sh >"$work/build.log" 2>&1 || { tail -n 30 "$work/build.log" >&2; exit 1; }
app_src=$here/build/dd/Build/Products/Release/Guesthouse.app
[ -d "$app_src" ] || { echo "nightly.sh: the build left no app" >&2; exit 1; }
mkdir -p "$out"
stage=$work/stage
mkdir -p "$stage"
ditto "$app_src" "$stage/Guesthouse.app"
app=$stage/Guesthouse.app

if [ -n "$helpers" ]; then
    [ -d "$helpers" ] || { echo "nightly.sh: no helpers folder $helpers" >&2; exit 1; }
    mkdir -p "$app/Contents/Helpers"
    ditto "$helpers" "$app/Contents/Helpers"
    say "helpers: $(ls "$app/Contents/Helpers" | tr '\n' ' ')"
fi

# --- Sign inside-out: every Mach-O file, deepest first, then the app ------------------------
main_exec=$app/Contents/MacOS/Guesthouse
find "$app" -type f ! -path "$main_exec" | while IFS= read -r file; do
    if file -b "$file" | grep -q 'Mach-O'; then
        depth=$(printf '%s' "$file" | tr -cd / | wc -c)
        printf '%s\t%s\n' "$depth" "$file"
    fi
done | sort -rn | cut -f2- >"$work/machos"
signed=0
while IFS= read -r file; do
    base_name=$(basename "$file")
    ent=$here/Nightly/entitlements/$base_name.entitlements
    [ -f "$ent" ] || ent=
    sign "$ent" "$file"
    signed=$((signed + 1))
done <"$work/machos"
sign "$here/Nightly/entitlements/guesthouse.entitlements" "$app"
say "signed: $signed programs inside, then the app"

codesign --verify --deep --strict --verbose=2 "$app"
say "verified: codesign --verify --deep --strict passes"
assess() {
    if result=$(spctl --assess --type "$1" --verbose "$2" 2>&1); then
        say "spctl: accepted ($result)"
    else
        say "spctl: rejected ($result)"
    fi
}

if [ -n "$notary" ]; then
    ditto -c -k --keepParent "$app" "$work/app.zip"
    notarise "$work/app.zip"
    xcrun stapler staple "$app"
    xcrun stapler validate "$app"
fi
assess execute "$app"

# --- The disk image -------------------------------------------------------------------------
mkdir "$work/dmg"
ditto "$app" "$work/dmg/Guesthouse.app"
ln -s /Applications "$work/dmg/Applications"
dmg=$out/$name.dmg
rm -f "$dmg"
hdiutil create -quiet -volname "Guesthouse nightly" -srcfolder "$work/dmg" -ov -format UDZO -fs HFS+ "$dmg"
if [ "$identity" = - ]; then
    codesign --force --timestamp=none -s - "$dmg"
elif [ -n "$kc_args" ]; then
    codesign --force --timestamp --keychain "$kc_args" -s "$identity" "$dmg"
else
    codesign --force --timestamp -s "$identity" "$dmg"
fi
if [ -n "$notary" ]; then
    notarise "$dmg"
    xcrun stapler staple "$dmg"
    xcrun stapler validate "$dmg"
fi
assess open "$dmg"

# --- What it made ---------------------------------------------------------------------------
sum=$(shasum -a 256 "$dmg" | awk '{ print $1 }')
size=$(stat -f %z "$dmg")
printf '%s  %s\n' "$sum" "$name.dmg" >"$out/$name.sha256"
cat >"$out/guesthouse-macos-nightly.json" <<JSON
{
  "version": "$version",
  "commit": "$commit",
  "date": "$iso_date",
  "file": "$name.dmg",
  "size": $size,
  "sha256": "$sum",
  "minimum_macos": "$minimum"
}
JSON
say "made:"
say "  $dmg ($size bytes)"
say "  $out/$name.sha256"
say "  $out/guesthouse-macos-nightly.json"
say "  version $version, commit $commit, sha256 $sum"
