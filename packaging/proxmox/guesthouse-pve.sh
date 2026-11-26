#!/bin/sh
# Says what a Proxmox VE host has and lacks for Windows guests with Guesthouse's GPU, in the order to put it
# right. It only reads: it changes nothing and opens no device, and of other programs it runs only
# guesthouse-check, as its user, and QEMU, to list its devices. docs/proxmox.md says what each part is for.
#
# Usage: packaging/proxmox/guesthouse-pve.sh check [--report]
#
# It runs as root on the host, as /etc/pve and the users' homes are root's to read. --report prints the same
# without the names of users, the host and VMs, and without users' paths, for pasting into an issue. It ends
# with 1 if something stops a VM from starting with the GPU, or stops Guesthouse from making the guest, with 0
# if not, and with 2 if it can't tell.
#
# GUESTHOUSE_PVE_ROOT puts a folder in front of every file it reads and of QEMU, so the tests can give it a
# host of their own. The other programs it runs come from PATH.
set -u
export LC_ALL=C

ROOT=${GUESTHOUSE_PVE_ROOT:-}
# The cards Windows has run on with GPU-PV under Proxmox, as docs/proxmox.md has them. Which cards Guesthouse
# itself knows is guesthouse-check's to say, and this asks it.
PROXMOX_RAN_CARDS='1002:15bf'
# What the host needs left of its RAM beside a guest's, as the hookscript has it.
RESERVE_MIB=2048
# The first virglrenderer that QEMU turns its async fences on for, which native context needs.
VIRGL_NATIVE=1.1.2
# What Guesthouse's installer puts in swtpm's local AppArmor rules, each a line of its own.
SWTPM_RULE='owner /run/user/[0-9]*/guesthouse/** rwk,'
SWTPM_SOCKET_RULE='unix (send, receive) type=stream addr=none,'
# How long guesthouse-check may take. It reads files and asks no one, so it's done in a second.
CHECK_SECONDS=60

# The results are kept split by the unit separator, as a tab would let read join empty fields.
sep=$(printf '\037')
tab=$(printf '\t')
results=''
failed=0
report=''

die() {
    printf 'guesthouse-pve: %s\n' "$*" >&2
    exit 2
}

# One result: the state (ok, note, warning or failed), the title, what was found, and the fix.
result() {
    results="$results$1$sep$2$sep$3$sep${4:-}
"
    [ "$1" != failed ] || failed=1
}

ok() { result ok "$@"; }
note() { result note "$@"; }
warning() { result warning "$@"; }
fail() { result failed "$@"; }

file() {
    printf '%s%s' "$ROOT" "$1"
}

# A user's name as it's printed: USER for --report.
shown() {
    if [ -n "$report" ]; then
        printf 'USER'
    else
        printf '%s' "$1"
    fi
}

has() {
    command -v "$1" >/dev/null 2>&1
}

# A package's status and installed version, such as "hi 1.0": what was asked for (h for one on hold) and i.
package_status() {
    dpkg-query -W -f='${db:Status-Abbrev}${Version}\n' "$1" 2>/dev/null | sed -n 's/^\([a-z]\)i *\(.*\)/\1i \2/p' | head -n 1
}

installed() {
    package_status "$1" | sed 's/^[a-z]i //'
}

# Whether version $1 is lower than $2, as Debian sorts them.
older() {
    dpkg --compare-versions "$1" lt "$2"
}

# Proxmox's version of a build of ours: what comes before +virgl or +gpupv.
base_version() {
    printf '%s\n' "$1" | sed -e 's/+virgl.*//' -e 's/+gpupv.*//'
}

# A VM's config without its snapshots, which follow in sections of their own.
vm_config() {
    sed '/^\[/,$d' "$1"
}

vm_line() {
    printf '%s\n' "$2" | sed -n "s/^$1: //p" | head -n 1
}

# --- The host ---

check_proxmox() {
    has pveversion || die "there's no pveversion, so this isn't a Proxmox VE host."
    # pmxcfs makes /etc/pve. Without it running, the folder is empty and every VM would seem to be missing.
    [ -e "$(file /etc/pve/.version)" ] || die "/etc/pve is empty, so pve-cluster isn't running. Start it first."
    pve=$(pveversion 2>/dev/null | sed -n 's|^pve-manager/\([^/ ]*\).*|\1|p')
    debian=$(cat "$(file /etc/debian_version)" 2>/dev/null)
    kernel=$(cat "$(file /proc/sys/kernel/osrelease)" 2>/dev/null)
    case $pve in
        9.*) ok "Proxmox VE" "pve-manager $pve, Debian ${debian:-?}, kernel ${kernel:-?}." ;;
        '') warning "Proxmox VE" "pveversion said no version." "Only Proxmox VE 9 has run." ;;
        *) warning "Proxmox VE" "pve-manager $pve, Debian ${debian:-?}, kernel ${kernel:-?}." \
            "Only Proxmox VE 9 on Debian 13 has run." ;;
    esac
}

