#!/bin/bash
# Tests packaging/proxmox/guesthouse-pve.sh check on made-up hosts, with no root and no Proxmox.
#
# Each test gets a folder with a host of its own in it: the files under /sys, /proc and /etc that the script
# reads, and stand-ins for the programs it runs (pveversion, dpkg-query, apt-cache, apt-mark, QEMU, runuser
# and df), which answer from files in the folder. The host starts as one that has run, and each test changes
# the one thing it's about.
#
# Usage: tests/proxmox/run.sh [TEST...]. SHELL_UNDER_TEST names the shell, sh if not, as Proxmox's sh is dash.
set -u

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$HERE/../.." && pwd)
SCRIPT=$REPO/packaging/proxmox/guesthouse-pve.sh
TEST_ROOT=${TEST_ROOT:-$(mktemp -d)}
SHELL_UNDER_TEST=${SHELL_UNDER_TEST:-sh}

passed=0
failed=0

fail() {
    printf '  FAILED: %s\n' "$*"
    test_failed=yes
}

said() {
    grep -qF -- "$1" "$T/said" || fail "it didn't say: $1"
}

not_said() {
    ! grep -qF -- "$1" "$T/said" || fail "it shouldn't have said: $1"
}

exit_was() {
    [ "$status" -eq "$1" ] || fail "exit $status, not $1"
}

# The host: a 780M on amdgpu, Landlock on, Proxmox's QEMU with GPU-PV and held, one user with Guesthouse,
# and one Windows VM set up by setup-vm.sh.
make_host() {
    R=$T/root
    S=$T/state
    mkdir -p "$R/proc/sys/kernel" "$R/sys/kernel/security" "$R/etc/pve/qemu-server" "$R/etc/guesthouse/proxmox" \
        "$R/etc/modprobe.d" "$R/etc/apt/preferences.d" "$R/etc/apparmor.d/local" "$R/dev/shm" \
        "$R/var/lib/systemd/linger" "$R/usr/lib/x86_64-linux-gnu" "$R/usr/bin" "$S" "$T/bin"
    touch "$R/etc/pve/.version"
    echo 13.8 >"$R/etc/debian_version"
    echo 7.0.14-30-pve >"$R/proc/sys/kernel/osrelease"
    echo 'BOOT_IMAGE=/boot/vmlinuz-7.0.14-30-pve root=/dev/mapper/pve-root ro quiet lsm=landlock,lockdown,yama,integrity,apparmor' >"$R/proc/cmdline"
    echo 'lockdown,capability,landlock,yama,apparmor,ima,evm' >"$R/sys/kernel/security/lsm"
    printf 'MemTotal:       63553536 kB\nMemAvailable:   27000000 kB\nHugePages_Total:       0\nHugepagesize:       2048 kB\n' >"$R/proc/meminfo"
    echo pve >"$R/etc/hostname"
    touch "$R/dev/kvm"
    make_card 0000:c4:00.0 0x1002 0x15bf amdgpu
    printf 'root:x:0:0::/root:/bin/bash\nalice:x:1000:1000::/home/alice:/bin/bash\n' >"$R/etc/passwd"
    printf 'kvm:x:103:alice\nrender:x:104:alice\n' >"$R/etc/group"
    touch "$R/var/lib/systemd/linger/alice"
    make_guesthouse alice
    touch "$R/etc/apparmor.d/usr.bin.swtpm"
    printf '%s\n' '# Guesthouse' 'owner /run/user/[0-9]*/guesthouse/** rwk,' 'unix (send, receive) type=stream addr=none,' \
        >"$R/etc/apparmor.d/local/usr.bin.swtpm"
    printf 'pve-qemu-kvm hi 11.1.3-4+gpupv1\nswtpm ii 0.10.2+pve3\n7zip ii 25.01\ngenisoimage ii 1.1.11-4\nlibvirglrenderer1 ii 1.1.0-2\n' >"$S/packages"
    policy 11.1.3-4
    printf 'name "vmbus-bridge"\nname "hv-timesync"\nname "gpupv-global"\nname "gpupv-vgpu"\nname "gpupv-render"\n' >"$S/devices"
    make_windows_vm 101 1 8
    make_stand_ins
}

