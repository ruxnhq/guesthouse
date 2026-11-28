# Android machines

This is a preview. It's in the app from 0.9.0, on Linux hosts, marked Preview in New Machine.

An Android machine runs Guesthouse's own build of LineageOS 24, which is Android 17. It draws on your card through QEMU's virtio GPU: OpenGL ES 3.2 through virgl, and Vulkan through Venus. That works on NVIDIA, AMD and Intel cards, as it runs on the host's own OpenGL and Vulkan drivers. It has been tried on an RTX 5090 and a Radeon 610M.

## Making one

In New Machine, choose Android.

1. The first time, Download fetches Guesthouse's Android image from files.ruxn.com, about 1.4 GB, and checks it. It's kept, so the next Android machine needs no download. Choose takes an image you built yourself instead.
2. Set the size of its data disk, its cores and its RAM, and whether it has Play services.
3. Create. There's nothing to install. The first start is Android's own setup.

From a terminal, `guesthouse-vm new NAME --kind android` does the same, with `--play on` for Play services. [The commands](headless.md).

Stop shuts Android down cleanly, as a long press of its power button does.

## Play services

Play services come through [microG](https://microg.org), an open-source stand-in for Google's. Apps that need them can sign in, get notifications and find your location. It's off unless you turn it on, in New Machine or later in the machine's settings, and it keeps its data while it's off.

There's no Play Store and no Google apps. Apps come from Aurora Store or F-Droid. Apps that check Play Integrity, such as some banking apps and games, may refuse to run.

## Apps for ARM phones

Most Android apps are built for ARM phones. Those built only for ARM64 run too, translated by [Digitalis](https://github.com/DigitalisX64), an open ARM64 backend for Android's Berberis. Translated code is slower than code built for x86.

## Not yet

- A shared clipboard, dragging files in and out, and shared folders.
- Windows hosts. Android machines are on Linux hosts only.
- Moving a machine to a newer image. Its system disk holds the key to its data, so a new image needs a new machine for now.
- AMD's native context, which would make OpenGL faster on AMD cards.

## The image

It's built from LineageOS's sources with Guesthouse's setup app, microG and Digitalis added. [guest/android/](../guest/android/README.md) is how it's built, and each published image has the manifest of the sources it came from beside it. It's a userdebug build, which is how LineageOS lets microG stand in for Google's services, so root over adb is possible inside the guest.

It isn't an official LineageOS build, and it isn't from Google. Android and Google Play are trademarks of Google LLC, and LineageOS is a trademark of LineageOS. Guesthouse isn't made, endorsed or supported by either. [THIRD-PARTY.md](../THIRD-PARTY.md) has the licences.