check_cluster() {
    if [ -e "$(file /etc/pve/corosync.conf)" ]; then
        warning "Cluster" "This host is in a cluster." \
            "A cluster hasn't been tried. Each node needs its own package, and a VM with the GPU can't migrate."
    else
        ok "Cluster" "This host is on its own."
    fi
    ha=$(file /etc/pve/ha/resources.cfg)
    if [ -s "$ha" ] && grep -q '^vm:' "$ha"; then
        warning "High availability" "HA manages VMs here." "HA hasn't been tried with a VM that has the GPU."
    fi
}

# The enabled sources that name Proxmox's enterprise repository, one file on each line. A .sources file
# holds stanzas split by blank lines, and Enabled: no turns one off.
enterprise_sources() {
    for list in "$(file /etc/apt/sources.list)" "$(file /etc/apt/sources.list.d)"/*.list; do
        [ -f "$list" ] || continue
        grep -Eq '^[[:space:]]*deb[[:space:]].*enterprise\.proxmox\.com' "$list" && printf '%s\n' "${list#"$ROOT"}"
    done
    for sources in "$(file /etc/apt/sources.list.d)"/*.sources; do
        [ -f "$sources" ] || continue
        awk -v name="${sources#"$ROOT"}" '
            function stanza() {
                if (uris ~ /enterprise\.proxmox\.com/ && tolower(enabled) != "no")
                    found = 1
                uris = ""
                enabled = ""
            }
            /^[[:space:]]*$/ { stanza(); next }
            /^#/ { next }
            tolower($1) == "uris:" { uris = $0 }
            tolower($1) == "enabled:" { enabled = $2 }
            END { stanza(); if (found) print name }
        ' "$sources"
    done
}

# A fresh Proxmox VE has its enterprise repositories on. Without a subscription apt update fails at them,
# and nothing can be installed until they're off.
check_sources() {
    enterprise=$(enterprise_sources | tr '\n' ' ' | sed 's/ $//')
    if [ -n "$enterprise" ]; then
        warning "Package sources" "Proxmox's enterprise repository is on, in $enterprise." \
            "Without a subscription apt update fails at it. Turn it off (Enabled: no) and add pve-no-subscription, as Proxmox's docs on package repositories say."
    else
        ok "Package sources" "No enterprise repository is on."
    fi
}

check_kvm() {
    if [ -e "$(file /dev/kvm)" ]; then
        ok "Virtualization" "/dev/kvm is there."
    else
        fail "Virtualization" "There's no /dev/kvm." "Turn virtualization on in the firmware's settings."
    fi
}

# The words in modprobe's files and the kernel's line that keep a driver off a card, or give a card to
# vfio-pci for passthrough, each with where it is.
passthrough_words() {
    cmdline=$(cat "$(file /proc/cmdline)" 2>/dev/null)
    for word in $cmdline; do
        case $word in
            vfio-pci.ids=* | vfio_pci.ids=* | modprobe.blacklist=* | module_blacklist=*) printf 'kernel line: %s\n' "$word" ;;
        esac
    done
    for conf in "$(file /etc/modprobe.d)"/*.conf; do
        [ -f "$conf" ] || continue
        sed -n -e 's/^[[:space:]]*\(blacklist[[:space:]].*\)/\1/p' \
            -e 's/^[[:space:]]*\(install[[:space:]].*\)/\1/p' \
            -e 's/^[[:space:]]*\(options[[:space:]]\{1,\}vfio[-_]pci[[:space:]].*\)/\1/p' \
            -e 's/^[[:space:]]*\(softdep[[:space:]].*vfio.*\)/\1/p' "$conf" |
            while IFS= read -r line; do
                printf '%s: %s\n' "${conf#"$ROOT"}" "$line"
            done
    done
}

# Of those, what touches this card: its ID, or its own maker's driver. Another maker's driver, such as
# nouveau beside nvidia, is no matter.
blocks_for() {
    printf '%s\n' "$words" | grep -Ei -e "$1" \
        -e "(blacklist|install)[[:space:]]+($2)([[:space:]]|$)" \
        -e "softdep[[:space:]]+($2)[[:space:]].*vfio" \
        -e "blacklist=(.*,)?($2)(,|$)" | tr '\n' ';' | sed 's/;$//; s/;/; /g'
}

check_cards() {
    usable=0
    unusable=''
    words=$(passthrough_words)
    for dev in "$(file /sys/bus/pci/devices)"/*; do
        [ -r "$dev/class" ] || continue
        case $(cat "$dev/class") in
            0x03*) ;;
            *) continue ;;
        esac
        address=${dev##*/}
        vendor=$(sed 's/^0x//' "$dev/vendor")
        device=$(sed 's/^0x//' "$dev/device")
        id=$vendor:$device
        driver=''
        [ -L "$dev/driver" ] && driver=$(basename "$(readlink "$dev/driver")")
        render=''
        for node in "$dev"/drm/renderD*; do
            [ -e "$node" ] && render=${node##*/}
        done
        case $vendor in
            1002) maker=AMD want=amdgpu drivers=amdgpu ;;
            10de) maker=NVIDIA want=nvidia drivers='nvidia|nvidia_drm|nvidia-drm' ;;
            8086) maker=Intel want='i915 or xe' drivers='i915|xe' ;;
            *) continue ;;
        esac
        title="$maker card $id at $address"
        found="driver ${driver:-none}${render:+, $render}."
        blocks=$(blocks_for "$id" "$drivers")
        # A card that can't be the GPU is held back until it's known whether another one can, as a second card
        # may be meant for passthrough.
        case $driver in
            vfio-pci)
                unusable="$unusable$title$sep$found It's given to vfio-pci, for passthrough.${blocks:+ Found: $blocks.}$sep"
                unusable="${unusable}GPU-PV needs the host's own driver ($want) on the card. Take the card's ID out of vfio-pci's options and anything else found, run update-initramfs -u -k all, and reboot.