make_card() {
    dev=$R/sys/bus/pci/devices/$1
    mkdir -p "$dev/drm/renderD128" "$R/sys/bus/pci/drivers/$4"
    echo 0x030000 >"$dev/class"
    echo "$2" >"$dev/vendor"
    echo "$3" >"$dev/device"
    ln -sfn "../../drivers/$4" "$dev/driver"
}

make_guesthouse() {
    program=$R/home/$1/.local/lib/guesthouse/0.10.0
    mkdir -p "$program/guest"
    ln -sfn 0.10.0 "$R/home/$1/.local/lib/guesthouse/current"
    printf '#!/bin/sh\n' >"$program/guesthouse-vm"
    printf 'version=0.10.0\n' >"$program/RELEASE"
    cat >"$program/guesthouse-check" <<'EOF'
#!/bin/sh
printf 'check\tok\tVirtualization\t/dev/kvm can be opened.\t\n'
printf 'check\tok\tMemory\tUp to 43 GiB.\t\n'
printf 'gtk\t4.18\ngpu\tyes\namd\tyes\nintel\tno\n'
EOF
    chmod +x "$program/guesthouse-vm" "$program/guesthouse-check"
}

# What apt-cache policy says, with the newest of Proxmox's versions given, and the installed one's priority.
policy() {
    installed=$(awk '$1 == "pve-qemu-kvm" { print $3 }' "$S/packages")
    priority=${2:-100}
    cat >"$S/policy" <<EOF
pve-qemu-kvm:
  Installed: $installed
  Candidate: $installed
  Version table:
 *** $installed $priority
        100 /var/lib/dpkg/status
     $1 500
        500 http://download.proxmox.com/debian/pve trixie/pve-no-subscription amd64 Packages
EOF
}

# A VM as setup-vm.sh leaves it: VMID, slot, GiB.
make_windows_vm() {
    cat >"$R/etc/pve/qemu-server/$1.conf" <<EOF
args: -device vmbus-bridge,mmio-base=0x10000000000,mmio-size=0x1000000000 -device hv-timesync -chardev socket,id=gpg,path=/run/user/1000/guesthouse/run-$2/global.sock,reconnect-ms=500 -object memory-backend-file,id=gpio,size=64G,mem-path=/dev/shm/gpupv-window.$1,share=on -device gpupv-global,chardev=gpg,instanceid=711dad3a-73ce-468b-90a9-ede6906841b2,iomem=gpio,iomem-base=0x10000000000,owner=1000 -chardev socket,id=gpv,path=/run/user/1000/guesthouse/run-$2/vgpu.sock,reconnect-ms=500 -device gpupv-vgpu,chardev=gpv,instanceid=7a1e2f30-4c55-4d6e-9f10-0123456789ab,owner=1000 -object memory-backend-file,id=ram0,size=$3G,mem-path=/dev/shm/gpupv-ram.$1,share=on -machine memory-backend=ram0 -device gpupv-render,channel=7a1e2f30-4c55-4d6e-9f10-0123456789ab,hostcaps=1,bus=pcie.0,addr=0x6
balloon: 0
bios: ovmf
cpu: host,flags=-nested-virt
hookscript: local:snippets/guesthouse.sh
machine: pc-q35-11.1
memory: $(($3 * 1024))
name: secret-vm-name
ostype: win11

[before-update]
memory: 4096
EOF
    printf 'user=alice\nslot=%s\nram=%sG\ndaemon_env=\n' "$2" "$3" >"$R/etc/guesthouse/proxmox/$1.conf"
}

