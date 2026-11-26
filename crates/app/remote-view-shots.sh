#!/bin/bash
# Pictures of the Remote View group in each state, on GTK's Broadway backend, which draws in software:
# nothing appears on the desktop and nothing touches the GPU. Usage: remote-view-shots.sh FOLDER
# Needs a build first: cargo build -p guesthouse-app --example remote_view_shots
set -e
FOLDER=$(realpath -m "${1:?usage: remote-view-shots.sh FOLDER}")
HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
PROGRAM=${CARGO_TARGET_DIR:-$HERE/../../target}/debug/examples/remote_view_shots
BROADWAY_DISPLAY=${SHOTS_DISPLAY:-:51}
BROADWAY_PORT=${SHOTS_PORT:-28951}
mkdir -p "$FOLDER" "$FOLDER/.scratch/home"
RUNTIME=$(mktemp -d)
trap 'rm -rf "$RUNTIME" 2>/dev/null || true' EXIT
env -i HOME="$FOLDER/.scratch/home" PATH=/usr/bin XDG_RUNTIME_DIR="$RUNTIME" \
  XDG_CONFIG_HOME="$FOLDER/.scratch/home/.config" XDG_DATA_HOME="$FOLDER/.scratch/home/.local/share" \
  GDK_BACKEND=broadway BROADWAY_DISPLAY=$BROADWAY_DISPLAY GSK_RENDERER=cairo GTK_A11Y=none \
  dbus-run-session -- bash -c "
    gtk4-broadwayd --address 127.0.0.1 --port $BROADWAY_PORT $BROADWAY_DISPLAY >'$FOLDER/.scratch/broadway.log' 2>&1 &
    daemon=\$!
    sleep 1
    '$PROGRAM' '$FOLDER'
    code=\$?
    kill \$daemon
    exit \$code"
