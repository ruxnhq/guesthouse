# Reporting a bug

In the app, choose Show Log in the menu and press Send Report. A failure's See Details button opens the same log. Pick the machine and the kind of problem, say what went wrong, and look at the list of what goes in. Nothing is sent until you press Send. If the app won't open, `guesthouse-report` does the same from a terminal. It's in the same folder as the app, which is `~/.local/lib/guesthouse/current` after `install.sh`:

```sh
~/.local/lib/guesthouse/current/guesthouse-report                     # asks before it sends
~/.local/lib/guesthouse/current/guesthouse-report --save-only         # only writes the file
~/.local/lib/guesthouse/current/guesthouse-report --machine NAME --kind crashed --no-guest-serial --no-guest-logs
```

A report holds the app's log, that machine's logs from its last runs and its settings (the app keeps a copy of each machine's last run, so a reboot doesn't lose them), what Guesthouse's checks say of this PC, its system, GPU and driver, the kernel's lines about GPUs and KVM if you may read them, and a recent crash's stack trace. It never holds a core file, a memory dump or a disk. From the guest it has its serial log and, while a Windows machine is running, what Windows says: Guesthouse's own guest logs, display events, the adapter and its driver, the build, and whether a crash dump was written, by name, time and size. You can leave either out. It also has the few answers to Guesthouse's own scripts that the app's log keeps. The fullest report comes with the machine running, after the problem has happened. Your user name, host name and home folder are taken out where they're found, but logs are free text, so read it first if that matters to you. It goes to me, it's read by me and my automation tools, and it's deleted after 30 days. Sending gives you an ID, and the app can open an issue filled in with it.