make_stand_ins() {
    b=$T/bin
    printf '#!/bin/sh\necho "pve-manager/9.2.32/60a22e26 (running kernel: 7.0.14-30-pve)"\n' >"$b/pveversion"
    cat >"$b/dpkg-query" <<EOF
#!/bin/sh
# -W -f FORMAT PACKAGE: only the format the script asks for.
for p; do pkg=\$p; done
awk -v p="\$pkg" '\$1 == p { print \$2 " " \$3; found = 1 } END { exit !found }' "$S/packages"
EOF
    cat >"$b/dpkg" <<'EOF'
#!/bin/sh
# --compare-versions A lt B, near enough for these versions.
[ "$1" = --compare-versions ] && [ "$3" = lt ] || exit 2
[ "$2" != "$4" ] && [ "$(printf '%s\n%s\n' "$2" "$4" | sort -V | head -n 1)" = "$2" ]
EOF
    printf '#!/bin/sh\ncat "%s/policy"\n' "$S" >"$b/apt-cache"
    printf '#!/bin/sh\ncat "%s/devices"\n' "$S" >"$R/usr/bin/qemu-system-x86_64"
    chmod +x "$R/usr/bin/qemu-system-x86_64"
    printf '#!/bin/sh\nprintf "Filesystem 1024-blocks Used Available Capacity Mounted on\\ntmpfs 31777252 40560 31736692 1%% /dev/shm\\n"\n' >"$b/df"
    cat >"$b/runuser" <<'EOF'
#!/bin/sh
# runuser -u USER -- COMMAND...
shift 3
exec "$@"
EOF
    chmod +x "$b"/*
}

run_check() {
    status=0
    PATH="$T/bin:$PATH" GUESTHOUSE_PVE_ROOT="$T/root" "$SHELL_UNDER_TEST" "$SCRIPT" check "$@" >"$T/said" 2>&1 || status=$?
}

# --- The tests ---

test_a_host_that_has_run_has_nothing_to_put_right() {
    run_check
    exit_was 0
    said "ok       AMD card 1002:15bf at 0000:c4:00.0: driver amdgpu, renderD128. It has run under Proxmox."
    said "ok       pve-qemu-kvm: 11.1.3-4+gpupv1, with the GPU-PV devices."
    said "ok       pve-qemu-kvm, kept: A hold keeps it."
    said "ok       VM 101: Windows with the GPU, 8 GiB, slot 1."
    said "Nothing here stops a VM with the GPU, or Guesthouse making its guest."
    not_said "To put right"
}

test_without_landlock_it_gives_the_line_for_grub() {
    echo 'lockdown,capability,yama,apparmor,ima,evm' >"$R/sys/kernel/security/lsm"
    echo 'BOOT_IMAGE=/boot/vmlinuz root=/dev/mapper/pve-root ro quiet' >"$R/proc/cmdline"
    run_check
    exit_was 1
    said "failed   Landlock: It's off"
    said "Add lsm=landlock,lockdown,yama,apparmor,ima,evm to GRUB_CMDLINE_LINUX_DEFAULT in /etc/default/grub, run update-grub"
}

test_without_landlock_on_systemd_boot_it_names_proxmox_boot_tool() {
    echo 'lockdown,capability,yama,apparmor' >"$R/sys/kernel/security/lsm"
    echo 'root=ZFS=rpool/ROOT/pve-1 boot=zfs lsm=lockdown,yama,apparmor' >"$R/proc/cmdline"
    mkdir -p "$R/etc/kernel" "$R/sys/firmware/efi"
    echo 1234-ABCD >"$R/etc/kernel/proxmox-boot-uuids"
    echo 'root=ZFS=rpool/ROOT/pve-1 boot=zfs' >"$R/etc/kernel/cmdline"
    run_check
    exit_was 1
    said "Add lsm=landlock,lockdown,yama,apparmor to /etc/kernel/cmdline, run proxmox-boot-tool refresh, in place of lsm=lockdown,yama,apparmor"
}

test_without_landlock_on_zfs_with_grub_it_names_grub_and_proxmox_boot_tool() {
    echo 'lockdown,capability,yama,apparmor' >"$R/sys/kernel/security/lsm"
    mkdir -p "$R/etc/kernel"
    echo '1234-ABCD grub' >"$R/etc/kernel/proxmox-boot-uuids"
    run_check
    exit_was 1
    said "Add lsm=landlock,lockdown,yama,apparmor to GRUB_CMDLINE_LINUX_DEFAULT in /etc/default/grub, run proxmox-boot-tool refresh"
}

test_a_card_given_to_vfio_pci_is_named_with_what_gives_it() {
    rm "$R/sys/bus/pci/devices/0000:c4:00.0/driver"
    make_card 0000:c4:00.0 0x1002 0x15bf vfio-pci
    echo 'options vfio-pci ids=1002:15bf,1002:1640' >"$R/etc/modprobe.d/vfio.conf"
    echo 'blacklist amdgpu' >"$R/etc/modprobe.d/pve-blacklist.conf"
    run_check
    exit_was 1
    said "failed   AMD card 1002:15bf at 0000:c4:00.0: driver vfio-pci, renderD128. It's given to vfio-pci, for passthrough."
    said "/etc/modprobe.d/vfio.conf: options vfio-pci ids=1002:15bf,1002:1640"
    said "/etc/modprobe.d/pve-blacklist.conf: blacklist amdgpu"
    said "failed   GPU: There's no AMD, NVIDIA or Intel card with its own driver."
}

test_a_blacklist_that_waits_for_the_next_boot_is_a_warning() {
    echo 'blacklist amdgpu' >"$R/etc/modprobe.d/pve-blacklist.conf"
    echo 'blacklist nvidiafb' >"$R/etc/modprobe.d/nvidiafb.conf"
    run_check
    exit_was 0
    said "warning  AMD card 1002:15bf at 0000:c4:00.0: These would keep amdgpu off it at the next boot: /etc/modprobe.d/pve-blacklist.conf: blacklist amdgpu."
    not_said "nvidiafb"
}

test_another_makers_blacklist_is_no_matter() {
    printf 'blacklist nouveau\nblacklist radeon\n' >"$R/etc/modprobe.d/pve-blacklist.conf"
    run_check
    exit_was 0
    not_said "would keep"
}

test_a_softdep_that_gives_the_card_to_vfio_is_named() {
    rm "$R/sys/bus/pci/devices/0000:c4:00.0/driver"
    make_card 0000:c4:00.0 0x1002 0x15bf vfio-pci
    echo 'softdep amdgpu pre: vfio-pci' >"$R/etc/modprobe.d/vfio.conf"
    run_check
    exit_was 1
    said "Found: /etc/modprobe.d/vfio.conf: softdep amdgpu pre: vfio-pci."
}

test_a_second_card_for_passthrough_is_only_noted() {
    make_card 0000:01:00.0 0x10de 0x2208 vfio-pci
    echo 'options vfio-pci ids=10de:2208' >"$R/etc/modprobe.d/vfio.conf"
    echo 'blacklist nouveau' >"$R/etc/modprobe.d/pve-blacklist.conf"
    run_check
    exit_was 0
    said "note     NVIDIA card 10de:2208 at 0000:01:00.0: driver vfio-pci, renderD128. It's given to vfio-pci, for passthrough."
    said "That's no matter if it's meant for something else."
}

test_nvidia_on_nouveau_is_refused() {
    rm -r "$R/sys/bus/pci/devices/0000:c4:00.0"
    make_card 0000:01:00.0 0x10de 0x2208 nouveau
    run_check
    exit_was 1
    said "failed   NVIDIA card 10de:2208 at 0000:01:00.0: driver nouveau, renderD128."
    said "GPU-PV needs nvidia on the card, not nouveau."
}

test_another_card_says_it_hasnt_run_under_proxmox() {
    rm "$R/sys/bus/pci/devices/0000:c4:00.0/driver"
    make_card 0000:c4:00.0 0x1002 0x73bf amdgpu
    run_check
    exit_was 0
    said "note     AMD card 1002:73bf at 0000:c4:00.0: driver amdgpu, renderD128. It hasn't run under Proxmox yet."
}

test_proxmoxs_own_qemu_is_refused() {
    printf 'pve-qemu-kvm ii 11.1.3-4\n' >>"$S/packages.new"
    grep -v '^pve-qemu-kvm ' "$S/packages" >>"$S/packages.new"
    mv "$S/packages.new" "$S/packages"
    run_check
    exit_was 1
    said "failed   pve-qemu-kvm: 11.1.3-4, Proxmox's own, which has no GPU-PV devices."
    said "packaging/proxmox/build-pve-qemu-kvm.sh 11.1.3-4"
}

unhold() {
    sed -i 's/^pve-qemu-kvm hi /pve-qemu-kvm ii /' "$S/packages"
}

test_a_build_nothing_keeps_is_a_warning() {
    unhold
    run_check
    exit_was 0
    said "warning  pve-qemu-kvm, kept: No hold or pin keeps it."
}

test_a_pin_above_proxmoxs_keeps_it() {
    unhold
    policy 11.1.4-1 990
    run_check
    exit_was 0
    said "ok       pve-qemu-kvm, kept: A pin keeps it, at priority 990."
}

test_a_file_that_names_the_package_but_doesnt_raise_it_keeps_nothing() {
    unhold
    printf 'Package: pve-qemu-kvm\nPin: origin download.proxmox.com\nPin-Priority: 1001\n' >"$R/etc/apt/preferences.d/guesthouse.disabled"
    run_check
    said "warning  pve-qemu-kvm, kept: No hold or pin keeps it."
}

test_a_newer_qemu_from_proxmox_is_a_warning() {
    policy 11.1.4-1
    run_check
    exit_was 0
    said "warning  pve-qemu-kvm, Proxmox's newest: Proxmox has 11.1.4-1, and this build is of 11.1.3-4."
}

test_a_build_whose_qemu_lacks_a_device_is_refused() {
    grep -v gpupv-render "$S/devices" >"$S/devices.new" && mv "$S/devices.new" "$S/devices"
    run_check
    exit_was 1
    said "failed   pve-qemu-kvm: 11.1.3-4+gpupv1, but its QEMU lacks: gpupv-render."
}

test_without_the_gpu_files_it_names_the_second_install() {
    rm -r "$R/home/alice/.local/lib/guesthouse/0.10.0/guest"
    run_check
    exit_was 1
    said "failed   Guesthouse, alice's GPU files"
    said "run install.sh once more"
}

test_a_user_outside_render_and_without_lingering_is_refused() {
    printf 'kvm:x:103:alice\nrender:x:104:\n' >"$R/etc/group"
    rm "$R/var/lib/systemd/linger/alice"
    run_check
    exit_was 1
    said "failed   Guesthouse, alice's groups: alice isn't in: render."
    said "failed   Guesthouse, alice's lingering: It's off."
}

test_guesthouse_checks_own_lines_come_through() {
    cat >"$R/home/alice/.local/lib/guesthouse/0.10.0/guesthouse-check" <<'EOF'
#!/bin/sh
printf 'check\tfailed\tVirtualization\t/dev/kvm can'"'"'t be opened.\tAdd yourself to kvm.\n'
printf 'gpu\tno\n'
EOF
    run_check
    exit_was 1
    said "failed   guesthouse-check: Virtualization: /dev/kvm can't be opened."
    said "failed   guesthouse-check: GPU: It says this PC can't run GPU machines."
}

test_missing_packages_for_windows_are_named_and_never_qemu_utils() {
    grep -v -e '^7zip ' -e '^genisoimage ' "$S/packages" >"$S/packages.new" && mv "$S/packages.new" "$S/packages"
    run_check
    exit_was 1
    said "failed   Packages for making a Windows guest: Not installed: 7zip genisoimage."
    said "Never qemu-utils"
}

test_swtpm_with_only_the_first_of_guesthouses_rules_is_refused() {
    echo 'owner /run/user/[0-9]*/guesthouse/** rwk,' >"$R/etc/apparmor.d/local/usr.bin.swtpm"
    run_check
    exit_was 1
    said "failed   swtpm's AppArmor rules: /etc/apparmor.d/local/usr.bin.swtpm lacks Guesthouse's lines."
}

test_without_swtpms_profile_no_rule_is_needed() {
    rm "$R/etc/apparmor.d/usr.bin.swtpm" "$R/etc/apparmor.d/local/usr.bin.swtpm"
    run_check
    exit_was 0
    not_said "swtpm's AppArmor rules"
}

test_a_restored_vm_names_the_old_vms_files() {
    make_windows_vm 101 1 8
    sed 's/gpupv-\(ram\|window\)\.101/gpupv-\1.205/g' "$R/etc/pve/qemu-server/101.conf" >"$R/etc/pve/qemu-server/205.conf"
    rm "$R/etc/pve/qemu-server/101.conf"
    mv "$R/etc/guesthouse/proxmox/101.conf" "$R/etc/guesthouse/proxmox/301.conf"
    sed 's/gpupv-\(ram\|window\)\.205/gpupv-\1.101/g' "$R/etc/pve/qemu-server/205.conf" >"$R/etc/pve/qemu-server/301.conf"
    run_check
    exit_was 1
    said "failed   VM 301: args: names the memory files of VM 101."
    said "failed   VM 205: /etc/guesthouse/proxmox/205.conf isn't there"
}

test_memory_and_the_ram_file_have_to_agree() {
    sed -i 's/^memory: 8192$/memory: 6144/' "$R/etc/pve/qemu-server/101.conf"
    run_check
    exit_was 1
    said "failed   VM 101: memory: 6144, and args: gives the RAM file 8G."
    not_said "memory: 4096"
}

test_a_vm_without_memory_has_proxmoxs_512() {
    sed -i '0,/^memory:/{/^memory:/d}' "$R/etc/pve/qemu-server/101.conf"
    run_check
    exit_was 1
    said "failed   VM 101: memory: 512, and args: gives the RAM file 8G."
}

test_a_window_file_of_another_vm_is_refused() {
    sed -i 's/gpupv-window\.101/gpupv-window.205/' "$R/etc/pve/qemu-server/101.conf"
    run_check
    exit_was 1
    said "failed   VM 101: args: names the memory files of VM 205."
}

test_args_without_the_ram_backend_is_refused() {
    sed -i 's/ -object memory-backend-file,id=ram0,[^ ]*//' "$R/etc/pve/qemu-server/101.conf"
    run_check
    exit_was 1
    said "failed   VM 101: args: lacks the RAM backend"
}

