#!/bin/bash
# Makes the signing keys of Guesthouse's Android image: the release key, the platform, shared, media and
# network stack keys, and the rest the build signs with. The private halves are written to FOLDER and
# nowhere else; keep them in a secret store and give them to build.sh as its /keys folder. Nothing made here
# belongs in the repository.
#
# Usage: make-keys.sh FOLDER   (FOLDER must not exist yet)
set -euo pipefail

NAMES="releasekey platform shared media networkstack bluetooth nfc sdk_sandbox testkey"
SUBJECT='/C=XX/O=Guesthouse/OU=Android image/CN=Guesthouse Android'

folder=${1:?usage: make-keys.sh FOLDER}
[ ! -e "$folder" ] || { echo "$folder is there already" >&2; exit 1; }
umask 077
mkdir -p "$folder"
for name in $NAMES; do
    pem=$(mktemp)
    openssl genrsa -out "$pem" 4096 2>/dev/null
    # The private key in the form the build reads, with no passphrase, and its certificate.
    openssl pkcs8 -topk8 -inform PEM -outform DER -in "$pem" -out "$folder/$name.pk8" -nocrypt
    openssl req -new -x509 -sha256 -key "$pem" -out "$folder/$name.x509.pem" -days 10950 -subj "$SUBJECT $name"
    rm -f "$pem"
done
echo "Made $(echo "$NAMES" | wc -w) keys in $folder."
