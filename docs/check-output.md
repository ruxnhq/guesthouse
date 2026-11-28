# What `guesthouse-check` prints

`guesthouse-check` is what `install.sh` asks about this PC, and what scripts can ask too. It changes nothing and opens no device. Its lines are a stable format. They only change in a release whose notes say so, and a test in `linux/app-core/src/bin/check/linux.rs` holds them exactly, so a change would break CI first.

Every line is split by tabs, with the line's name first.

    check<TAB>state<TAB>title<TAB>found<TAB>fix
    gtk<TAB>version
    gpu<TAB>yes|no
    amd<TAB>yes|no
    intel<TAB>yes|no

- `check` comes once for each thing looked at, with five fields. The state is `ok`, `warning` or `failed`. The title says what was looked at, found says what was there, and fix says what to do, and is empty when there is nothing to do. A tab or a line break inside a field is printed as a space, so a line always has five fields.
- `gtk` comes only when there is a GTK on the PC, with its version, so the script can choose the app that runs on it.
- `gpu`, `amd` and `intel` always come, in that order, after the `check` and `gtk` lines. `gpu` says whether a GPU can be given to a VM, `amd` whether there is an AMD card that can be a Linux guest's GPU, and `intel` whether there is an Intel card Guesthouse's daemon is written for.

New kinds of line may be added after these, and a reader should skip a line whose name it doesn't know. A name, a field's place or a state word that exists today stays as it is.

`guesthouse-check --firmware` prints one `firmware` line and nothing else, and is not part of this promise. It is for tests.

The exit code is 0 when the lines were printed, and 1 when the check couldn't start.