test_a_file_for_another_users_run_folder_is_refused() {
    printf 'bob:x:1001:1001::/home/bob:/bin/bash\n' >>"$R/etc/passwd"
    printf 'kvm:x:103:alice,bob\nrender:x:104:alice,bob\n' >"$R/etc/group"
    touch "$R/var/lib/systemd/linger/bob"
    make_guesthouse bob
    sed -i 's/^user=alice$/user=bob/' "$R/etc/guesthouse/proxmox/101.conf"
    run_check
    exit_was 1
    said "failed   VM 101: args: doesn't agree with its file, which has slot 1 and ram=8G for bob."
}

test_a_machine_with_options_is_q35_too() {
    sed -i 's/^machine: .*/machine: type=pc-q35-11.1,viommu=virtio/' "$R/etc/pve/qemu-server/101.conf"
    run_check
    exit_was 0
    said "ok       VM 101: Windows with the GPU, 8 GiB, slot 1."
}

test_guesthouse_check_runs_in_a_session_of_its_own() {
    cat >"$R/home/alice/.local/lib/guesthouse/0.10.0/guesthouse-check" <<EOF2
#!/bin/sh
session=\$(sed 's/^[^)]*) //' /proc/self/stat | cut -d' ' -f4)
if [ "\$session" = "$(sed 's/^[^)]*) //' /proc/$$/stat | cut -d' ' -f4)" ]; then
    printf 'check\tfailed\tSession\tIt shares the session of the script that ran it.\t\n'
else
    printf 'check\tok\tSession\tIt has a session of its own.\t\n'
fi
EOF2
    run_check
    said "guesthouse-check: Session: It has a session of its own."
}

