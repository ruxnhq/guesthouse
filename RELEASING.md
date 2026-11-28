# Cutting a release

The Linux host and the Windows host are released apart, each with a version of its own, so each goes out when it has something new. The Linux host's version is `version` in the workspace's `Cargo.toml`, and its tags are `vVERSION`. The Windows host's is `windows/version.txt`, and its tags are `windows-vVERSION`. A Linux release never changes `windows/version.txt`, and a Windows release never changes `Cargo.toml`'s version. Both start from `main`, so a release of either has what's merged by then, and the Windows host's installer carries what the Linux host shares with it, such as the NVAPI wrapper, as `main` has it.

## The quick way, for either host

1. **Run Release.** In the Actions tab, run `Release` (`.github/workflows/release.yml`) from `main` with the new `version`, like `0.11.0`, and the `host`, `linux` or `windows`. It checks that the version is well-formed and newer than the host's last tag, and that no tag or open release pull request for it exists. It opens the pull request `Release VERSION` (`Release Windows VERSION` for Windows) that changes only the version files, as `scripts/release-bump.sh` does, and turns on auto-merge. When the checks pass and it merges, the run tags that merge commit and pushes the tag. If a required check fails, or it hasn't merged in two hours, the run fails and says so, and nothing is tagged. Fix it, close the pull request, and run Release again. It needs the secret `RELEASE_PR_TOKEN`, as the pull request's checks and the tag's build don't start from `GITHUB_TOKEN`, and stops at once without it.

2. **Read the draft and publish it.** The tag starts the build, which makes the draft, as the sections below say. Publish it by hand: for Linux as the latest release, for Windows with "Set as the latest release" unticked. Nothing publishes by itself.

3. **The rest follows.** Publishing starts `Release files`, `Release credits` and `Point the installer`. With `RELEASE_PR_TOKEN` set, that one turns on auto-merge for its pull request, so `main` points at the release once the checks pass. If the AppImage wasn't on files.ruxn.com in time, that pull request is a draft and waits for a person, as step 7 says. A Windows release's site pull request (step 5 of the Windows host's steps) comes from the same workflow.

Everything below is what that does, and the fallback if a part of it fails.

## The Linux host

CI builds it from the tag, and makes the GitHub release as a draft. I read the draft and publish it, and the rest follows by itself. Building by hand on my own PC, below that, is the fallback. `SHA256SUMS` isn't signed yet.

### From the tag, in CI

1. **Set the version,** as step 1 of the by-hand way says. The `Release` workflow does this and step 2 too.

2. **Tag it and push the tag.** Tag the commit that pull request became on `main`.

   ```sh
   git tag vVERSION
   git push origin vVERSION
   ```

3. **CI builds it.** The tag starts the `Release build` workflow (`.github/workflows/release-build.yml`). It does steps 3 to 6 of the by-hand way:
   - It stops if the tag isn't on `main`, or isn't the version in `Cargo.toml`.
   - The display driver follows step 3's rule. If nothing compiled in `guest/drivers/windows-display` changed since the release before (lines that are only comments don't count), that release's DLL and INF go in again, checked against its `SHA256SUMS`. Otherwise a Windows runner builds it, with Visual Studio's WDK rather than the Enterprise WDK ISO. Then try the new driver in a guest before publishing.
   - It runs `packaging/build-in-images.sh` in `/var/tmp/guesthouse-release`, then step 5's three tests on what it built.
   - It makes the release as a draft, with the five files and notes drafted by `scripts/release-notes.py`. The notes come from the pull requests merged since the release before, squashed or with a merge commit, under the area each title starts with. They're only this host's: a pull request whose title names another host ("Mac host: ...", "Windows app: ..."), or that touched only another host's files (`mac/`, `windows/`, a host's own crates), is left out, and so are release and version pull requests and the ones pointing the installer or the site at a release. Whatever is left out is listed with why in a comment at the end, to put back by hand. A comment at the top says whether the tests passed.

   Nobody sees a draft, and nothing points at it. If the workflow fails, fix it and run it again from the Actions tab, with the tag and "release" ticked. That replaces a draft's files, and never touches a published release. With "release" unticked it only builds and tests.

4. **Read the draft and publish it.** Write the opening, sort the lines, and drop what nobody using it would notice. Check that the comment says the tests passed. Then publish it, as the latest release.

5. **Publishing starts the rest.** `Release files` puts the AppImage and its `.sha256` on files.ruxn.com. `Point the installer` opens the pull request of step 7, and `Release credits` writes the Thanks into the notes. Then go on from step 7.

`Release files` needs the publish address on files.ruxn.com's Worker. Until that's there, publish the AppImage by hand, as the end of step 6 says.

### By hand

The same release, built on my own PC. Pointing `main` at the release, the credits in its notes and the site's deploy follow by themselves from the release being published, as steps 6 to 8 say.

