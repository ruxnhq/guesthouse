# Windows on an Intel card

Intel support is experimental. On an Arc B580, on Linux's `xe` driver, the card draws Windows' desktop, and programs reach it through Direct3D 11 and 12, Vulkan and OpenCL. One tester's card is all it has run on, and it has rough edges: frames reach the screen by a copy, video in a browser is new and untried, and work on the card can hang the card or make it reset. If your Linux desktop is on the same card, that can freeze or restart your Linux session, so save your work before you start a machine. The cards offered are the ones Intel's Windows driver 32.0.101.9175 covers and `xe` takes by default: Lunar Lake (Arc 130V and 140V), Battlemage (Arc B570, B580 and the Pro cards), and Panther Lake and Wildcat Lake. Only the Arc B580 has been read on a real PC so far; the others are set up from Intel's and the kernel's open tables and are untested. The daemon tells a card's generation from what the card says, so a card of these generations is offered whatever its PCI id.

Alchemist (the Arc A-series), Meteor Lake and Arrow Lake are known too, but `i915` drives them unless the kernel is told otherwise, and a card that `i915` drives is not offered: the daemon works through `xe`. The setup check says so and gives the kernel options that hand the card to `xe`. `xe` on those cards is newer than `i915` and less tried.

## What it's meant to do

The daemon built with the `intel` feature answers Intel's Windows driver in the guest, so that it loads and makes its devices, contexts, queues and memory, and it runs the guest's work on the card, as "Work on the card" below says. Where the card doesn't pass its own test at the start, nothing is run and Windows draws its desktop in software. Either way the log matters most. Every exchange that is Intel's own gets a line in `daemon.out` starting with `intel`, and with `GUESTHOUSE_VERBOSE` set the log holds the whole of each one too, so a bug report from one run says how far the guest got and where it stopped. The app turns that on for a machine on an Intel card, and the daemon keeps the log under 64 MiB.

It only takes Intel's driver 32.0.101.9175 in the guest, as the data it passes around changes from one release to the next.

## Trying it

You need one of those cards and Linux with the `xe` driver on it. It takes a Windows 11 ISO and about an hour.

1. Download Intel's graphics driver 32.0.101.9175 for Arc from Intel's site. The file is `gfx_win_101.9175.exe`, about 860 MB. Other releases are refused.
2. Make sure `7z` can unpack it. Intel's installer keeps its files in a RAR archive, and many distributions' `7z` leave RAR out, so it lists the file and then fails to unpack it. The app says so when that happens. 7-Zip's own Linux build from 7-zip.org works. Put it on your PATH as `7z`.
3. In the app, choose New Machine and pick your Windows ISO. Pick the Intel card. The dialog asks for Intel's installer. Choose the file, and it takes the drivers out of it. This reads the whole archive, so it takes a few minutes and about 1.8 GB.
4. Create the machine and let it install. The install is the same as for the other cards. It checks at the end that the driver's files reached Windows, and puts the loaders from Intel's INF in Windows' system folders (see "What it changes in Windows").
5. Start the machine. The app starts the daemon with its full log on. Wait a minute or two after the desktop comes up, even if it's black. If Windows comes up without its GPU, the app starts it once more on the software desktop; if the GPU is still missing, it says so and leaves the VM running on Windows' basic display, so the next step still works. A bug report from the app then takes the files of step 6 too, with the guest's account and computer names taken out.
6. Run this in a terminal. `MACHINE` is the name of the machine's folder in `~/.local/share/guesthouse/machines`:

   ```sh
   ~/.local/lib/guesthouse/current/guesthouse-task dxdiag MACHINE
   ```

   It works through the app's agent, so nobody needs to look at the screen, in two parts. First a script of the app's, `api-check.ps1`, tries each API against the card: Direct3D 12 and 11 on every adapter, OpenCL, Level Zero, Vulkan and OpenGL (on a window that is never shown). Each API runs in a PowerShell process of its own with a one minute limit, so a hang or a crash in one costs only its own lines, and the report says how each ended. Direct3D and OpenCL run a second time in the 32-bit PowerShell, as the 32-bit drivers are a separate set. Nothing is installed or downloaded for this. Then Windows' own `dxdiag` runs, which makes Direct3D devices on the card too. The first part waits up to ten minutes and the second up to five. When it's done it puts four files in `test-report` in the machine's folder: `api-check.txt`, `dxdiag.txt`, `daemon.out`, and `intel-probe.txt`, which says what your host's driver knows about the card. `guesthouse-task api-check MACHINE` is the same step under another name.
