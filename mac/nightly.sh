#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# Builds the Mac app as a nightly: from a clean checkout, in Release, stamped with the date and the
# commit, every program in it signed inside-out with the hardened runtime, checked, made into a
# compressed disk image that is signed too, and, when notary keys are given, notarised and stapled.
# It writes the image Guesthouse-nightly-YYYYMMDD-COMMIT8.dmg, the same name with .sha256 after
# it, and guesthouse-macos-nightly.json into the output folder.
#
#   sh mac/nightly.sh [--out FOLDER] [--helpers FOLDER]
#
# Everything private comes from the environment, and none of it is ever printed or kept:
#   GUESTHOUSE_SIGN_IDENTITY       the identity to sign with (a name or a SHA-1), or `-` for ad hoc.
#   GUESTHOUSE_SIGN_P12            a .p12 file holding the identity, with any encryption: it is read
#   GUESTHOUSE_SIGN_P12_PASSWORD   with an openssl that can, and its certificate and key go, in an
#                                  old-format bundle with a random password, into a keychain made here
#                                  that is deleted on the way out. The login keychain is never touched.
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
saved_list=
cleanup() {
    status=$?
    if [ -n "$saved_list" ]; then
        # shellcheck disable=SC2086
        security list-keychains -d user -s $saved_list >/dev/null 2>&1 || true
    fi
    if [ -n "$keychain" ]; then security delete-keychain "$keychain" >/dev/null 2>&1 || true; fi
    rm -rf "$work"
    exit "$status"
}
trap cleanup EXIT INT TERM HUP

say() { printf '%s\n' "$*"; }

# --- The name: version, date and commit ---------------------------------------------------
# The date is the commit's own (UTC), and the commit is its first 8 hex digits, so a commit always
# has the same name, however often or whenever it is built: a second run for it, after a publish
# that failed halfway, has the same names to find.
date_stamp=${GUESTHOUSE_VERSION_DATE:-$(TZ=UTC git -C "$here" show -s --format=%cd --date=format-local:%Y%m%d HEAD 2>/dev/null || echo unknown)}
commit=${GUESTHOUSE_COMMIT:-$(git -C "$here" rev-parse HEAD 2>/dev/null | cut -c1-8)}
case $date_stamp in
    [0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]) ;;
    *) echo "nightly.sh: the date $date_stamp is not YYYYMMDD" >&2; exit 1 ;;
esac
case $commit in
    [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
    *) echo "nightly.sh: the commit $commit is not 8 hex digits" >&2; exit 1 ;;
esac
base=$(sed -n 's/^ *MARKETING_VERSION: "\([0-9]*\.[0-9]*\)\.[0-9]*".*/\1/p' project.yml | head -n 1)
minimum=$(sed -n 's/^ *macOS: "\([0-9.]*\)".*/\1/p' project.yml | head -n 1)
[ -n "$base" ] && [ -n "$minimum" ] || { echo "nightly.sh: project.yml has no version or deployment target" >&2; exit 1; }
# The release's major and minor, and the date as the patch: newer than the release it follows
# and older than the next one, and three numbers, as the update check reads them.
version=$base.$date_stamp
name=Guesthouse-nightly-$date_stamp-$commit
iso_date=$(printf '%s' "$date_stamp" | sed 's/\(....\)\(..\)\(..\)/\1-\2-\3/')

# --- Reading the .p12 ------------------------------------------------------------------------
# macOS's `security import` reads only the old encryption of a .p12 (3DES and a SHA-1 MAC), and a
# bundle made with a new openssl (AES and a SHA-256 MAC) is refused as "MAC verification failed". So
# the bundle is read with an openssl that can, and the certificate and key go into a new bundle with
# the old encryption and a fresh random password, which `security` reads: that works for any key
# type and any openssl, where importing the PEM files one by one depends on the key's format. The
# password reaches openssl through the environment, never a command line. The fresh one is for a
# file that exists for a moment in the private folder, and it is on `security`'s command line,
# which has no other way to take it.
find_openssl() {
    candidates=$(command -v openssl 2>/dev/null || true)
    for dir in /opt/homebrew/opt/openssl@4 /opt/homebrew/opt/openssl@3 /opt/homebrew/opt/openssl \
        /usr/local/opt/openssl@4 /usr/local/opt/openssl@3 /usr/local/opt/openssl; do
        [ -x "$dir/bin/openssl" ] && candidates="$candidates $dir/bin/openssl"
    done
    candidates="$candidates /usr/bin/openssl"
    for candidate in $candidates; do
        [ -x "$candidate" ] || continue
        for legacy in "" -legacy; do
            # shellcheck disable=SC2086
            if "$candidate" pkcs12 $legacy -in "$GUESTHOUSE_SIGN_P12" -passin env:GUESTHOUSE_SIGN_P12_PASSWORD \
                -nokeys -noout >/dev/null 2>&1; then
                ossl=$candidate
                ossl_legacy=$legacy
                return 0
            fi
        done
    done
    return 1
}