The order matters. `install.sh` on `main` is what points people at a release, so it changes last, once everything it names is really there.

Below, `VERSION` is the version with no `v`, like `0.2.0`, and `BUILD_FOLDER` is a new folder with about 10 GB free. Use a neutral one like `/var/tmp/guesthouse-release`, and never one under my home folder. Its path ends up in the binaries, as the compilers write down where each source file was. It has to be new because an old one keeps its QEMU tree, and the patch wouldn't get applied again.

1. **Set the version.** Change `version` in the workspace's `Cargo.toml`, run `cargo check` so `Cargo.lock` follows (or run `scripts/release-bump.sh linux VERSION`, which does both), and merge that into `main` with a pull request. `main` takes nothing else, and the checks have to pass.

2. **Tag it.** Tag the commit that pull request became on `main`. The download URLs have a `v` in them, so the tag must too.

   ```sh
   git tag vVERSION
   ```

3. **Build the display driver.** This one step is in Windows, as [guest/drivers/windows-display/BUILDING.md](guest/drivers/windows-display/BUILDING.md) says, from the tagged commit. Copy `GuesthouseDisplay.dll` and `GuesthouseDisplay.inf` to a folder on this PC, outside the repo. They never go in git.

   If nothing compiled in `guest/drivers/windows-display` has changed since my last build, I reuse that build and skip Windows. `git diff LAST_BUILD vVERSION -- guest/drivers/windows-display` shows it. The docs, the licences, `vdd_settings.xml` and the scripts in `setup/` don't count, as step 4 takes those from the tree.

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

   Write `NOTES` without a credits section. Once the release is published, the `Release credits` workflow adds one to the notes: a line for each pull request of someone outside the project that was merged since the release before, as `scripts/release-credits.py` finds them. To see it first, run `scripts/release-credits.py TAG`. If the workflow didn't run, run it from the Actions tab with the tag. The README's Thanks list names projects, not people.

   The two sources are there for the GPL and the LGPL, so a release doesn't go out without them. They have to stay up for as long as the payload does.

   The site's download button points at the AppImage on files.ruxn.com, not at GitHub. Publishing the release starts the `Release files` workflow, which puts it there with its hash. If it can't, publish them by hand with the file drop's `files` tool:

   ```sh
   cd BUILD_FOLDER
   sha256sum guesthouse-installer-VERSION.AppImage > /tmp/guesthouse-installer-VERSION.AppImage.sha256
   files publish guesthouse guesthouse-installer-VERSION.AppImage
   files publish guesthouse /tmp/guesthouse-installer-VERSION.AppImage.sha256
   ```

7. **Point `main` at it, last of all.** Publishing the release starts the `Point the installer` workflow. It downloads the payload and the source again and checks them against `SHA256SUMS`, waits for the AppImage's `.sha256` on files.ruxn.com, and opens the pull request `Point the installer at VERSION`. That one sets `RELEASE_VERSION`, `PAYLOAD_SHA256` and `SOURCE_SHA256` in `install.sh`, `version` and `hash` in `packaging/nix/package.nix`, the version in the README's note, and in `site/public/index.html` the Linux version at the top, the AppImage's links, name and size, and the release's link, name and size, all by `scripts/point-installer.py`. If the AppImage isn't on files.ruxn.com within half an hour, the pull request is a draft: publish it, then mark it ready. Read the pull request, and merge it once the checks pass. From that moment the one line installs this release, `--update` and `--check-update` offer it to everyone, and the flake gives it to NixOS. If the workflow didn't run, run it from the Actions tab with the tag, or do the same by hand with `scripts/point-installer.py VERSION SHA256SUMS PAYLOAD_BYTES APPIMAGE_BYTES`.

   The merge starts the Packages workflow, as it changes `install.sh`. It downloads the release, checks it against the two hashes, and puts a copy of its files on pkg.ruxn.com, the deb, rpm and pacman packages in their signed repositories with an attestation each, and last of all `guesthouse/latest.json`. From that moment `--check-update` and the app in a package offer it too. Watch the run. If it fails, run it again from the Actions tab: what it published already stays as it is.

8. **The site deploys itself.** A change to `site/` on `main` runs the `Site deploy` workflow, which does what `cf deploy public` in `site` did. Open the page after the merge and see that it names the new version. If the workflow fails, run it again from the Actions tab, or deploy by hand as before.

9. **Check it from the outside.** Run `install.sh --dry-run` from the raw URL, in a fake HOME. It should download, verify and show the plan.

If something's wrong after step 7, revert that one commit, and everyone goes back to the release before. A file on a release must never change once `main` names it: fix it with a new version.

## Proxmox's package