7. Open an issue and attach those four files. If step 6 says the agent didn't answer, or dxdiag never finished, the guest has probably stopped. Then attach `daemon.out` from `$XDG_RUNTIME_DIR/guesthouse/run` (usually `/run/user/1000/guesthouse/run`) and say so, with `api-check.txt` if it has anything in it. `daemon.out` is the most useful of them.

`api-check.txt` starts with the loader files Windows has (OpenCL.dll, vulkan-1.dll, ze_loader.dll and so on, in System32 and SysWOW64), then has a section per API in the form of the call (`->`), its return code (`<-`), and any names returned:

```
-- opencl, 64-bit PowerShell --
  -> clGetPlatformIDs
  <- clGetPlatformIDs = 0, 1 platform(s)
     clGetPlatformInfo(NAME) = 0 "Intel(R) OpenCL Graphics"
     -> clCreateContext
     <- clCreateContext = -33 (null)
  finished
  result: exit code 0 (0x00000000)
```

A line with `->` and no `<-` after it is the call that hung or crashed the process, and `result:` says whether it exited, crashed (an exit code like 0xC0000005) or timed out. The script's part ends with `api-check done` and a summary of the furthest layer each API reached. After it the app adds a line per API from the stage in `daemon.out`: "its work runs on the card", "withheld by stage=..." when the stage finishes its work without running it (its devices are still made, so that is as far as it can get), or "FAILED" when it reached nothing, which the stage doesn't explain. On the software desktop the Direct3D line also says that Direct3D 9 to 12 are refused.

You can shut the machine down afterwards. If you build from source, `guesthouse-task` and `intel-probe` are in `target/release`, and `cargo build --release -p guesthouse-app-core -p guesthouse-daemon --features guesthouse-daemon/intel` makes the daemon.

## A desktop you can use

A start tries the hardware desktop: Windows' desktop compositor and Intel's own drivers on the card, with the Guesthouse display driver showing the picture. That needs the guest's work to run on the card (see the next part). Where it doesn't, because the card is not on `xe`, its self-test failed, or work on the card is switched off, the same start gives the software desktop instead and `daemon.out` says why. If the hardware desktop does not come up, the app falls back by itself to the software desktop for that start and the next ones on the same build, and says why in its log and the bug report. On a machine that already has the display driver, the start is done as soon as the picture is up, and the app goes on watching the desktop until Windows has been up two and a half minutes; if the desktop fails in that time, the machine shows as starting again while it moves to the software desktop.

On the software desktop Windows draws in software on its basic display. Intel's render device is switched off in the guest for that start, so programs in the guest have no GPU; the compositor would otherwise keep asking the card for a driver and draw nothing. `GUESTHOUSE_INTEL_SOFTWARE_DESKTOP=1` in the app's environment asks for the software desktop, and `=0` for the hardware one.

The device stays on with the software desktop only when you ask for work on the card, by setting `GUESTHOUSE_INTEL_SOFTWARE_DESKTOP=1` or `GUESTHOUSE_INTEL_RUN_ON_CARD=compute`. Then the screen stays black, because the compositor can't draw, and you reach the guest through the app's agent with `guesthouse-task`. Programs get the card's compute drivers and Vulkan then, but none of Intel's Direct3D drivers: Windows would run a program's Direct3D 11 on Intel's Direct3D 12 driver, and the sign-in screen and Explorer would draw on the card for a compositor that can't show it, fail, and start over. The app's log says "Intel device left on with the software desktop". `compute` is the way to try OpenCL on a machine whose hardware desktop failed. It turns the device back on at that start, and the fallback stays as it was. Whenever the app turns the device back on and it stays on, for this or for the hardware desktop, it restarts Windows once first: Direct3D 12, OpenCL and Level Zero only find the card if it was on when Windows started, while Vulkan and OpenGL find it either way.

