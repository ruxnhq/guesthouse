# The Android image

An Android machine boots Guesthouse's own build of LineageOS 24 (Android 17) for VMs, its `virtio_x86_64`
target, as a userdebug build. It draws through QEMU's virtio GPU on the host's card: OpenGL ES through virgl
and Vulkan through Venus, on NVIDIA, AMD or Intel. It is based on LineageOS and isn't an official LineageOS
build.

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
docker build -f android/Containerfile -t guesthouse-android-build android
mkdir -p ~/lineage24
docker run --rm --user="$(id -u):$(id -g)" -e HOME=/src/.home -e JOBS=12 --memory=64g \
    -v ~/lineage24:/src -v "$PWD/android:/guesthouse:ro" guesthouse-android-build \
    bash /guesthouse/build.sh sync
```

The system disk is `out/target/product/virtio_x86_64/disk-vda.img`. A machine is made from a folder holding
it as `vda.qcow2`: `qemu-img convert -O qcow2 disk-vda.img vda.qcow2`. The source of each project it was
built from is pinned in `pinned-manifest.xml` at the top of the tree.

## Sources

LineageOS's sources are at https://github.com/LineageOS, at the revisions in the image's pinned manifest.
Guesthouse's changes are this folder: `vendor-guesthouse/` and `patches/`. microG is from
https://github.com/microg/GmsCore and Digitalis from https://github.com/DigitalisX64.