"
                continue ;;
            amdgpu | nvidia | i915 | xe) ;;
            '')
                unusable="$unusable$title$sep$found No driver has it.${blocks:+ Found: $blocks.}$sep"
                unusable="${unusable}GPU-PV needs the host's own driver ($want) on the card. Is it kept off, or not installed?
"
                continue ;;
            *)
                unusable="$unusable$title$sep$found${sep}GPU-PV needs $want on the card, not $driver.
"
                continue ;;
        esac
        # The daemon opens an AMD or Intel card by its render node. NVIDIA's daemon talks to nvidia's own device.
        if [ -z "$render" ] && [ "$vendor" != 10de ]; then
            unusable="$unusable$title$sep$found It has no render node.${sep}The daemon opens the card's /dev/dri/renderD node.
"
            continue
        fi
        usable=$((usable + 1))
        case " $PROXMOX_RAN_CARDS " in
            *" $id "*) ok "$title" "$found It has run under Proxmox." ;;
            *) note "$title" "$found It hasn't run under Proxmox yet." \
                "Whether Guesthouse knows it is guesthouse-check's line below. A check --report in an issue helps." ;;
        esac
        [ -z "$blocks" ] || warning "$title" "These would keep $want off it at the next boot: $blocks." \
            "Take them out, then update-initramfs -u -k all."
    done
    while IFS="$sep" read -r title found fix; do
        [ -n "$title" ] || continue
        if [ "$usable" -gt 0 ]; then
            note "$title" "$found It can't be the GPU for GPU-PV. That's no matter if it's meant for something else."
        else
            fail "$title" "$found" "$fix"
        fi
    done <<EOF
$unusable
EOF
    [ "$usable" -gt 0 ] || fail "GPU" "There's no AMD, NVIDIA or Intel card with its own driver."
}

# How the kernel's line is changed. proxmox-boot-tool keeps the boot partitions of a ZFS root in step, with
# systemd-boot on UEFI, which reads /etc/kernel/cmdline, or with GRUB, which reads /etc/default/grub.
boot_loader() {
    uuids=$(file /etc/kernel/proxmox-boot-uuids)
    if [ -s "$uuids" ] && [ -d "$(file /sys/firmware/efi)" ] && [ -f "$(file /etc/kernel/cmdline)" ] &&
        ! grep -qw grub "$uuids"; then
        printf 'Add %s to /etc/kernel/cmdline, run proxmox-boot-tool refresh' "$1"
    elif [ -s "$uuids" ]; then
        printf 'Add %s to GRUB_CMDLINE_LINUX_DEFAULT in /etc/default/grub, run proxmox-boot-tool refresh' "$1"
    else
        printf 'Add %s to GRUB_CMDLINE_LINUX_DEFAULT in /etc/default/grub, run update-grub' "$1"
    fi
}

# The list for lsm=: the modules running now, with Landlock first. capability is always on and isn't named.
lsm_line() {
    printf 'lsm=landlock'
    printf '%s\n' "$1" | tr ',' '\n' | grep -vx -e capability -e landlock -e '' | while IFS= read -r name; do
        printf ',%s' "$name"
    done
}