What a start gives, by the two switches, whether the machine fell back from the hardware desktop on this build, and the card. "Fell back" means the app's own fallback. "Fell back, asked" means it fell back while `GUESTHOUSE_INTEL_SOFTWARE_DESKTOP=0` was set.

| Run on card | Software desktop | Fell back | Card | Desktop | Intel device | Work on the card |
|---|---|---|---|---|---|---|
| unset or `1` | unset | no | real | hardware, watched, falls back if it fails | on | everything |
| unset or `1` | unset | yes | real | software, drawn | off | none |
| unset | `0` | no, or fell back with nothing set | real | hardware, tried again | on | everything |
| unset | `0` | fell back, asked | real | software, drawn | off | none |
| unset | `1` | any | real | software, black | on | Vulkan and compute |
| `compute` | any | any | real | software, black | on | OpenCL's work only |
| `0`, `listen` or an unknown word | any | any | real | software, drawn | off | none |
| any | any | any | real, but not on `xe` or its self-test failed | software, drawn | off | none |
| any | any | any | recorded | software, drawn | off | none |

When the hardware desktop fails, the app keeps that start's daemon log as `intel-hardware-desktop-daemon.out` beside the fallback in the machine's folder, and a bug report takes both. A big log is cut to its start, its warnings and its end.

## Work on the card

The daemon can run the guest's work on the card through `xe`: real buffers, an address space for each guest program, and a queue behind each of the guest's. It has only run against a stand-in for the driver, never on a real card, so expect it to fail in ways nobody has seen.

At each start the daemon runs the short self-test `intel-probe --run` runs. If a step the guest's work needs fails (the address space, buffers, host pages, binds, the render, compute or copy engine), or the card can't be opened for work, the machine starts as before with nothing run on the card. A failed video engine step, copy instruction, clock reading or cleanup is a warning in `daemon.out`, and work runs all the same. A line in `daemon.out` starting with `intel stage=` says which it is and, if nothing runs, what refused.

`GUESTHOUSE_INTEL_RUN_ON_CARD=0` in the environment the app or `guesthouse-vm` starts the machine from keeps everything off the card. The daemon and the app both read it there, so set it where the machine is started. `GUESTHOUSE_INTEL_RUN_ON_CARD=compute` runs OpenCL's work only. Programs still get Intel's Vulkan, OpenGL and Level Zero drivers and can make devices (not its Direct3D drivers, as the desktop is then in software), but their work is finished without running, so nothing they draw or compute is real. Level Zero is among them because its work can't be told apart yet. A recorded card never runs anything.

A guest program whose work hangs or faults loses its device, as it would on Windows, and the other programs go on. On a card with memory of its own the guest can't take all of it: a quarter of the card, or 1 GiB if that is more, and what your desktop already holds stay out of its reach.

## intel-probe

`intel-probe` prints what the host's `xe` driver says about your card: its PCI ids, memory, engines, GT and topology, the hardware config table, the GuC and HuC versions and the clocks, then the record a guest would be given. It only asks the driver questions. It makes no buffer or queue, so nothing runs on the card and your desktop isn't touched. A release has it beside `guesthouse-task`, and step 6 runs it for you.

```sh
cargo run --release -p guesthouse-backend-intel --bin intel-probe
```

Give it a PCI address, like `0000:00:02.0`, if you have more than one Intel card. If you have one of these cards, or another Intel card on `xe`, and want to help, open an issue with its output.

`intel-probe --run` goes one step further and runs a short self-test on the card, with no guest. It makes an address space, three 64 KiB buffers and 2 MiB of shared memory, runs one tiny batch on each engine class and one copy, reads the engine clock, and frees everything. Each step prints `PASS`, `FAIL` with what the kernel said, or `SKIP`. It takes a few seconds and doesn't touch what your desktop has on the card.

