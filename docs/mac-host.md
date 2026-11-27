# The Mac host

This is the Mac app: `mac/` is the SwiftUI app, `crates/host-mac/` its Rust core. It runs Windows 11 for ARM or Ubuntu on Apple silicon, with the guest drawing on the Mac's GPU.

It is a preview, and some of it isn't here yet. Here is what the app needs that this repo doesn't hold:

- `guesthouse-pvg-host`, the graphics process, and the library it uses to talk to the guest's driver. They come in their own pull requests.
- A QEMU with the `pvg-fwd` device, from a patch that comes with the graphics process.
- A Windows guest driver and its package, signed for testing. The package isn't published, so an install stops at its driver step and offers to go on without it.

The app finds all three through its Settings, an environment variable (`GUESTHOUSE_QEMU`, `GUESTHOUSE_PVG_QEMU`, `GUESTHOUSE_PVG_HOST`, `GUESTHOUSE_RMHOST`), or inside its own bundle.

The graphics process uses three classes of Apple's ParavirtualizedGraphics framework that Apple doesn't declare, as QEMU's patch that lists them does. A macOS update can break them. Guesthouse isn't affiliated with Apple, and the app isn't distributed through the App Store.

See `mac/README.md` for how to build it.

## Nightly builds

A nightly build of the Mac app is built from `main` each night, if `main` has changed, by `.github/workflows/mac-nightly.yml` on a GitHub-hosted Mac (the `xcode-27` image, which has Xcode 27 and macOS 27). `mac/nightly.sh` does the building: a Release build, every program in it signed with the hardened runtime, a signed disk image, notarisation and stapling, then the image, its `.sha256` and `guesthouse-macos-nightly.json` (version, commit, date, file, size, sha256 and the lowest macOS). The app's Settings can look for it: Updates, Look for nightly builds, which is off unless you turn it on.

None has been published yet. Until the first one is on files.ruxn.com there is nothing to download, and the app's switch finds nothing.

The workflow needs these before its first run:

- An environment called `mac-nightly`, limited to `main`, with five secrets: `MAC_SIGN_P12_BASE64` and `MAC_SIGN_P12_PASSWORD` (the Developer ID Application certificate and its key), `MAC_NOTARY_KEY`, `MAC_NOTARY_KEY_ID` and `MAC_NOTARY_ISSUER_ID` (an App Store Connect key that notarises). `mac/nightly.sh` never prints them and puts the certificate in a keychain of its own that it deletes.
- The Worker behind files.ruxn.com taking `mac-nightly.yml` for the project `guesthouse-nightly`, whose files are at `/p/guesthouse-nightly/`, as it takes `release-files.yml` for `/p/guesthouse/`. It takes only `Guesthouse-nightly-YYYYMMDD-<8 hex>.dmg` and its `.dmg.sha256` (written once), and `Guesthouse-nightly.dmg`, its `.sha256` and `guesthouse-macos-nightly.json` (which are replaced). The upload has no secret: `scripts/files-publish.sh` sends the job's GitHub OIDC token.
- For the 14-day cleanup (`mac-nightly-cleanup.yml`), the repository variable `FILES_BUCKET` and an environment `mac-nightly-cleanup` with the bucket's keys. Without them the cleanup does nothing.

A nightly is the release's major and minor with the date as the patch (`0.1.20261128`), so it is newer than the release it follows and older than the next one.
