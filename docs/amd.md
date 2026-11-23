# Windows on an AMD card

This is experimental. It works on the card I have and on a few testers' cards, and there's a lot it hasn't met yet.

## What's run

My card is the integrated Radeon in a Ryzen 9 9950X3D: RDNA2, 1002:13C0, on Fedora 45's amdgpu, with AMD's Windows driver 32.0.21045.20006 in the guest.

| | |
|---|---|
| Desktop | works, on my display driver at the size and refresh you pick, and the guest sees AMD's vendor ID |
| HDR | works, where the screen has it and the machine's card draws the desktop |
| Direct3D 11 | works, the test triangle matches WARP pixel for pixel |
| Direct3D 12 | works |
| Vulkan in a window | works, 0 wrong pixels, 144 fps at 144 Hz in my present test |
| OpenGL in a window | works, 4.6 compatibility and 3.3 core, 0 wrong pixels, about 100 to 110 fps |
| Hardware video decode | not yet, it falls back to the CPU; on two newer chips it can be turned on, see [Hardware video](#hardware-video) |

Testers have run it on these too:

| Card | How it went |
|---|---|
| RX 9070 XT, RDNA4 | works: Windows, 3ds Max and Superposition |
| RX 7900 XTX, RDNA3 | installs, display driver at 1920x1080 144 Hz, all four graphics APIs start |
| RX 7800 XT, RDNA3 | works |
| RX 6900 XT, RDNA2 | works |
| RX 6700 XT, RDNA2 | works |
| RX 6700, RDNA2 | works |
| RX 6600 XT, RDNA2 | works: OpenGL games and Edge on the GPU |

On an RX 9070 XT, Superposition at 720p Low went from 21,920 to 36,903 with the speed work in 0.5.2, and 4K Optimized reaches 72% of the same card on the host. [#118](https://github.com/ruxnhq/guesthouse/issues/118) has the rest.

## Setting it up

1. Download AMD's Windows driver installer for your card from AMD's site. AMD ships each release in packages for different generations, so get the one that lists your card.
2. In the app, make a new Windows machine and pick the AMD card. When there's no AMD driver yet, the dialog asks for the installer and unpacks it for you. On a PC with no desktop, `guesthouse-vm new` does the same when you give it the installer with `--amd-driver`. [Without a window](headless.md) has the rest.
3. Install as usual. Windows comes up with AMD's driver.

For full speed your user has to be able to open `/dev/udmabuf`. The daemon gives the guest's memory to the card through it. On most distros it belongs to the `kvm` group, so `sudo usermod -aG kvm $USER` and a new login do it. The app's checks and the installer tell you when it can't be opened. Without it Windows still runs, only slower.

A machine made for AMD stays on AMD. It can move to another AMD card while it's off, but an NVIDIA card needs a new machine, and the other way round.

## Other AMD cards

Other RX 6000, RX 7000 and RX 9000 cards should work like the ones above. RX 5000 cards use the same driver package as the RX 6000s, but nobody has tried one. Older cards like Polaris aren't handled.

If you have an AMD card, open an issue to say how it went.

## OpenGL in a window

AMD's OpenGL driver shows a window's frames with a plain copy. In a guest, Windows turns each frame into a copy to a staging buffer and then copies it again with the CPU. The driver only takes the faster DXGI route for HDR, stereo, or a window the size of the screen. So the daemon turns on the driver's own setting for it, the one Adrenalin calls OpenGL Triple Buffering, and every OpenGL window gets DXGI.

On a Radeon 610M with driver 32.0.21044.64 in the guest, a 1280x720 window of a light OpenGL scene went from about 690 frames a second to 1150. A heavy scene went from 206 to 264, the same as Direct3D 12. Driver 32.0.32015.12012 reads the setting the same way, but nobody has run it with this yet. Layered windows still get the copy, as the driver refuses DXGI for them on any PC.

It's on by default. With `GUESTHOUSE_AMD_GL_COPY=1` in the environment the app or `guesthouse-vm` starts the machine from, OpenGL windows get the copy as before. Try that if an OpenGL program shows a wrong or stale picture.

## Sharing the card

Off by default since 0.7.7. With `GUESTHOUSE_AMD_GIVE_WAY=1` in the environment the app or `guesthouse-vm` starts the machine from, a guest gives way when it sees others waiting for the card. It then sends one piece of work at a time instead of several, so a small app on the host, in a container or in another VM waits for one of the guest's at most, not for all of them. With nobody else on the card nothing changes, and the guest's own programs, such as Windows' compositor beside a game, don't count as others.

In 0.7.6 it was on for everyone and also paused after each piece of work. A guest alone on the card could then give way to itself, and on a tester's Radeon 780M GravityMark ran at under a third of 0.7.5's score (#327). That's fixed, but not yet tried on a card that shows it. A small app gets less than it did in 0.7.6 beside a busy guest, and more than with the switch off.

## Hardware video

Off by default, and an experiment. With `GUESTHOUSE_AMD_VIDEO=1` in the environment the app or `guesthouse-vm` starts the machine from, the daemon offers Windows the card's video engine. AMD's driver then lists its hardware decoders, and AMF finds its encoder. Without it, video is decoded and encoded on the CPU as before.

It's only offered on Phoenix and Strix chips, which are the Radeon 780M, the 890M and their siblings, and only on a chip with one video instance. The guest's driver has to be told how much video work the chip takes, and that's known for those two. With a second instance amdgpu reads more of the guest's video work than the daemon has copied, so the daemon asks amdgpu how many there are. On any other card the daemon's log says why, and the machine starts without it.

Know this before you turn it on. Video work that the engine's firmware doesn't take makes amdgpu reset the card's video ring, and a reset hits everything on the host that's using the video engine just then: a browser playing a video, a recording, another machine. Earlier builds did that, with two encoders opened one after the other in one process. This build hasn't, in twenty runs of RustDesk's codec probe, but what changed isn't known. So turn it on only on a host where that's all right.

Two more switches were a stopgap for that reset, and haven't been needed with this build. Each is off unless set, and does nothing without the first. `GUESTHOUSE_AMD_VIDEO_CLOSE=1` has the daemon close an encode session in the card's firmware when the guest's driver says it's over, which neither the guest's driver nor AMD's own does. `GUESTHOUSE_AMD_VIDEO_REST=1500` holds a new encode session's first work until the video engine has been idle that many milliseconds, long enough for amdgpu to power it down. The rest makes RustDesk's codec probe run out of time, and it then misses encoders.

A tester ran it on a Radeon 780M with driver 32.0.32015.12012 in the guest:

- H.264 and HEVC decode through Direct3D 11, and the frames match the software decoder's. For H.264 the daemon has Windows in the guest keep a locked buffer's CPU view until the buffer is freed, as AMD's driver reads the stream after it has unlocked it. `GUESTHOUSE_UNMAP_AT_UNLOCK=1` brings back the old behaviour, for any card, and H.264 then crashes in AMD's driver.
- `hevc_amf`, `h264_amf` and `av1_amf` encode 1080p, about 2 ms a frame.
- RustDesk's codec probe finds the decoders and encoders and writes its codec file itself, ten times in a row without the two switches above.
- Video work doesn't hold up the daemon. The guest's other GPU work goes on while the card decodes or encodes.

Programs like ffmpeg's `h264_amf`, RustDesk and OBS load AMF by name, so the guest gets what AMD's installer puts there: `amfrt64.dll` in System32 and `amfrt32.dll` in SysWOW64. Each start copies them from the driver's folder in the guest if they're missing, and a newer driver is followed because the names come from its INF. That doesn't depend on `GUESTHOUSE_AMD_VIDEO`, as it's what a PC with the driver installed has. Without the switch AMF finds no encoder and the program falls back to the CPU, same as before.

Earlier test builds of the same work also ran AV1 decode, and sessions of a minute and more in 1440p and 4K.

## Linux guests on AMD

That's a different route, through virtio-gpu's native context, and it's in [`linux/`](../linux/README.md#an-amd-card).