test_guesthouse_checks_empty_fields_keep_their_places() {
    printf '#!/bin/sh\nprintf "check\\twarning\\tDisk\\t\\tFree some room.\\n"\n' >"$R/home/alice/.local/lib/guesthouse/0.10.0/guesthouse-check"
    run_check
    said "guesthouse-check: Disk (warning): Free some room."
}

test_guesthouse_check_reading_its_input_leaves_the_next_user() {
    printf 'bob:x:1001:1001::/home/bob:/bin/bash\n' >>"$R/etc/passwd"
    printf 'kvm:x:103:alice,bob\nrender:x:104:alice,bob\n' >"$R/etc/group"
    touch "$R/var/lib/systemd/linger/bob"
    make_guesthouse bob
    printf '#!/bin/sh\ncat >/dev/null\nprintf "gpu\\tyes\\n"\n' >"$R/home/alice/.local/lib/guesthouse/0.10.0/guesthouse-check"
    run_check
    said "ok       Guesthouse, bob: 0.10.0"
}

test_the_balloon_has_to_be_off() {
    sed -i '/^balloon:/d' "$R/etc/pve/qemu-server/101.conf"
    run_check
    exit_was 1
    said "failed   VM 101: balloon: on."
}

test_two_vms_in_one_slot_is_a_warning() {
    make_windows_vm 102 1 4
    run_check
    exit_was 0
    said "warning  VM 102: It has slot 1 of the same user as VM 101."
}

