# App windows

App windows show the programs of a Windows machine as windows of their own on your Linux desktop, without the Windows desktop around them. Notepad opens next to your browser, goes in the task bar under its own name and icon, and can be moved, resized, tiled and closed like any other window. It is off for every machine until you switch it on.

The other way to look at a machine is Show Screen, which is the whole Windows desktop in one window. A machine shows one or the other. Pressing Show Screen closes the app windows. Opening an app while the screen is open says to close the screen first.

## What it needs

- A Windows machine made by Guesthouse, with its agent working. Ubuntu and plain machines don't have it.
- A Wayland session on the host. The picture of each window is handed to the compositor as it comes from the guest, and that takes Wayland. In an X11 session the viewer says so and exits with an error. The machine's own screen still works there.

## Switching it on

Open the machine's settings, go to the Apps page and turn on Show Apps as Their Own Windows. It counts from the machine's next start, so restart a running machine. The same switch from a terminal is `guesthouse-vm set NAME app-windows on`.

The Apps page lists the programs Windows has in its Start menu. Open starts one, and starts the machine first if it is off. The In Launcher switch puts the program in your desktop's menu, as `Notepad (Office PC)` with the program's icon. Show All adds the uninstallers and help files that are left out at first. The machine's card also has Open Another App.

From a terminal, `guesthouse --app NAME PROGRAM` does the same as Open for a program given by its path or name in Windows, such as `notepad.exe`, and `guesthouse --app NAME --start-menu ID` takes a Start menu id as `guesthouse-vm apps NAME` lists them. [The commands](headless.md#one-step-from-a-click-to-a-window) have the rest.

## How it works

A small helper runs in the Windows session. It tells Guesthouse where each program window is, and Guesthouse opens a window on the host for each one. The helper is started through the agent when it is needed. It listens only to the host, and only to a caller that sends the machine's token first. The picture comes from the guest's frames without a copy, and a click, a key or a scroll on the host window goes to the guest's window. Menus and drop-downs are popups of the window they belong to. A host window's size is asked of the guest's window, and the guest's window can ask for a size of its own.

Opening a window starts one viewer for the machine. It stays while the machine runs, even when no window is open, and ends when the machine stops.

## What doesn't work yet

- The secure desktop, which shows for the User Account Control prompt, and the lock screen, show as one window of the whole screen. The programs are left where they are meanwhile and take no input.
- Rounded window corners show dark in the corners.
- Resizing is new and has had little testing.
- It was tried on KDE Plasma with an NVIDIA card and a Windows 11 guest. On GNOME, sway, Hyprland and niri the pictures show, but whether the mouse and keyboard work there has not been checked. COSMIC works in part. X11 is not supported.
- A program that has no Start menu shortcut, such as the Store's Notepad and Calculator, is not listed on the Apps page. Open it by name from a terminal.