check_landlock() {
    lsm=$(cat "$(file /sys/kernel/security/lsm)" 2>/dev/null)
    case ,$lsm, in
        *,landlock,*)
            ok "Landlock" "The kernel runs it."
            return ;;
    esac
    where=$(boot_loader "$(lsm_line "$lsm")")
    current=$(sed -n 's/.*\(lsm=[^ ]*\).*/\1/p' "$(file /proc/cmdline)" 2>/dev/null)
    [ -z "$current" ] || where="$where, in place of $current"
    fail "Landlock" "It's off: the kernel runs ${lsm:-no modules it names}." \
        "Guesthouse makes the guest in its jail, which needs it. $where, and reboot the host."
}

mib_of() {
    sed -n "s/^$1: *\([0-9]*\) kB$/\1/p" "$(file /proc/meminfo)" 2>/dev/null | awk '{ print int($1 / 1024) }'
}

check_memory() {
    total=$(mib_of MemTotal)
    available=$(mib_of MemAvailable)
    shm=$(df -Pk "$(file /dev/shm)" 2>/dev/null | awk 'NR == 2 { print int($2 / 1024) }')
    huge=$(sed -n 's/^HugePages_Total: *\([0-9]*\)$/\1/p' "$(file /proc/meminfo)" 2>/dev/null)
    size=$(sed -n 's/^Hugepagesize: *\([0-9]*\) kB$/\1/p' "$(file /proc/meminfo)" 2>/dev/null)
    ok "Memory" "${total:-?} MiB, ${available:-?} available now. /dev/shm holds ${shm:-?} MiB."
    if [ "${huge:-0}" -gt 0 ]; then
        warning "Huge pages" "$huge pages of ${size:-?} kB are set aside." \
            "A GPU-PV guest's RAM is a file in /dev/shm and can't use them, so they're lost to it."
    fi
}

# --- QEMU ---

# apt's version table for pve-qemu-kvm, one "version priority" on each line, the newest first. apt is told
# to keep its cache in memory, so a stale cache isn't written anew.
qemu_versions() {
    apt-cache -o Dir::Cache::pkgcache= -o Dir::Cache::srcpkgcache= policy pve-qemu-kvm 2>/dev/null |
        awk '/Version table:/ { t = 1; next }
            t { sub(/^ *(\*\*\* )?/, ""); if (NF == 2 && $2 ~ /^-?[0-9]+$/) print $1, $2 }'
}

check_qemu() {
    status=$(package_status pve-qemu-kvm)
    version=${status#?? }
    if [ -z "$status" ]; then
        fail "pve-qemu-kvm" "It isn't installed." "Proxmox VE needs it. apt install pve-qemu-kvm, then a build with the GPU."
        return
    fi
    case $version in
        *+gpupv*) ;;
        *)
            fail "pve-qemu-kvm" "$version, Proxmox's own, which has no GPU-PV devices." \
                "Build one with packaging/proxmox/build-pve-qemu-kvm.sh $version, install it and hold it, as docs/proxmox.md says."
            return ;;
    esac
    # Proxmox starts every VM with this one, whatever is first in root's PATH.
    devices=$("$(file /usr/bin/qemu-system-x86_64)" -device help 2>/dev/null)
    missing=''
    for name in vmbus-bridge gpupv-global gpupv-vgpu gpupv-render hv-timesync; do
        printf '%s\n' "$devices" | grep -q "name \"$name\"" || missing="$missing $name"
    done
    if [ -n "$missing" ]; then
        fail "pve-qemu-kvm" "$version, but its QEMU lacks:$missing." "Build it again with packaging/proxmox/build-pve-qemu-kvm.sh."
        return
    fi
    ok "pve-qemu-kvm" "$version, with the GPU-PV devices."
    versions=$(qemu_versions)
    newest=$(printf '%s\n' "$versions" | grep -v -e '+gpupv' -e '+virgl' | head -n 1)
    ours=$(printf '%s\n' "$versions" | awk -v v="$version" '$1 == v { print $2; exit }')
    # A hold keeps it, and so does a pin that puts it above Proxmox's: apt takes the higher priority first.
    if [ "${status%% *}" = hi ]; then
        ok "pve-qemu-kvm, kept" "A hold keeps it."
    elif [ -n "$newest" ] && [ "${ours:-0}" -gt "${newest#* }" ]; then
        ok "pve-qemu-kvm, kept" "A pin keeps it, at priority $ours."
    else
        warning "pve-qemu-kvm, kept" "No hold or pin keeps it." \
            "The next update of Proxmox's QEMU puts its package back, and VMs with the GPU stop starting. apt-mark hold pve-qemu-kvm."
    fi
    base=$(base_version "$version")
    newest=${newest%% *}
    if [ -n "$newest" ] && older "$base" "$newest"; then
        warning "pve-qemu-kvm, Proxmox's newest" "Proxmox has $newest, and this build is of $base." \
            "Its fixes don't come until it's built: packaging/proxmox/build-pve-qemu-kvm.sh newest, then install and hold it."
    elif [ -n "$newest" ]; then
        ok "pve-qemu-kvm, Proxmox's newest" "$newest, the same as this build's."
    fi
}

