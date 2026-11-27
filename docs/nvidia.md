# Windows on an NVIDIA card

## The card's memory clock while a device views the machine

When another device views a machine through [remote view](remote-view.md), Windows encodes the machine's screen on the card. An NVIDIA card clocks itself up for steady work, such as a game, a video or scrolling, but not for a desktop that changes now and then. It stays at its idle clocks, and each new picture then takes 20 to 40 ms to encode instead of 7 to 10.

So while a device streams a machine on an NVIDIA card, Guesthouse holds the card's memory clock at the highest the card says it supports. It holds it from when the device connects until about half a minute after the device has gone. It puts the clock back when the machine stops, and when the remote view ends in any other way. A clock you locked yourself with `nvidia-smi` while nothing of Guesthouse's held the card is left as it is. If you lock the memory clock yourself while Guesthouse holds it, Guesthouse's reset when the device leaves undoes yours too. If you manage the card's clocks yourself, turn this off.

The cost is power. While held, an idle card draws about what it draws under load: 100 W rather than 17 W on an RTX 3080 Ti.

It's on by default. To turn it off for a machine, turn off Full Memory Clock While Viewed on the Advanced page of its settings, or run `guesthouse-vm set NAME stream-clocks off`. A running machine lets the clock go at once.

Locking a clock needs root, so the installer puts a small helper in `/usr/local/libexec/guesthouse/guesthouse-clock-helper`, setuid root. On NixOS the module puts it in `/run/wrappers/bin`. It does this one thing: it locks one NVIDIA card's memory clock, or puts it back. Without the helper, as in a build from the repository, Guesthouse leaves the clocks alone and says so in one line of its log.