`intel-probe --run --break` adds two steps that break things on purpose, to see the driver recover: a batch that writes where nothing is mapped, and a batch that never ends. They can freeze a desktop on the same card for a moment, or worse if the driver can't recover, so they only run after you type `yes`. Leave them out unless you are asked for them.

## A recorded card

`intel-probe --record card.txt` also writes what it read to `card.txt`, a plain text file with one fact a line. That file can stand in for the card on a PC that doesn't have it, so a run of Intel's driver in a guest can be tried in a lab instead of on your PC. It holds the card's ids, memory sizes, engines, topology, hardware config table, firmware versions and clocks, and the PCI address and kernel version it was read on. Nothing else about your PC.

It is only used when `GUESTHOUSE_INTEL_RECORDED_CARD` names the file, in the environment the app or `guesthouse-vm` and `guesthouse-task` run in. Then the app lists the recorded card beside the PC's own, named "recorded, no card", and the daemon of a machine on it opens no card and no render node: its jail lets it read the recording and nothing of any card. Its log starts with a `RECORDED CARD` warning, and a bug report's host facts say so too. Unset, everything looks for the card itself, as before. With nothing reaching a card, this is for seeing what Intel's driver in the guest asks, not for drawing anything.

## A machine from an earlier test build

Test builds up to 0.6.2-intel.1 left the kernel driver file out, so Windows on a machine they made starts without its GPU. Such a machine needs no new install. The first start on this version says Intel's driver on this PC lacks the file. Give Intel's installer again in the New Machine dialog: pick a Windows ISO and the Intel card, choose `gfx_win_101.9175.exe` where it is asked for, wait for it to be unpacked, and close the dialog. Then start the machine. Once its desktop is up, the app copies the file into Windows and restarts Windows once, and the GPU check runs after that.

## What it changes in Windows

At install, Windows gets the user-mode part of Intel's driver in its host driver store, under the folder name the daemon gives it, and one file of the kernel driver: `igdkmdn64.sys` for a card in the processor or `igdkmdnd64.sys` for one with memory of its own, the card's own. Windows never loads it, as the paravirtual adapter's kernel driver is Windows' own, but the daemon names it as the adapter's driver image, and Windows won't start the adapter unless that file is in the folder. NVIDIA's and AMD's folders carry theirs for the same reason. The rest of the kernel driver, the firmware and Intel's media software don't go in.

It also gets the libraries Intel's INF copies to Windows' system folders, which programs find by name and which lead them to Intel's OpenCL, Vulkan and Level Zero drivers. The app reads the list from the INF when the driver is unpacked (the sections for the card that copy to directory 11 or `10,SysWow64`), leaving out Intel's control library, media libraries and tools such as `vulkaninfo`, and the guest's setup script places them from the store copy. Every start checks them again and puts back any that are missing. A file is only copied when Windows lacks it or has an older one, which is the INF's own rule, so Windows' own `opengl32.dll` stays. For driver 32.0.101.9175 on the Arc 140V:

- `System32`: `vulkan-1.dll` and `vulkan-1-999-0-0-0.dll` (both from `vulkan-1-64.dll`), `OpenCL.dll` (from `Intel_OpenCL_ICD64.dll`), `ze_loader.dll`, `ze_validation_layer.dll`, `ze_tracing_layer.dll`, `ze_intel_gpu_raytracing.dll`.
- `SysWOW64`: `vulkan-1.dll` and `vulkan-1-999-0-0-0.dll` (from `vulkan-1-32.dll`), `OpenCL.dll` (from `Intel_OpenCL_ICD32.dll`), `msvcp140.dll` and `vcruntime140.dll` (from `msvcp140_32.dll` and `vcruntime140_32.dll`, the C++ runtime the 32-bit drivers need). It also gets one registry value, `HKLM\SOFTWARE\INTEL\IGFX\D3D12\EnableTrinity` set to 2, which should make Intel's Direct3D 12 driver load its older module, so a run doesn't depend on a list inside the driver. I haven't confirmed that 2 is the value that does it.