# --- Guesthouse ---

# The users with a Guesthouse in their home, one "name uid home" on each line.
guesthouse_users() {
    awk -F: '$3 >= 1000 && $3 < 65534 { print $1, $3, $6 }' "$(file /etc/passwd)" 2>/dev/null | while read -r name uid home; do
        [ -x "$(file "$home/.local/lib/guesthouse/current/guesthouse-vm")" ] && printf '%s %s %s\n' "$name" "$uid" "$home"
    done
}

in_group() {
    awk -F: -v g="$2" -v u="$1" '$1 == g { n = split($4, m, ","); for (i = 1; i <= n; i++) if (m[i] == u) found = 1 }
        END { exit !found }' "$(file /etc/group)"
}

check_guesthouse() {
    users=$(guesthouse_users)
    if [ -z "$users" ]; then
        fail "Guesthouse" "No user here has it installed." \
            "It's installed by an ordinary user, with lingering on and in the groups kvm and render, as docs/headless.md says."
        return
    fi
    while read -r name uid home; do
        one_user "$name" "$uid" "$home"
    done <<EOF
$users
EOF
}

# What a user's Guesthouse has, and what its own guesthouse-check says, run as the user.
one_user() {
    name=$1 uid=$2 home=$3
    who=$(shown "$name")
    program=$home/.local/lib/guesthouse/current
    release=$(sed -n 's/^version=//p' "$(file "$program/RELEASE")" 2>/dev/null)
    ok "Guesthouse, $who" "${release:-an unnamed version}, in $program."
    missing=''
    in_group "$name" kvm || missing="$missing kvm"
    in_group "$name" render || missing="$missing render"
    [ -z "$missing" ] || fail "Guesthouse, $who's groups" "$who isn't in:$missing." "usermod -aG kvm,render $who, then log in again."
    [ -e "$(file "/var/lib/systemd/linger/$name")" ] ||
        fail "Guesthouse, $who's lingering" "It's off." "loginctl enable-linger $who, so the daemon has /run/user/$uid without a login."
    if [ ! -d "$(file "$program/guest")" ]; then
        fail "Guesthouse, $who's GPU files" "$program/guest isn't there." \
            "The installer leaves them out when guesthouse-check couldn't open /dev/kvm, as happens before the group kvm counts. Log in again and run install.sh once more."
    fi
    # Its lines are check, the state, the title, what was found and the fix, split by tabs, as check.rs says.
    # They come from a program in the user's home, so nothing but plain text of them reaches root's terminal.
    printed=$(runuser -u "$name" -- env -i HOME="$home" USER="$name" LOGNAME="$name" PATH=/usr/local/bin:/usr/bin:/bin \
        XDG_RUNTIME_DIR="/run/user/$uid" timeout "$CHECK_SECONDS" "$(file "$program/guesthouse-check")" </dev/null 2>/dev/null) || {
        warning "guesthouse-check, $who" "It didn't run." "Run it as $who to see why."
        return
    }
    lines=$(printf '%s\n' "$printed" | tr -d '\000-\010\013-\037\177' | tr "$tab" "$sep")
    while IFS="$sep" read -r kind state title text fix; do
        case $kind:$state in
            check:ok) ok "guesthouse-check: $title" "$text" ;;
            check:warning) warning "guesthouse-check: $title" "$text" "$fix" ;;
            check:failed) fail "guesthouse-check: $title" "$text" "$fix" ;;
            gpu:no) fail "guesthouse-check: GPU" "It says this PC can't run GPU machines." "Its lines above say why." ;;
        esac
    done <<EOF
$lines
EOF
}

# Guesthouse makes the guest with these, and its installer brings none of them.
check_packages() {
    missing=''
    for package in swtpm 7zip genisoimage; do
        [ -n "$(installed "$package")" ] || missing="$missing $package"
    done
    if [ -n "$missing" ]; then
        fail "Packages for making a Windows guest" "Not installed:$missing." \
            "apt install$missing. Never qemu-utils, which removes Proxmox VE."
    else
        ok "Packages for making a Windows guest" "swtpm, 7zip and genisoimage."
    fi
}

