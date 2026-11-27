# Contributing

## Getting going

The workspace builds on Linux and on Windows. Each builds the host for the system it's on, and the other system's crates build empty, so `cargo test` works on both. The GTK app is the exception, and only builds on Linux. No GPU and no VM are needed for any of this.

### On Linux

You need Rust 1.88 or newer. `cargo clippy` needs 1.92 and the app's development files below.

```sh
cargo test
cargo clippy --workspace --all-targets --all-features
cargo fmt --check
cargo bench -p guesthouse-core
```

`linux/README.md` says how to test what goes into a Linux guest, and the device in the QEMU patch.

The desktop app in `crates/app` also needs GTK 4 and libadwaita's development files, so `cargo test` leaves it out. Build it with `cargo build -p guesthouse-app`. That needs GTK 4.16 for HDR. On GTK 4.14 and libadwaita 1.5, as Ubuntu 24.04 has, use `--no-default-features`, and the viewer shows SDR only. `crates/app/page-shots.sh` walks its pages on GTK's Broadway backend and saves a picture of each, with no desktop and no GPU.

To try the sockets by hand, `cargo run -p guesthouse-daemon --features stand-in` answers messages with a pretend GPU behind them. It's for poking at, and it won't run a guest.

A real VM also needs QEMU with the patch in `qemu/`, which `qemu/README.md` says how to build.

### On Windows

You need Windows 11, Rust 1.95 or newer, as windows-reactor asks for it, and Visual Studio's C++ build tools.

```powershell
cargo test
cargo build --release -p guesthouse-service -p guesthouse-host-windows
cargo build --release -p guesthouse-app-windows
```

`cargo test` leaves the app out here too. The app's first build downloads the Windows App Runtime from NuGet. The app does nothing to a machine itself and only asks the service, so to make or start one, turn on Virtual Machine Platform and run `.\target\release\guesthouse-service.exe run` in an elevated prompt. A build with `--features preview` shows the window with made-up machines and no service. `scripts\windows-setup.ps1` packs it all into the installer. [docs/windows-host.md](docs/windows-host.md) has the details.

The checks on pull requests run on Linux only, so build a Windows change on Windows before you send it.

## Legal rules

These matter more than anything else here, because text inside code is something `.gitignore` can't catch.

- Notes, findings and research stay out of this repo, and so does anything quoted from them.
- Nothing copied from a GPU vendor's or Microsoft's drivers, binaries or private files: no data, tables or text. What comes from code they've published as open source is listed in `THIRD-PARTY.md` with its licence, and nothing goes in without that line.
- A protocol field the code needs goes in as a named constant or type, described in plain words, saying what it does.
- Microsoft's public Linux driver for this protocol is GPL-2. You may read it to understand the protocol. Don't copy its code, comments or structure text, and name things in your own words.
- The QEMU patch in `qemu/` keeps QEMU's licence, GPL-2.0-or-later.
- No secrets, and no paths from your own machine.
- Everything here is GPL-2.0-or-later, and that includes what you add. `THIRD-PARTY.md` lists the exceptions.
- Send only what you wrote yourself, or open source code under a licence that fits GPL-2.0-or-later, with its line in `THIRD-PARTY.md`.
- Nothing leaked or confidential, and nothing an NDA or an agreement with your employer covers.
- If you aren't sure about something, open an issue and ask before you write it.

## The contributor licence agreement

Your first pull request goes in once you've signed the [contributor licence agreement](CLA.md), by adding a file with your line to [CLA/signers](CLA/signers) in that pull request, as [CLA/README.md](CLA/README.md) says. You keep your copyright. The agreement gives RUXN LLC a licence to your work that isn't tied to the GPL, and RUXN LLC keeps publishing your work under GPL-2.0-or-later. A check on the pull request looks for your file.

## Recordings

Recordings hold guest data. They never go in git. Keep them in `/captures/`, which is ignored.

## Code

