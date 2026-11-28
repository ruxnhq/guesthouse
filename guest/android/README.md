# The Android image

An Android machine boots Guesthouse's own build of LineageOS 24 (Android 17) for VMs, its `virtio_x86_64`
target, as a userdebug build. It draws through QEMU's virtio GPU on the host's card: OpenGL ES through virgl
and Vulkan through Venus, on NVIDIA, AMD or Intel. It is based on LineageOS and isn't an official LineageOS
build. Android is a trademark of Google LLC, and LineageOS a trademark of LineageOS. The image isn't made or
endorsed by either, and holds no Google apps or services.

What Guesthouse adds to LineageOS:

- **Its setup app** (`vendor-guesthouse/setup`), which runs at each boot. It reads the machine's choices,
  turns Play services on or off, and makes a long press of power shut down, which is how the app stops it.
- **microG** (Apache-2.0), as Play services. It's in every image and off unless the machine has Play services.
  LineageOS fakes Google's signature for microG's real key on debuggable builds, which is why the build is
  userdebug. Root over adb is possible in the guest as a result.
- **Digitalis** (Apache-2.0), an open ARM64 backend for AOSP's Berberis, so ARM64-only apps run.
- **GRUB reading the machine's choices.** The app passes them as words in the SMBIOS SKU, `-smbios
  type=1,sku=guesthouse microg`. GRUB puts them on the kernel's line as `androidboot.guesthouse.*`, and
  Android has them as `ro.boot.guesthouse.*`.

## Building it

The build needs about 300 GB of disk, 64 GB of RAM for its container, and an hour and a half on 16 cores the
first time. Later builds only build what changed.

```sh
docker build -f guest/android/Containerfile -t guesthouse-android-build android
mkdir -p ~/lineage24
docker run --rm --user="$(id -u):$(id -g)" -e HOME=/src/.home -e JOBS=12 --memory=64g \
    -v ~/lineage24:/src -v "$PWD/android:/guesthouse:ro" guesthouse-android-build \
    bash /guesthouse/build.sh sync
```

The system disk is `out/target/product/virtio_x86_64/disk-vda.img`. A machine is made from a folder holding
it as `vda.qcow2`: `qemu-img convert -O qcow2 disk-vda.img vda.qcow2`. The source of each project it was
built from is pinned in `pinned-manifest.xml` at the top of the tree.

## Where machines get it

The app and `guesthouse-vm new --kind android` download the published image from files.ruxn.com, a
compressed qcow2 of `disk-vda.img`, and keep it only if its SHA-256 is the one in
`linux/app-core/src/downloads.rs`. A new build is published under a new name, with its checksum there.
Each published image has its pinned manifest beside it, `…-manifest.xml`.

A machine's system disk holds `/metadata`, where Android keeps the key to its data disk. A machine
can't be moved to a newer image by swapping its system disk: its data disk wouldn't open, and Android
would start in recovery.

## Sources

LineageOS's sources are at https://github.com/LineageOS, at the revisions in the image's pinned manifest.
Guesthouse's changes are this folder: `vendor-guesthouse/` and `patches/`. microG is from
https://github.com/microg/GmsCore and Digitalis from https://github.com/DigitalisX64.
