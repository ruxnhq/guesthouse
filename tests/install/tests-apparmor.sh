# The rule root adds to swtpm's AppArmor profile, so a Windows machine's virtual TPM can write in the
# run folder. The stand-in sudo runs as the test's user, and the profile's files are in the test's folder.

SWTPM_RULE_LINE='owner /run/user/[0-9]*/guesthouse/** rwk,'
SWTPM_SOCKET_LINE='unix (send, receive) type=stream addr=none,'

swtpm_local() {
    printf '%s' "$T/system/apparmor.d/local/usr.bin.swtpm"
}

# AppArmor on, and swtpm's profile as Ubuntu's package leaves it: an empty local file.
give_apparmor() {
    mkdir -p "$T/system/apparmor.d/local"
    printf 'lockdown,capability,landlock,yama,apparmor,bpf' >"$T/system/lsm"
    printf 'profile swtpm /usr/bin/swtpm {\n  #include <local/usr.bin.swtpm>\n}\n' >"$T/system/apparmor.d/usr.bin.swtpm"
    : >"$(swtpm_local)"
}

t_root_adds_the_rule_swtpm_s_profile_lacks() {
    release 1.0
    give_everything
    give_apparmor
    install_version 1.0
    check_exit 0
    check_said "It lets swtpm, the virtual TPM a Windows machine needs, write in your run folder"
    grep -qxF "$SWTPM_RULE_LINE" "$(swtpm_local)" || fail "the rule isn't in the profile's local file"
    grep -qxF "$SWTPM_SOCKET_LINE" "$(swtpm_local)" || fail "the socket rule isn't in the profile's local file"
    check_log "apparmor_parser -r $T/system/apparmor.d/usr.bin.swtpm"
    grep -q "^apparmor-rule" "$H/.local/lib/guesthouse/manifest" || fail "the manifest doesn't list the rule"
    : >"$T/log"
    install_version 1.0
    check_said "is the latest release"
    check_not_log "sudo"
    [ "$(grep -cxF "$SWTPM_RULE_LINE" "$(swtpm_local)")" -eq 1 ] || fail "the rule is there more than once"
}

# An install from before the socket rule has the first rule and its comment. Only the socket rule goes in.
t_a_rule_from_before_gets_the_socket_rule_beside_it() {
    release 1.0
    give_everything
    give_apparmor
    printf '%s\n' '# Guesthouse: the virtual TPM keeps its log and socket in the run folder.' "$SWTPM_RULE_LINE" >"$(swtpm_local)"
    install_version 1.0
    check_exit 0
    grep -qxF "$SWTPM_SOCKET_LINE" "$(swtpm_local)" || fail "the socket rule isn't in the profile's local file"
    [ "$(wc -l <"$(swtpm_local)")" -eq 3 ] || fail "a line went in twice: $(cat "$(swtpm_local)")"
    check_log "apparmor_parser -r"
}

t_running_the_same_version_again_adds_a_missing_rule() {
    release 1.0
    give_everything
    install_version 1.0
    check_exit 0
    give_apparmor
    install_version 1.0
    check_exit 0
    check_not_said "is the latest release"
    grep -qxF "$SWTPM_RULE_LINE" "$(swtpm_local)" || fail "the rule isn't in the profile's local file"
}

t_with_no_apparmor_or_no_profile_nothing_is_added() {
    release 1.0
    give_everything
    give_apparmor
    printf 'capability,landlock,selinux' >"$T/system/lsm"
    install_version 1.0
    check_exit 0
    check_said "Nothing here needs root."
    [ ! -s "$(swtpm_local)" ] || fail "the local file was written"

    new_test "${FUNCNAME[0]}-no-profile"
    release 1.0
    give_everything
    give_apparmor
    rm "$T/system/apparmor.d/usr.bin.swtpm"
    install_version 1.0
    check_exit 0
    check_said "Nothing here needs root."
}

# swtpm's profile comes with its package, so root looks for it once the package is in.
t_root_looks_for_the_profile_after_installing_swtpm() {
    release 1.0 windows
    give_everything
    rm "$T/bin/swtpm"
    give_apparmor
    rm "$T/system/apparmor.d/usr.bin.swtpm"
    fake_os ubuntu debian
    install_version 1.0
    check_exit 0
    check_said "It lets swtpm, the virtual TPM a Windows machine needs, write in your run folder"
    # The stand-in package manager brings no profile, so root finds none and leaves the file alone.
    [ ! -s "$(swtpm_local)" ] || fail "the local file was written with no profile"
    ! grep -q "^apparmor_parser" "$T/log" || fail "the profile was loaded with no profile there"
}

t_uninstall_takes_the_rule_out_as_root() {
    release 1.0
    give_everything
    give_apparmor
    printf '# Someone else'"'"'s rule\nowner /srv/tpm/** rwk,\n' >"$(swtpm_local)"
    install_version 1.0
    check_exit 0
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_said "The install added a rule to swtpm's AppArmor profile"
    ! grep -qF "guesthouse" "$(swtpm_local)" || fail "the rule is still there"
    ! grep -qxF "$SWTPM_SOCKET_LINE" "$(swtpm_local)" || fail "the socket rule is still there"
    ! grep -qF "Guesthouse" "$(swtpm_local)" || fail "its comment is still there"
    grep -qxF 'owner /srv/tpm/** rwk,' "$(swtpm_local)" || fail "someone else's rule went too"
    check_gone "$(swtpm_local).new"
    [ "$(grep -c "^apparmor_parser -r" "$T/log")" -eq 2 ] || fail "the profile wasn't loaded again"
}