test_the_same_slot_of_two_users_is_no_matter() {
    make_windows_vm 102 1 4
    sed -i 's|/run/user/1000/|/run/user/1001/|g' "$R/etc/pve/qemu-server/102.conf"
    rm "$R/etc/guesthouse/proxmox/102.conf"
    sed -i 's|^hookscript: .*|hookscript: local:snippets/own-hook.sh|' "$R/etc/pve/qemu-server/102.conf"
    run_check
    not_said "of the same user"
}

test_a_vm_whose_file_is_gone_is_refused_and_a_file_without_a_vm_is_a_warning() {
    mv "$R/etc/guesthouse/proxmox/101.conf" "$R/etc/guesthouse/proxmox/999.conf"
    run_check
    exit_was 1
    said "failed   VM 101: /etc/guesthouse/proxmox/101.conf isn't there"
    said "warning  /etc/guesthouse/proxmox/999.conf: There's no VM 999."
}

test_another_hookscript_is_only_noted() {
    sed -i 's|^hookscript: .*|hookscript: local:snippets/own-hook.sh|' "$R/etc/pve/qemu-server/101.conf"
    rm "$R/etc/guesthouse/proxmox/101.conf"
    run_check
    exit_was 0
    said "note     VM 101: Its hookscript is local:snippets/own-hook.sh"
}