- Small modules with one job. A crate's top comment should say what that job is.
- Name your numbers. A bare offset or size should be a constant, unless the test or the line beside it makes it obvious.
- Comments say why. If a comment says what the next line does, delete it.
- The message path shouldn't allocate or lock. Measure first, though: `core/tests/no_allocation.rs` and the benchmark will tell you, and a clever change needs a number behind it.
- Dependencies are few, and each one has a comment saying why it's there.
- `unsafe` lives in `daemon`, in the parts of the backends that talk to the kernel or to memory the GPU reads, in `hcs`, whose every job is a call into Windows, and in the Windows parts of `jail` and of `service-windows` (`src/windows/`), which talk to the kernel's security and to the service manager, and in `relay` (`src/signal.rs`, a signal handler that only sets a flag), and in `installer-windows` (`src/system.rs` and the pages' window procedure), which talks to the shell, the registry and the service manager, with a `SAFETY` comment on each block. There are two exceptions in `app`: `viewer/import.rs`, as GTK's call for a texture on a dmabuf is `unsafe`, and `main.rs`, which sets an environment variable before GTK starts. The Windows app keeps its own in `app-windows/src/system.rs`, for the few calls WinUI doesn't make: the file dialog, the clipboard, and cancelling a read on the pipe, and in `app-windows/src/console.rs`, which hosts Windows' Remote Desktop control and RdpBase's pipe connector, through COM interfaces declared by hand, and in its example, which takes that file as it is and asks which process serves a pipe. Two tests have it too: `core/tests/no_allocation.rs` and `backend-intel/tests/quiet_path.rs` count allocations, and a global allocator has to be `unsafe`.

## Keeping the layers apart

`core` must never know about a vendor, and a backend must never reach into `core`. If a backend needs something, add a method to `HostServices`, and if the core needs a vendor's answer, add one to `Backend`. See [ARCHITECTURE.md](ARCHITECTURE.md).

The hosts follow the same rule, whether it's Linux, Windows or the Mac host. Before you build a feature for one host, read how the other two do the same job, even if one's code lives outside this repository, and reuse their names and shapes. Logic with no OS or hypervisor calls belongs in a crate every host builds on, not in a host's own crate. Don't rewrite a working host to fit: move one piece at a time, in a commit that changes no behaviour. See "What the hosts share" in [ARCHITECTURE.md](ARCHITECTURE.md).

## Behaviour

The prototype is the reference, bugs included, until the replay tool has proved the port. If you find a bug in its behaviour, keep the behaviour and write it down. Fixing it comes later, as a change of its own, once a recording can show what the fix changes.

## Tests

Each handler has tests in `core/src/session/tests.rs`, which send whole messages through a `Session` with the stand-in backend. Name a test after what must be true, like `fences_only_go_up`.

`core/tests/handler_fuzz.rs`, and the same in `backend-nvidia/tests/boot.rs` on the fake RM, send random messages and fail on any panic. `FUZZ_ROUNDS=20000000 FUZZ_SEED=7 cargo test --release -p guesthouse-core --test handler_fuzz` runs one for longer.

`tests/install/run.sh` tests `install.sh` and `uninstall.sh` with stand-ins for sudo, curl and the package manager. It needs bubblewrap, as each test runs with everything but its own folder read-only. `tests/install/real-run.sh` does a whole install and uninstall with a real release's files, in a fake HOME, and `tests/install/in-containers.sh` runs one on each distro image you already have. `tests/install/real-in-containers.sh` installs a real release on Ubuntu 24.04 and Debian 13, the oldest systems it runs on.

## Commits

Small steps, each of which builds and passes its tests. Say what changed and why, in a line a stranger can follow.

A change reaches `main` through a pull request. It runs `cargo fmt --check`, clippy with warnings as errors, the tests, shellcheck on the shell scripts and PSScriptAnalyzer on the PowerShell ones, and all of them have to pass.
