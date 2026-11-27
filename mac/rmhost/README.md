# rmhost

`rmhost` shares a machine's screen with a remoteman viewer on another device. The graphics device publishes each frame as an IOSurface, `rmhost` hands the surface straight to the Mac's hardware HEVC encoder, and `rmhost serve` sends the stream to the viewer through remoteman's own core. The viewer's mouse and keyboard go into the guest through QMP. Nothing runs in the guest, and the CPU never copies the pixels.

It is the Mac's piece of remote view: the shared crate `guesthouse-remote-view-core` drives its pairing (the agent feeds it what `rmhost` says, and sends `rmhost` the lines it reads), and `mac/rmframes` is how the frames reach it.

## Build

```sh
sh mac/rmhost/build.sh
```

That builds `rmhost`, `testsource`, `attacker` and `viewcheck` into `mac/build/rmhost`. `sh mac/rmhost/build.sh --helpers FOLDER` also puts `rmhost` in a folder, which `mac/nightly.sh --helpers` takes to put it in the app. `--no-serve` builds an `rmhost` that only records and counts, with no remoteman code in it.

`rmhost serve` links remoteman's core, which isn't in this repository. `remoteman-source.json` is the pin: remoteman 0.1.10, the version `crates/remote-view/remoteman.json` names for the other hosts, and the commit of [remoteman](https://github.com/ruxnlabs/remoteman) that released it. The build fetches that commit, checks its hash, builds the core with `cargo build --locked`, and links it. If the two pins disagree the build stops, so they move together. It needs git, Rust with the `aarch64-apple-darwin` target and a network the first time.

## Test

```sh
sh mac/rmhost/test.sh
```

It needs a Mac with Metal and no guest. `testsource` publishes a moving 1080p picture at 60 Hz the way the device does, `rmhost stats` has to receive frames from it, and `attacker` forges about 270 000 dead-name notices and wrong messages while readers read: it must not be able to drop a reader or free a name.

## Using it

```sh
mac/build/rmhost/testsource -t 15 &              # publishes as rmhost.frames
mac/build/rmhost/rmhost stats 10                  # or: rmhost record /tmp/out.mov 10
mac/build/rmhost/rmhost serve -p -l 127.0.0.1:7791 &   # -q QMP_SOCKET for input
mac/build/rmhost/viewcheck -o /tmp/view.hevc 127.0.0.1:7791 /tmp/vstate 6
```

`rmhost serve` takes lines on its standard input: `pair open SECONDS`, `pair accept`, `pair decline`, `pair close`, `device FINGERPRINT allow KIND on|off`, `revoke FINGERPRINT` and `drop`. It says what happens on its output, one line each: `rmhost: pairing open until UNIX_SECONDS`, `rmhost: pairing pending JSON`, `rmhost: paired FINGERPRINT NAME` and `rmhost: pairing closed: REASON`. The app reads those lines.

## Licence

The files here are MIT. remoteman's core is GPL-3.0-or-later, so an `rmhost` with `serve` in it is under that licence as a whole, which GPL-2.0-or-later code can be combined with. The app runs it as a program of its own.