test_a_linux_guest_on_amd_needs_virglrenderer_with_amds_part() {
    cat >"$R/etc/pve/qemu-server/144.conf" <<'EOF'
args: -global virtio-vga-gl.drm_native_context=on -global virtio-vga-gl.blob=on -global virtio-vga-gl.hostmem=8G -fw_cfg name=opt/guesthouse/amd-mode,string=native
vga: virtio-gl
EOF
    run_check
    exit_was 1
    said "failed   VM 144: A Linux guest on AMD's native context, with virglrenderer 1.1.0-2 and pve-qemu-kvm 11.1.3-4+gpupv1."
    sed -i 's/^libvirglrenderer1 .*/libvirglrenderer1 ii 1.3.0-1+amd1/; s/^pve-qemu-kvm .*/pve-qemu-kvm hi 11.1.3-4+virgl1+gpupv1/' "$S/packages"
    printf 'amdgpu_ccmd_bo_va_op\n' >"$R/usr/lib/x86_64-linux-gnu/libvirglrenderer.so.1"
    run_check
    exit_was 0
    said "ok       VM 144: A Linux guest on AMD's native context, with virglrenderer 1.3.0-1+amd1 and AMD's part."
}

test_an_enterprise_repository_that_is_on_is_a_warning() {
    mkdir -p "$R/etc/apt/sources.list.d"
    printf 'Types: deb\nURIs: https://enterprise.proxmox.com/debian/pve\nSuites: trixie\nComponents: pve-enterprise\n' \
        >"$R/etc/apt/sources.list.d/pve-enterprise.sources"
    printf 'Types: deb\nURIs: http://deb.debian.org/debian/\nSuites: trixie\n\nTypes: deb\nURIs: https://enterprise.proxmox.com/debian/ceph-squid\nSuites: trixie\nComponents: enterprise\n' \
        >"$R/etc/apt/sources.list.d/ceph.sources"
    run_check
    exit_was 0
    said "warning  Package sources: Proxmox's enterprise repository is on, in /etc/apt/sources.list.d/ceph.sources /etc/apt/sources.list.d/pve-enterprise.sources."
}

test_an_enterprise_repository_that_is_off_is_no_matter() {
    mkdir -p "$R/etc/apt/sources.list.d"
    printf 'Types: deb\nURIs: https://enterprise.proxmox.com/debian/pve\nSuites: trixie\nComponents: pve-enterprise\nEnabled: no\n' \
        >"$R/etc/apt/sources.list.d/pve-enterprise.sources"
    printf '# deb https://enterprise.proxmox.com/debian/pve trixie pve-enterprise\n' >"$R/etc/apt/sources.list.d/old.list"
    run_check
    said "ok       Package sources: No enterprise repository is on."
}