import_p12() {
    keychain_to=$1
    find_openssl || {
        echo "nightly.sh: no openssl on this machine reads the .p12 (wrong password, or a format none of them reads)" >&2
        return 1
    }
    say "p12: read with $("$ossl" version | awk '{ print $1, $2 }')${ossl_legacy:+ ($ossl_legacy)}"
    mkdir -p "$work/p12"
    chmod 700 "$work/p12"
    # shellcheck disable=SC2086
    "$ossl" pkcs12 $ossl_legacy -in "$GUESTHOUSE_SIGN_P12" -passin env:GUESTHOUSE_SIGN_P12_PASSWORD \
        -nodes -out "$work/p12/identity.pem" >/dev/null 2>&1 ||
        "$ossl" pkcs12 $ossl_legacy -in "$GUESTHOUSE_SIGN_P12" -passin env:GUESTHOUSE_SIGN_P12_PASSWORD \
            -noenc -out "$work/p12/identity.pem" >/dev/null 2>&1 || {
        echo "nightly.sh: cannot unpack the .p12" >&2
        return 1
    }
    rewrap_pass=$(openssl rand -hex 24)
    REWRAP_PASS=$rewrap_pass "$ossl" pkcs12 -export -in "$work/p12/identity.pem" -out "$work/p12/old.p12" \
        -passout env:REWRAP_PASS -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1 >/dev/null 2>&1 || {
        rm -rf "$work/p12"
        echo "nightly.sh: cannot make the old-format bundle" >&2
        return 1
    }
    rm -f "$work/p12/identity.pem"
    security import "$work/p12/old.p12" -k "$keychain_to" -P "$rewrap_pass" \
        -T /usr/bin/codesign -T /usr/bin/productsign -T /usr/bin/security >/dev/null
    rm -rf "$work/p12"
}

# --- The Developer ID intermediate ---------------------------------------------------------------
# codesign builds the signature's chain from the certificate to Apple's root, and needs Apple's
# "Developer ID Certification Authority" (G2) between them. The system's keychains have it on a Mac that
# has been used for signing; a fresh build machine may not. So it is fetched from Apple's own site over
# https, checked against the SHA-256 pinned here, and put in the temporary keychain.
DEVID_G2_URL=https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer
DEVID_G2_SHA256=f16cd3c54c7f83cea4bf1a3e6a0819c8aaa8e4a1528fd144715f350643d2df3a
developer_id_chain() {
    if security find-certificate -a -Z -c "Developer ID Certification Authority" 2>/dev/null |
        tr 'A-F' 'a-f' | grep -q "$DEVID_G2_SHA256"; then
        say "chain: Apple's Developer ID G2 intermediate is in this machine's keychains already"
    else
        say "chain: Apple's Developer ID G2 intermediate was not found here"
    fi
    curl -fsSL --proto =https --max-time 60 -o "$work/devid-g2.cer" "$DEVID_G2_URL"
    got=$(shasum -a 256 "$work/devid-g2.cer" | awk '{ print $1 }')
    [ "$got" = "$DEVID_G2_SHA256" ] || {
        echo "nightly.sh: Apple's intermediate has the fingerprint $got, not the pinned one" >&2
        return 1
    }
    security import "$work/devid-g2.cer" -k "$1" >/dev/null 2>&1 || true
    say "chain: the intermediate (checked against its pinned SHA-256) is in the build keychain"
}

# --- The identity -------------------------------------------------------------------------
identity=${GUESTHOUSE_SIGN_IDENTITY:-}
kc_args=
names=
if [ -n "${GUESTHOUSE_SIGN_P12:-}" ]; then
    keychain=$work/nightly.keychain-db
    pass=$(openssl rand -hex 24)
    security create-keychain -p "$pass" "$keychain"
    security set-keychain-settings -lut 7200 "$keychain"
    security unlock-keychain -p "$pass" "$keychain"
    import_p12 "$keychain"
    security set-key-partition-list -S apple-tool:,apple: -s -k "$pass" "$keychain" >/dev/null
    if [ -z "$identity" ]; then
        identity=$(security find-identity -p codesigning "$keychain" | awk '/^ *1\)/ { print $2 }')
    fi
    # Which certificate signs, by its name only, so a log shows it and nothing else.
    names=$(security find-identity -p codesigning "$keychain" | sed -n 's/^ *[0-9]*) [0-9A-F]* "\(.*\)".*/\1/p')
    say "identity: $names"
    case $names in
        "Developer ID Application"*) developer_id_chain "$keychain" ;;
    esac
    if [ -n "${GITHUB_ACTIONS:-}" ] || [ -n "${CI:-}" ]; then
        # On a build machine of its own, the keychain is put first in the search list for the run and the
        # list is put back on the way out, so the chain can be built.
        saved_list=$(security list-keychains -d user | tr -d '"' | sed 's/^ *//')
        # shellcheck disable=SC2086
        security list-keychains -d user -s "$keychain" $saved_list
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
    say "signing: ${names:-the identity given}"
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
cat >"$work/dmg/SOURCE.txt" <<TXT
Guesthouse nightly $version, built from commit $commit of https://github.com/ruxnlabs/guesthouse
(mac/ and crates/host-mac). Guesthouse is free software under GPL-2.0-or-later, and that
repository is the source of this build, with the patch and the version of QEMU named in
docs/mac-host.md if QEMU is inside (Contents/Helpers).
TXT
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
printf '%s  %s\n' "$sum" "$name.dmg" >"$out/$name.dmg.sha256"
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
say "  $out/$name.dmg.sha256"
say "  $out/guesthouse-macos-nightly.json"
say "  version $version, commit $commit, sha256 $sum"
