# ib-dump

For a run that hangs an AMD card. Start Guesthouse with `GUESTHOUSE_AMD_IB_DUMP=1` and the daemon keeps copies of the last graphics and compute command buffers of each queue. It writes them to `ib-dump-latest.bin` in the VM's run folder every ten seconds, and to `ib-dump-hung-1.bin` (up to `-4`) when a run hangs. The run folder is `$XDG_RUNTIME_DIR/guesthouse/run` for the first VM, `run-2` and on for others, next to `daemon.out`.

`split.py ib-dump-hung-1.bin` writes each command buffer to a raw `.bin` file with an `index.txt`, and checks each one's PM4 packet lengths add up. [umr](https://gitlab.freedesktop.org/tomstdenis/umr) decodes one with `umr -f <asic> -df <file>.bin`, with an ASIC of the card's generation from umr's database, as `yellow_carp` for GFX10.3 or `navi48_vm_test` for GFX12.

The run folder is in memory on most systems and goes at logout or a reboot, and the next start of the VM writes a new `ib-dump-latest.bin` within seconds, so copy the files out before either. Nothing is copied with the variable unset. With it set, the daemon holds at most 24 MiB of copies, and keeps at most four hung sets; take them out to keep more.