test_an_enterprise_line_in_a_list_file_is_seen() {
    mkdir -p "$R/etc/apt/sources.list.d"
    printf 'deb https://enterprise.proxmox.com/debian/pve trixie pve-enterprise\n' >"$R/etc/apt/sources.list.d/pve-enterprise.list"
    run_check
    said "Proxmox's enterprise repository is on, in /etc/apt/sources.list.d/pve-enterprise.list."
}

test_a_vm_on_a_bridge_is_told_about_remote_desktop() {
    sed -i 's/^balloon: 0$/balloon: 0\nnet0: virtio=BC:24:11:00:00:01,bridge=vmbr0/' "$R/etc/pve/qemu-server/101.conf"
    run_check
    exit_was 0
    said "note     VM 101: It's on a bridge."
    said "only from 10.0.2.2"
}

test_a_vm_without_a_bridge_is_not_told() {
    run_check
    not_said "It's on a bridge."
}

test_a_cluster_is_a_warning() {
    touch "$R/etc/pve/corosync.conf"
    run_check
    exit_was 0
    said "warning  Cluster: This host is in a cluster."
}

test_huge_pages_are_a_warning() {
    sed -i 's/^HugePages_Total: .*/HugePages_Total:    6144/' "$R/proc/meminfo"
    run_check
    exit_was 0
    said "warning  Huge pages: 6144 pages of 2048 kB are set aside."
}

test_the_report_leaves_out_names_and_homes() {
    rm -r "$R/home/alice/.local/lib/guesthouse/0.10.0/guest"
    sed -i 's|^hookscript: .*|hookscript: alice-store:snippets/hook-alice.sh|' "$R/etc/pve/qemu-server/101.conf"
    cat >"$R/home/alice/.local/lib/guesthouse/0.10.0/guesthouse-check" <<'EOF'
#!/bin/sh
printf 'check\twarning\tFree disk\t10 GiB free in /mnt/alice/vms on pve.\tFree some room, alice.\n'
printf 'gpu\tyes\n'
EOF
    run_check --report
    said '```text'
    said "Guesthouse, USER's GPU files: ~/.local/lib/guesthouse/current/guest isn't there."
    said "10 GiB free in /mnt/USER/vms on HOST."
    said "pve-manager 9.2.32"
    said "kernel 7.0.14-30-pve."
    said "Its hookscript is another hookscript"
    not_said alice
}

test_the_report_keeps_guesthouses_own_names_for_a_user_called_guesthouse() {
    sed -i 's/alice/guesthouse/g' "$R/etc/passwd" "$R/etc/group" "$R/etc/guesthouse/proxmox/101.conf"
    mv "$R/home/alice" "$R/home/guesthouse"
    mv "$R/var/lib/systemd/linger/alice" "$R/var/lib/systemd/linger/guesthouse"
    run_check --report
    exit_was 0
    said "Guesthouse, USER: 0.10.0, in ~/.local/lib/guesthouse/current."
    said "guesthouse-check: Memory"
}

test_it_changes_no_file() {
    before=$(cd "$T" && find root state -exec stat -c '%n %s %Y %a' {} + | sort | sha256sum)
    run_check
    run_check --report
    after=$(cd "$T" && find root state -exec stat -c '%n %s %Y %a' {} + | sort | sha256sum)
    [ "$before" = "$after" ] || fail "the host's files changed"
}

test_without_pmxcfs_it_stops() {
    rm "$R/etc/pve/.version"
    run_check
    exit_was 2
    said "pve-cluster isn't running"
}

test_not_proxmox_stops_at_once() {
    rm "$T/bin/pveversion"
    run_check
    exit_was 2
    said "this isn't a Proxmox VE host"
}

# --- Running them ---

tests=("$@")
if [ ${#tests[@]} -eq 0 ]; then
    mapfile -t tests < <(declare -F | awk '{ print $3 }' | grep '^test_')
fi
for name in "${tests[@]}"; do
    T=$TEST_ROOT/$name
    rm -rf "$T"
    mkdir -p "$T"
    make_host
    test_failed=''
    printf '%s\n' "$name"
    "$name"
    if [ -n "$test_failed" ]; then
        failed=$((failed + 1))
        sed 's/^/    | /' "$T/said"
    else
        passed=$((passed + 1))
    fi
done
printf '\n%s passed, %s failed\n' "$passed" "$failed"
[ "$failed" -eq 0 ]