# Guesthouse's installer adds two lines to swtpm's AppArmor profile when AppArmor runs and the profile is
# there, and only if swtpm was installed when it ran.
check_swtpm_rule() {
    [ -n "$(installed swtpm)" ] || return 0
    grep -qw apparmor "$(file /sys/kernel/security/lsm)" 2>/dev/null || return 0
    [ -f "$(file /etc/apparmor.d/usr.bin.swtpm)" ] || return 0
    rules=$(file /etc/apparmor.d/local/usr.bin.swtpm)
    if grep -qxF "$SWTPM_RULE" "$rules" 2>/dev/null && grep -qxF "$SWTPM_SOCKET_RULE" "$rules" 2>/dev/null; then
        ok "swtpm's AppArmor rules" "Both of Guesthouse's lines are there."
    else
        fail "swtpm's AppArmor rules" "/etc/apparmor.d/local/usr.bin.swtpm lacks Guesthouse's lines." \
            "Guesthouse's installer adds them only when swtpm is there before it runs. Run install.sh again."
    fi
}

# --- The VMs ---

check_vms() {
    seen_slots=''
    any=''
    for conf in "$(file /etc/pve/qemu-server)"/*.conf; do
        [ -f "$conf" ] || continue
        vmid=${conf##*/}
        vmid=${vmid%.conf}
        config=$(vm_config "$conf")
        args=$(vm_line args "$config")
        case $args in
            *gpupv-render*) any=1 && check_windows_vm ;;
        esac
        case $args in
            *amd-mode*) any=1 && check_linux_vm ;;
        esac
    done
    for file in "$(file /etc/guesthouse/proxmox)"/*.conf; do
        [ -f "$file" ] || continue
        id=${file##*/}
        id=${id%.conf}
        [ -f "$(file "/etc/pve/qemu-server/$id.conf")" ] ||
            warning "/etc/guesthouse/proxmox/$id.conf" "There's no VM $id." \
                "setup-vm.sh counts its slot as taken until the file goes."
    done
    [ -n "$any" ] || note "VMs" "None has the GPU's words yet." "packaging/proxmox/setup-vm.sh sets one up."
}

# A failed result for the VM being checked, which then isn't ok.
problem() {
    fail "$title" "$1" "$2"
    problems=1
}

check_windows_vm() {
    title="VM $vmid"
    # Proxmox's default when there's no line.
    memory=$(vm_line memory "$config")
    memory=${memory:-512}
    balloon=$(vm_line balloon "$config")
    hook=$(vm_line hookscript "$config")
    bios=$(vm_line bios "$config")
    machine=$(vm_line machine "$config")
    ram_size=$(printf '%s\n' "$args" | sed -n 's/.*memory-backend-file,id=ram0,size=\([0-9]*\)G,.*/\1/p')
    ram_vm=$(printf '%s\n' "$args" | sed -n 's|.*id=ram0,[^ ]*mem-path=/dev/shm/gpupv-ram\.\([0-9]*\),.*|\1|p')
    window_vm=$(printf '%s\n' "$args" | sed -n 's|.*id=gpio,[^ ]*mem-path=/dev/shm/gpupv-window\.\([0-9]*\),.*|\1|p')
    # The run folder is the uid's, and its slot names it: run for slot 0, run-N for the others.
    run=$(printf '%s\n' "$args" | sed -n 's|.*path=\(/run/user/[0-9]*/guesthouse/run[-0-9]*\)/vgpu\.sock,.*|\1|p')
    run_uid=$(printf '%s\n' "$run" | sed -n 's|^/run/user/\([0-9]*\)/.*|\1|p')
    case $run in
        */run) slot=0 ;;
        */run-[1-3]) slot=${run##*-} ;;
        *) slot='' ;;
    esac
    problems=0
    [ "$bios" = ovmf ] || problem "bios: ${bios:-seabios}." "Windows with GPU-PV needs bios: ovmf."
    case ,$machine, in
        ,q35* | ,pc-q35* | *,type=q35* | *,type=pc-q35*) ;;
        *) problem "machine: ${machine:-i440fx}." "It needs a q35 machine type." ;;
    esac
    [ "${balloon:-1}" = 0 ] || problem "balloon: ${balloon:-on}." "qm set $vmid --balloon 0, as the RAM file is taken whole at the start."
    if [ -z "$ram_size" ] || [ -z "$ram_vm" ] || [ -z "$window_vm" ] || [ -z "$slot" ]; then
        problem "args: lacks the RAM backend, the window's file or the sockets it needs." "packaging/proxmox/setup-vm.sh $vmid MACHINE writes them."
    else
        for other in "$ram_vm" "$window_vm"; do
            [ "$other" = "$vmid" ] && continue
            problem "args: names the memory files of VM $other." \
                "It came from VM $other, as a restore or a clone does. packaging/proxmox/setup-vm.sh $vmid MACHINE puts it right."
            break
        done
        [ "$((ram_size * 1024))" = "$memory" ] ||
            problem "memory: $memory, and args: gives the RAM file ${ram_size}G." "They have to agree. setup-vm.sh sets both."
    fi
    case $hook in
        '') problem "It has no hookscript, so nothing starts its daemon." "packaging/proxmox/setup-vm.sh sets one." ;;
        */guesthouse.sh) check_hook_conf ;;
        *)
            [ -z "$report" ] || hook='another hookscript'
            note "$title" "Its hookscript is $hook, not Guesthouse's, so what it reads isn't checked." ;;
    esac
    if [ -n "$slot" ]; then
        case " $seen_slots " in
            *" $run_uid:$slot:"*)
                other=$(printf '%s\n' "$seen_slots" | tr ' ' '\n' | sed -n "s/^$run_uid:$slot://p" | head -n 1)
                warning "$title" "It has slot $slot of the same user as VM $other." "Only one of them can run at a time." ;;
        esac
        seen_slots="$seen_slots $run_uid:$slot:$vmid"
    fi
    [ "$problems" = 1 ] || ok "$title" "Windows with the GPU, $ram_size GiB, slot $slot."
    # Guesthouse sets Windows' firewall up for its own NAT, where the host is 10.0.2.2. On a bridge that
    # address isn't there, so what Guesthouse let in, Remote Desktop among it, stays shut from the LAN.
    if printf '%s\n' "$config" | grep -Eq '^net[0-9]+: .*bridge='; then
        note "$title" "It's on a bridge. If its Windows was made with Guesthouse, Windows' firewall lets Remote Desktop (3389) in only from 10.0.2.2, the host on Guesthouse's own network, so it's shut from the LAN until a rule in Windows lets it in. Proxmox's console stays black after the boot. Windows+P, Duplicate, put the desktop there for one tester."
    fi
}