The Proxmox package workflow builds each new `pve-qemu-kvm` from Proxmox the night it comes out and puts it in the `testing` suite of `pkg.ruxn.com/proxmox`. A version that's in either suite already isn't built again. When our patch changes and Proxmox's version doesn't, run the workflow by hand with a new suffix, such as `+gpupv2`. It refuses a version that's published.

Once a build in `testing` has run with a GPU-PV VM on a real card, run the Proxmox stable workflow with its version, such as `11.1.3-4+gpupv1`. It waits for a reviewer's approval in the `proxmox-stable` environment, then copies the same file to `stable` and signs its index again.

## The Windows host

CI builds this from the tag too. Nothing in the installer is signed, and nothing in its build needs a card, so a GitHub Windows runner can build it. The by-hand way is the fallback.

### From the tag, in CI

1. **Set the version,** as step 1 of the by-hand way says. The `Release` workflow does this and step 2 too.

2. **Tag it and push the tag,** on the commit that pull request became on `main`.

   ```sh
   git tag -a windows-vVERSION -m "Windows host VERSION"
   git push origin windows-vVERSION
   ```

3. **CI builds it.** The tag starts `Release build, Windows host` (`.github/workflows/release-windows.yml`). It stops if the tag isn't on `main` or isn't `windows/version.txt`'s version, and runs `scripts/windows-release.ps1 -BuildOnly` on a Windows runner. That's `-DryRun` without the check that the tag isn't on GitHub yet, so it still stops if this version's installer is on files.ruxn.com. Then it makes the release `Windows VERSION` as a draft, not marked latest, with the installer, its `.sha256`, WinFsp's source and `guesthouse-windows-latest.json`. Run it again from the Actions tab with the tag if it fails.

4. **Try it, then publish it.** Download the installer from the draft, install it on this PC and see the new things work. Publish the draft, and leave "Set as the latest release" unticked, as that's the Linux host's. Publishing starts `Release files`. It puts WinFsp's source, the installer and its `.sha256` on files.ruxn.com, then `guesthouse-windows-latest.json` last of all. From that moment every app offers the new version.

5. **Point the site at it. This is automatic.** Publishing the draft also starts `Point the installer`, the same workflow as Linux's. For a `windows-vVERSION` release it downloads the installer and checks it against its `.sha256`, waits up to half an hour for that `.sha256` on files.ruxn.com to show the same hash, then opens `Point the site at Windows VERSION`. That changes only the Windows version in the line at the top and the installer's link, name, size and hash link in `site/public/index.html`, and nothing else, and it stops if any of those places isn't found exactly once. With `RELEASE_PR_TOKEN` set it turns on auto-merge, so the site follows once the checks pass, and `Site deploy` puts it up. If the hash never shows, the pull request is a draft and waits for a person. If the workflow fails, run `Point the installer` again from the Actions tab with the tag, like `windows-v0.12.0`, or do it by hand as step 4 of the by-hand way says.

### By hand

This is on Windows, from a clean checkout of `main`, with git, cargo, gh and the cf CLI on PATH and each signed in. `scripts/windows-release.ps1` does steps 2 to 4.

1. **Set the version.** Change `windows/version.txt` to the new version (`scripts/release-bump.sh windows VERSION` does it), three numbers and no `v`, and merge that into `main` with a pull request, `Windows VERSION`. It takes nothing else, and the checks have to pass. The service says this version to the app, the installer puts it in Settings > Apps, and the app compares it with the newest to offer the update.

2. **Try it.** From a checkout of `main` at that merge, run `powershell -ExecutionPolicy Bypass -File scripts\windows-release.ps1 -DryRun`. It stops if the tree isn't clean, or if the tag or the installer of this version is published already, as a published file never changes. Then it builds the app and the installer into `target\windows-release\VERSION\`, with `guesthouse-setup-VERSION.exe`, its `.sha256` and `guesthouse-windows-latest.json`, and publishes nothing. Install it on this PC and see the new things work.

3. **Release it.** Run the script again without `-DryRun`. It builds again, tags the commit `windows-vVERSION` and pushes the tag, and makes the GitHub release `Windows VERSION` with the installer and its `.sha256`, which isn't marked latest, as that's the Linux host's. Then it publishes the installer, its `.sha256` and, last of all, `guesthouse-windows-latest.json` to the file drop with `cf r2 objects put`: the bucket `ruxn-files`, at `pub/guesthouse/`, which files.ruxn.com serves as `/p/guesthouse/`. It downloads the installer back to check it. From the moment the latest file is up, every app offers the new version.

4. **Point the site at it.** The workflow does this for a published release, so this is the fallback. In `site/public/index.html`, set the Windows version in the line at the top, and the installer's link, name and hash link, in a pull request. Merge it, and the `Site deploy` workflow puts it up. Then open the page and see that it names the new version.

If the release is wrong, publish a new version: a file on the file drop never changes once it's up. To stop apps offering a broken one meanwhile, put the previous version's `guesthouse-windows-latest.json` back.
