#!/bin/bash
# SPDX-License-Identifier: MIT
#
# Checks the Android image's signing keys step: make-keys.sh makes every key the build wants, with private
# halves the build can read and certificates that match them, and build.sh stops before it touches the
# source tree when a key is missing. No private key is in the repository.
#
# Usage: keys.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail=0

bash "$root/guest/android/make-keys.sh" "$work/keys" > /dev/null
for name in releasekey platform shared media networkstack bluetooth nfc sdk_sandbox testkey; do
    for file in "$work/keys/$name.pk8" "$work/keys/$name.x509.pem"; do
        [ -f "$file" ] || { echo "FAIL: no $file"; fail=1; }
    done
    # The private key reads with no passphrase, and the certificate holds its public half.
    if ! openssl pkcs8 -inform DER -nocrypt -in "$work/keys/$name.pk8" -out "$work/key.pem" 2> /dev/null; then
        echo "FAIL: $name.pk8 is not an unencrypted PKCS#8 key"
        fail=1
        continue
    fi
    from_key=$(openssl pkey -in "$work/key.pem" -pubout 2> /dev/null | sha256sum)
    from_cert=$(openssl x509 -in "$work/keys/$name.x509.pem" -noout -pubkey 2> /dev/null | sha256sum)
    [ "$from_key" = "$from_cert" ] || { echo "FAIL: $name's certificate is not for its key"; fail=1; }
done
# Two runs make two different keys.
bash "$root/guest/android/make-keys.sh" "$work/again" > /dev/null
if cmp -s "$work/keys/platform.pk8" "$work/again/platform.pk8"; then
    echo "FAIL: two runs made the same key"
    fail=1
fi
if [ -e "$work/keys" ] && bash "$root/guest/android/make-keys.sh" "$work/keys" 2> /dev/null; then
    echo "FAIL: make-keys.sh wrote over a folder that was there"
    fail=1
fi

# The build wants the keys before anything else.
mkdir "$work/none"
if KEYS="$work/none" bash "$root/guest/android/build.sh" > "$work/out.txt" 2>&1; then
    echo "FAIL: build.sh ran with no keys"
    fail=1
elif ! grep -q 'is missing' "$work/out.txt"; then
    echo "FAIL: build.sh did not say which key is missing"
    fail=1
fi

if git -C "$root" ls-files | grep -Ei '\.(pk8|p12|pfx|jks|keystore)$' | grep -q .; then
    echo "FAIL: a private key file is in the repository"
    fail=1
fi

[ "$fail" = 0 ] && echo "ok"
exit "$fail"
