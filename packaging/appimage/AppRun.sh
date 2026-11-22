#!/bin/sh
# The AppImage starts here. It carries one release, unpacked in payload/, and install.sh from that
# release puts it where every install goes, ~/.local/lib/guesthouse. The app then runs from there, as
# after any install. It can't run from in here: this folder goes away when the AppImage stops, and a VM's
# QEMU has to outlive the app.
#
#   guesthouse-installer-VERSION.AppImage
#                                       installs or updates when this release is newer than what's
#                                       there, then opens the app. From a terminal it installs in it.
#                                       Otherwise the app's own window shows the plan and asks, and a
#                                       window asks for root's password if a part needs it
#   guesthouse-installer-VERSION.AppImage install [--dry-run]
#                                       the install on its own, in this terminal
#   guesthouse-installer-VERSION.AppImage uninstall
#                                       uninstall.sh of what's installed
#
# Afterwards the app is in the desktop's apps, and the AppImage isn't needed. The app updates itself.
set -eu

here=$(dirname "$(readlink -f "$0")")
payload=$here/payload
installed=$HOME/.local/lib/guesthouse
current=$installed/current
# What the terminal is asked to run: the AppImage's own file, or this script when it's been extracted.
self=${APPIMAGE:-$here/AppRun}
TAB=$(printf '\t')
# What the loader exits with when a program's libraries aren't there, or are too old for it.
CANT_LOAD=127
# What the installer window exits with when someone chose a terminal. installer.rs says the same.
IN_A_TERMINAL=75

say() {
    printf '%s\n' "$*"
}

release_version() {
    sed -n 's/^version=//p' "$payload/RELEASE" | head -n 1
}

installed_version() {
    [ -f "$installed/manifest" ] || return 0
    awk -F "$TAB" '$1 == "version" { print $3; exit }' "$installed/manifest"
}

# Whether version $1 is the same as $2 or newer.
at_least() {
    [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$1" ]
}

open_app() {
    GUESTHOUSE_PROGRAM=$current exec "$current/guesthouse" "$@"
}

# Started on its own, so closing the terminal the install ran in doesn't take the app with it.
open_app_apart() {
    GUESTHOUSE_PROGRAM=$current setsid -f "$current/guesthouse" >/dev/null 2>&1 </dev/null
}

install_here() {
    sh "$payload/install.sh" "--payload=$payload" "$@"
}

# On a PC with no GTK or libadwaita yet, the window takes the AppImage's own, which are Ubuntu 24.04's.
# GIO and GTK are kept from the PC's own modules, which were built for other versions of them. The
# installer keeps all of this from what it starts, as the install adds the PC's own GTK.
with_bundled_gtk() {
    libs=$here/installer-libs
    loaders=$(mktemp "${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/guesthouse-loaders.XXXXXX")
    sed "s|@INSTALLER_LIBS@|$libs|g" "$libs/loaders.cache" >"$loaders"
    result=0
    LD_LIBRARY_PATH=$libs/lib GDK_PIXBUF_MODULE_FILE=$loaders GIO_MODULE_DIR=$libs/gio-modules GTK_IM_MODULE=simple \
        GUESTHOUSE_BUNDLED_GTK=1 GUESTHOUSE_INSTALL_FROM=$payload "$payload/guesthouse-no-hdr" || result=$?
    rm -f "$loaders"
    return "$result"
}

# The app from inside the AppImage, as the installer: GUESTHOUSE_INSTALL_FROM tells it so. The build
# with HDR needs GTK 4.16, so the other one is tried when it can't load, and then the AppImage's own
# GTK. If even that can't load, a terminal install is what's left.
open_installer() {
    for program in guesthouse guesthouse-no-hdr; do
        status=0
        GUESTHOUSE_INSTALL_FROM=$payload "$payload/$program" || status=$?
        [ "$status" -eq "$CANT_LOAD" ] || break
    done
    if [ "$status" -eq "$CANT_LOAD" ] && [ -f "$here/installer-libs/loaders.cache" ]; then
        status=0
        with_bundled_gtk || status=$?
    fi
    case $status in
        "$CANT_LOAD" | "$IN_A_TERMINAL") open_terminal "$self" install-then-open ;;
        *) exit "$status" ;;
    esac
}

# A terminal opened for this closes when the app starts, and stays on a failure until it's read.
install_then_open() {
    if install_here; then
        open_app_apart
        return 0
    fi
    printf '\nNothing more happens. Press Enter to close this.'
    read -r _ || true
    return 1
}

# xdg-terminal-exec is the standard way to ask, and the rest are the terminals distros ship with, each
# with its own way of being given a command.
open_terminal() {
    for terminal in xdg-terminal-exec ptyxis kgx gnome-terminal konsole xfce4-terminal mate-terminal tilix \
        x-terminal-emulator alacritty kitty foot wezterm xterm; do
        command -v "$terminal" >/dev/null 2>&1 || continue
        case $terminal in
            xdg-terminal-exec | kitty | foot) exec "$terminal" "$@" ;;
            ptyxis | kgx | gnome-terminal) exec "$terminal" -- "$@" ;;
            xfce4-terminal | mate-terminal) exec "$terminal" -x "$@" ;;
            wezterm) exec "$terminal" start -- "$@" ;;
            *) exec "$terminal" -e "$@" ;;
        esac
    done
    message="Guesthouse needs a terminal to install in, and there isn't one here. Run this in one: \"$self\" install"
    say "$message" >&2
    command -v notify-send >/dev/null 2>&1 && notify-send -a Guesthouse "Guesthouse" "$message"
    exit 1
}

main() {
    case ${1:-} in
        install)
            shift
            install_here "$@"
            return
            ;;
        install-then-open)
            install_then_open
            return
            ;;
        uninstall)
            [ -f "$current/uninstall.sh" ] || {
                say "Guesthouse isn't installed." >&2
                exit 1
            }
            exec sh "$current/uninstall.sh"
            ;;
    esac
    have=$(installed_version)
    if [ -n "$have" ] && at_least "$have" "$(release_version)" && [ -x "$current/guesthouse" ]; then
        open_app "$@"
    fi
    if [ -t 0 ] && [ -t 1 ]; then
        install_here && open_app "$@"
        exit 1
    fi
    open_installer
}

main "$@"
