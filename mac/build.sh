#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# Builds the core, the project and the app, into build/dd/Build/Products/Release/Guesthouse.app.
# Ad hoc signed unless GUESTHOUSE_SIGN_IDENTITY holds the SHA-1 of a team's Apple Development
# certificate in the login keychain; the team is read from that certificate here, never stored.
set -e
cd "$(dirname "$0")"
./build-core.sh
# The icon's pictures are kept in the tree; they are drawn again only if they are missing (and by
# hand, with Icon/make-icon.swift, when the drawing changes).
if [ ! -f App/Assets.xcassets/AppIcon.appiconset/icon_512x512@2x.png ]; then
    swiftc -O Icon/make-icon.swift -o build/make-icon
    build/make-icon Icon App/Assets.xcassets
fi
xcodegen generate
sign=
if [ -n "${GUESTHOUSE_SIGN_IDENTITY:-}" ]; then
    pem=$(security find-certificate -c "Apple Development" -a -Z -p | awk -v h="$GUESTHOUSE_SIGN_IDENTITY" '
        /^SHA-1 hash:/ { want = ($3 == h) } want { print }' | sed -n '/BEGIN CERT/,/END CERT/p')
    team=$(printf '%s\n' "$pem" | openssl x509 -noout -subject -nameopt multiline | awk '/organizationalUnitName/ { print $3 }')
    [ -n "$team" ] || { echo "no Apple Development certificate with that SHA-1" >&2; exit 1; }
    sign="CODE_SIGN_IDENTITY=$GUESTHOUSE_SIGN_IDENTITY DEVELOPMENT_TEAM=$team"
fi
# shellcheck disable=SC2086
xcodebuild -project Guesthouse.xcodeproj -scheme Guesthouse -configuration Release \
    -destination 'platform=macOS,arch=arm64' -derivedDataPath build/dd $sign \
    ${GUESTHOUSE_XCODEBUILD_SETTINGS:-} build