# What setup-vm.sh wrote for the VM, which Guesthouse's hookscript reads. The hookscript refuses to start
# the VM unless args: has these words, as check_config there has them.
check_hook_conf() {
    hook_conf=$(file "/etc/guesthouse/proxmox/$vmid.conf")
    if [ ! -r "$hook_conf" ]; then
        problem "/etc/guesthouse/proxmox/$vmid.conf isn't there, so the hookscript won't start it." \
            "packaging/proxmox/setup-vm.sh $vmid MACHINE writes it."
        return
    fi
    conf_slot=$(sed -n 's/^slot=//p' "$hook_conf")
    conf_ram=$(sed -n 's/^ram=//p' "$hook_conf")
    conf_user=$(sed -n 's/^user=//p' "$hook_conf")
    conf_uid=$(guesthouse_users | awk -v u="$conf_user" '$1 == u { print $2 }')
    if [ -z "$conf_uid" ]; then
        problem "Its file names $(shown "$conf_user"), who has no Guesthouse." "Run setup-vm.sh again with --user."
        return
    fi
    case $conf_slot in
        0) conf_run=/run/user/$conf_uid/guesthouse/run ;;
        *) conf_run=/run/user/$conf_uid/guesthouse/run-$conf_slot ;;
    esac
    case $conf_ram in
        [1-9]*G) gib=${conf_ram%G} ;;
        *) gib=x ;;
    esac
    case $gib in
        *[!0-9]*)
            problem "Its file has ram=$conf_ram, and the hookscript takes a number of GiB, like 8G." "Run setup-vm.sh again."
            return ;;
    esac
    [ "$memory" = "$((gib * 1024))" ] || problem "memory: $memory, and its file has ram=$conf_ram." "Run setup-vm.sh again."
    for word in "id=ram0,size=$conf_ram,mem-path=/dev/shm/gpupv-ram.$vmid,share=on" \
        "id=gpio,size=64G,mem-path=/dev/shm/gpupv-window.$vmid,share=on" \
        "path=$conf_run/global.sock," "path=$conf_run/vgpu.sock,"; do
        case " $args " in
            *"$word"*) ;;
            *)
                problem "args: doesn't agree with its file, which has slot $conf_slot and ram=$conf_ram for $(shown "$conf_user")." \
                    "The hookscript won't start it. Run setup-vm.sh again."
                return ;;
        esac
    done
}

check_linux_vm() {
    title="VM $vmid"
    vga=$(vm_line vga "$config")
    virgl=$(installed libvirglrenderer1)
    case $vga in
        virtio-gl*) ;;
        *) fail "$title" "It asks for AMD's native context, with vga: ${vga:-std}." "It needs vga: virtio-gl." && return ;;
    esac
    case $(installed pve-qemu-kvm) in
        *+virgl*) qemu_virgl=1 ;;
        *) qemu_virgl='' ;;
    esac
    if [ -z "$virgl" ] || older "$virgl" "$VIRGL_NATIVE" || [ -z "$qemu_virgl" ]; then
        fail "$title" "A Linux guest on AMD's native context, with virglrenderer ${virgl:-none} and pve-qemu-kvm $(installed pve-qemu-kvm)." \
            "It needs virglrenderer $VIRGL_NATIVE or newer with AMD's part, and a pve-qemu-kvm built against it. No package of Guesthouse's has that yet."
        return
    fi
    # The AMD part is a build option, and only a library built with it has its commands, named amdgpu_ccmd.
    if ! grep -qs amdgpu_ccmd "$(file /usr/lib/x86_64-linux-gnu/libvirglrenderer.so.1)"; then
        fail "$title" "A Linux guest on AMD's native context, with virglrenderer $virgl, built without AMD's part." \
            "It needs virglrenderer built with -Ddrm-renderers=amdgpu-experimental."
        return
    fi
    ok "$title" "A Linux guest on AMD's native context, with virglrenderer $virgl and AMD's part."
}

