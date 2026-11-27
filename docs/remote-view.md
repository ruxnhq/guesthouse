# Remote view

Remote view lets another device on your network see and use a Windows machine through remoteman's viewer: a laptop on the sofa, a phone, another PC. It is a preview, and it is off for every machine until you switch it on.

The machine's own window stays as it is. remoteman's host runs in Windows beside Guesthouse's display driver, and a device that connects gets a screen of its own.

## What it needs

- A Windows machine made by Guesthouse, with its agent working and an up-to-date display driver. An older driver's take-over script would take remoteman's screen for Guesthouse's, so the switch says to update the driver first.
- remoteman's viewer on the other device, on the same network as this PC.

## Switching it on

Open the machine's settings, go to the Network page and turn on Allow viewing from other devices, or run `guesthouse-vm set NAME remote-view on`. A running machine gets remoteman's host at once, and a stopped one at its next start. The first time, Guesthouse downloads remoteman's host installer, checked against the version it was built for, and installs it in Windows. That makes the machine's picture blink for about two seconds. It is only installed again when that version changes. From remoteman 0.1.9 its host looks for updates by itself. Guesthouse switches that off in Windows (`set updates off`) and brings a newer remoteman itself, when Guesthouse is updated. If that doesn't take, it's written in the log and tried again at the next start, and viewing works anyway.

The machine gets a port of its own on this PC, from 7792 to 7899, which stays the same. Guesthouse announces the machine on the network under its name, so remoteman's viewer lists it.

## Pairing a device

Press Pair a New Device. In remoteman on the other device, add a computer and enter the address shown, or pick the machine from the list. The device waits, and its name and a six-digit code appear in Guesthouse. Check that the code matches the one on the device, then press Accept. Pairing closes after one device, or when its time is up.

A new device can see the screen and use the mouse and keyboard. Its clipboard and file transfers are off until you switch them on for it in the list of devices. Remove takes a device off the list, and it has to pair again.

## Switching it off

Turning the switch off closes pairing, turns every device away and stops listening on the network. Paired devices stay paired for when it is switched on again.

## What to know

- Only devices you accepted here can connect. Anyone on the same network can ask to pair while pairing is open.
- While a device shows the machine on its screen only (remoteman's "only" placement), the machine's own window here has no picture until that device leaves.
- Windows in the machine doesn't lock when a device connects. A paired device lands on the open desktop.
