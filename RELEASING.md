# Cutting a release

I do this by hand on my own PC. Nothing is automated yet, and `SHA256SUMS` isn't signed yet.

The order matters. `install.sh` on `main` is what points people at a release, so it changes last, once everything it names is really there.

Below, `VERSION` is the version with no `v`, like `0.2.0`, and `BUILD_FOLDER` is a new folder with about 10 GB free. Use a neutral one like `/var/tmp/guesthouse-release`, and never one under my home folder. Its path ends up in the binaries, as the compilers write down where each source file was. It has to be new because an old one keeps its QEMU tree, and the patch wouldn't get applied again.

1. **Set the version.** Change `version` in `Cargo.toml`, run `cargo check` so `Cargo.lock` follows, and merge that into `main` with a pull request. `main` takes nothing else, and the checks have to pass.

2. **Tag it.** Tag the commit that pull request became on `main`. The download URLs have a `v` in them, so the tag must too.

   ```sh
   git tag vVERSION
   ```

3. **Build the display driver.** This one step is in Windows, as [drivers/windows-display/BUILDING.md](drivers/windows-display/BUILDING.md) says, from the tagged commit. Copy `GuesthouseDisplay.dll` and `GuesthouseDisplay.inf` to a folder on this PC, outside the repo. They never go in git.

   If nothing compiled in `drivers/windows-display` has changed since my last build, I reuse that build and skip Windows. `git diff LAST_BUILD vVERSION -- drivers/windows-display` shows it. The docs, the licences, `vdd_settings.xml` and the scripts in `setup/` don't count, as step 4 takes those from the tree.

4. **Build it in the images.** It needs podman or docker and no root, and takes a commit with nothing uncommitted. It stops if the release needs a glibc above 2.39, if the bundled source is missing, if it has no display driver, if it couldn't download OpenSSH for Windows, or if an archive has a link that points nowhere. The driver's licences go in beside it.

   ```sh
   packaging/build-in-images.sh --out BUILD_FOLDER --display-driver DRIVER_FOLDER
   ```

   The last lines it prints are the version and two hashes for step 7.

5. **Try it.** Both of these need the release in `BUILD_FOLDER`, and neither touches your real home folder.

   ```sh
   tests/install/run.sh
   tests/install/real-run.sh BUILD_FOLDER NEW_SCRATCH_FOLDER
   tests/install/real-in-containers.sh BUILD_FOLDER
   ```

6. **Upload it.** Push the tag, then make the release with these five files, and no others. Nothing points at it yet, so a mistake here costs nothing.

   ```sh
   git push origin vVERSION
   gh release create vVERSION --title VERSION --notes-file NOTES \
     BUILD_FOLDER/guesthouse-VERSION-x86_64.tar.xz \
     BUILD_FOLDER/guesthouse-installer-VERSION.AppImage \
     BUILD_FOLDER/guesthouse-VERSION-source.tar.xz \
     BUILD_FOLDER/guesthouse-VERSION-bundled-source.tar \
     BUILD_FOLDER/SHA256SUMS
   ```

   The two sources are there for the GPL and the LGPL, so a release doesn't go out without them. They have to stay up for as long as the payload does.

   The site's download button points at the AppImage on files.ruxn.com, not at GitHub. So publish it there too, with its hash beside it, using the file drop's `files` tool:

   ```sh
   cd BUILD_FOLDER
   sha256sum guesthouse-installer-VERSION.AppImage > /tmp/guesthouse-installer-VERSION.AppImage.sha256
   files publish guesthouse guesthouse-installer-VERSION.AppImage
   files publish guesthouse /tmp/guesthouse-installer-VERSION.AppImage.sha256
   ```

7. **Point `main` at it, last of all.** In `install.sh`, set `RELEASE_VERSION`, `PAYLOAD_SHA256` and `SOURCE_SHA256` to what step 4 printed. In `packaging/nix/package.nix`, set `version` too, and `hash` to the payload's SHA-256 as Nix writes it, which `nix hash convert --hash-algo sha256 --to sri PAYLOAD_SHA256` prints. In the same pull request, set the version in the README's note and in `site/public/index.html`: the Linux version at the top, the AppImage's two links and the release's link. Merge it all with one pull request. From that moment the one line installs this release, `--update` and `--check-update` offer it to everyone, and the flake gives it to NixOS.

8. **Deploy the site.** Nothing deploys it by itself, so until this step guesthouse.ruxn.com still names the release before. From a checkout of `main` at that merge, in `site`, run `cf deploy public`. Then open the page and see that it names the new version.

9. **Check it from the outside.** Run `install.sh --dry-run` from the raw URL, in a fake HOME. It should download, verify and show the plan.

If something's wrong after step 7, revert that one commit, and everyone goes back to the release before. A file on a release must never change once `main` names it: fix it with a new version.