# --- Printing ---

# What --report leaves out beyond what's printed as USER: the homes and names of all ordinary users and the
# host's name, wherever they come up, such as in guesthouse-check's lines. Each is taken as it is, not as a
# pattern, and only where it stands alone, so a host called pve doesn't change pve-manager. A user called
# guesthouse keeps Guesthouse's own folders and programs.
private() {
    if [ -z "$report" ]; then
        cat
        return
    fi
    host=$(cat "$(file /etc/hostname)" 2>/dev/null)
    users=$(awk -F: '$3 >= 1000 && $3 < 65534 { printf "%s %s|", $1, $6 }' "$(file /etc/passwd)" 2>/dev/null)
    awk -v host="$host" -v users="$users" '
        function alone(s, word, to,    out, i, before, after) {
            out = ""
            while ((i = index(s, word)) > 0) {
                before = i > 1 ? substr(s, i - 1, 1) : " "
                after = substr(s, i + length(word), 1)
                if (before !~ /[A-Za-z0-9_.-]/ && after !~ /[A-Za-z0-9_-]/ && !(word == "guesthouse" && before == "/"))
                    out = out substr(s, 1, i - 1) to
                else
                    out = out substr(s, 1, i - 1 + length(word))
                s = substr(s, i + length(word))
            }
            return out s
        }
        function literal(s, word, to,    out, i) {
            out = ""
            while ((i = index(s, word)) > 0) {
                out = out substr(s, 1, i - 1) to
                s = substr(s, i + length(word))
            }
            return out s
        }
        BEGIN {
            count = split(users, line, "|")
            for (i = 1; i <= count; i++)
                if (split(line[i], f, " ") == 2) { n++; name[n] = f[1]; home[n] = f[2] }
        }
        {
            for (i = 1; i <= n; i++)
                if (home[i] != "" && home[i] != "/")
                    $0 = literal($0, home[i], "~")
            for (i = 1; i <= n; i++)
                $0 = alone($0, name[i], "USER")
            if (host != "")
                $0 = alone($0, host, "HOST")
            gsub(/\/run\/user\/[0-9]+/, "/run/user/UID")
            print
        }'
}

print_results() {
    [ -z "$report" ] || printf '```text\n'
    printf '%s' "$results" | while IFS="$sep" read -r state title found fix; do
        printf '%-8s %s: %s\n' "$state" "$title" "$found"
    done
    todo=$(printf '%s' "$results" | awk -F "$sep" -v OFS="$sep" '($1 == "failed" || $1 == "warning") && $4 != "" { print }')
    if [ -n "$todo" ]; then
        printf '\nTo put right, in this order:\n'
        n=0
        printf '%s\n' "$todo" | while IFS="$sep" read -r state title found fix; do
            n=$((n + 1))
            printf '%2d. %s (%s): %s\n' "$n" "$title" "$state" "$fix"
        done
    fi
    if [ "$failed" = 0 ]; then
        printf '\nNothing here stops a VM with the GPU, or Guesthouse making its guest.\n'
    fi
    [ -z "$report" ] || printf '```\n'
}

check() {
    while [ $# -gt 0 ]; do
        case $1 in
            --report) report=1 && shift ;;
            *) die "check has no option $1." ;;
        esac
    done
    [ -n "$ROOT" ] || [ "$(id -u)" = 0 ] || die "this runs as root, as /etc/pve and the users' homes are root's to read."
    check_proxmox
    check_cluster
    check_sources
    check_kvm
    check_cards
    check_landlock
    check_memory
    check_qemu
    check_packages
    check_swtpm_rule
    check_guesthouse
    check_vms
    print_results | private
    return "$failed"
}

main() {
    [ $# -gt 0 ] || die "name what to do: check."
    command=$1
    shift
    case $command in
        check) check "$@" ;;
        -h | --help | help) sed -n '2,/^set -u$/ { /^#/ s/^# \{0,1\}//p }' "$0" ;;
        *) die "there's no $command. There's check." ;;
    esac
}

main "$@"
