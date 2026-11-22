# --payload: what the AppImage runs, with the release already unpacked beside the script.

payload_tree() {
    printf '%s' "$T/releases/tree-$1/guesthouse-$1-x86_64"
}

t_a_payload_folder_installs_with_no_download() {
    release 1.0 windows
    give_everything
    tree=$(payload_tree 1.0)
    # Nothing to download, so a script that tried would fail.
    rm -rf "$T/releases/v1.0"
    chmod -R a-w "$tree"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$tree/install.sh" "--payload=$tree"
    chmod -R u+w "$tree"
    check_exit 0
    check_not_said "Downloading"
    root=$H/.local/lib/guesthouse
    [ "$(readlink "$root/current")" = 1.0 ] || fail "current isn't 1.0"
    check_there "$root/1.0/qemu/build/qemu-system-x86_64"
    check_there "$root/1.0/guest/setup.ps1"
    check_there "$H/.local/share/applications/io.github.xnixy.Guesthouse.desktop"
    [ -z "$(find "$root" -name '.incoming*')" ] || fail "staging was left behind"
}

t_a_payload_folder_updates_an_older_install() {
    release 1.0
    release 1.1
    give_everything
    install_version 1.0
    tree=$(payload_tree 1.1)
    answering 'yes\n' "$SHELL_UNDER_TEST" "$tree/install.sh" "--payload=$tree"
    check_exit 0
    [ "$(readlink "$H/.local/lib/guesthouse/current")" = 1.1 ] || fail "current isn't 1.1"
}

t_a_payload_folder_never_goes_backwards() {
    release 1.0
    release 1.1
    give_everything
    install_version 1.1
    tree=$(payload_tree 1.0)
    answering 'yes\n' "$SHELL_UNDER_TEST" "$tree/install.sh" "--payload=$tree"
    check_exit 1
    check_said "which is newer than 1.0"
    [ "$(readlink "$H/.local/lib/guesthouse/current")" = 1.1 ] || fail "current isn't 1.1"
}

t_a_payload_folder_is_only_what_it_names() {
    release 1.0
    give_everything
    tree=$(payload_tree 1.0)
    answering 'yes\n' "$SHELL_UNDER_TEST" "$tree/install.sh" "--payload=$tree" --update
    check_exit 1
    check_said "can't go with --update"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$tree/install.sh" "--payload=$T/nowhere"
    check_exit 1
    check_said "there's no folder called"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$tree/install.sh" "--payload=$T"
    check_exit 1
    check_said "has no RELEASE file"
    check_gone "$H/.local/lib/guesthouse/current"
}
