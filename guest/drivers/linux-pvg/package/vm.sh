#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
#
# The build VM: Ubuntu 26.04's arm64 cloud image as a raw disk, started by
# UEFI under our QEMU with a cloud-init seed of ours, 4 processors and 6 GiB,
# no graphics device. It has a time limit and is reached by ssh on a port of
# the Mac's loopback.
#
# Usage: vm.sh PROJECT_ROOT make             the disk and the seed, once
#        vm.sh PROJECT_ROOT start SECONDS    starts it, waits for ssh
#        vm.sh PROJECT_ROOT ssh COMMAND...   a command in it, as pvg (sudo works)
#        vm.sh PROJECT_ROOT stop             powers it down, then ends QEMU
# Environment: PVG_VM the VM's folder (target/linux-pvg/vm), PVG_VM_PORT
#   the ssh port (22631), PVG_QEMU the QEMU (target/mac-helpers, from
#   qemu/mac/build.sh), PVG_FIRMWARE the folder with its edk2 firmware
#   (target/mac-helpers/Resources/qemu), PVG_IMAGE Ubuntu 26.04's arm64 cloud
#   image, for make.
#
# It has no pvg device, so it runs without the graphics process.
set -eu
root=$(cd "$1" && pwd)
what=$2
shift 2
vm=${PVG_VM:-$root/target/linux-pvg/vm}
port=${PVG_VM_PORT:-22631}
qemu=${PVG_QEMU:-$root/target/mac-helpers/qemu-system-aarch64}
firmware=${PVG_FIRMWARE:-$root/target/mac-helpers/Resources/qemu}
here=$root/guest/drivers/linux-pvg/package
ssh_to() {
	ssh -i "$vm/key" -p "$port" -o StrictHostKeyChecking=no -o UserKnownHostsFile="$vm/known_hosts" \
		-o ConnectTimeout=5 -o ServerAliveInterval=15 -o LogLevel=ERROR pvg@127.0.0.1 "$@"
}

case $what in
make)
	mkdir -p "$vm"
	[ -f "$vm/key" ] || ssh-keygen -q -t ed25519 -N '' -C pvg-build-vm -f "$vm/key"
	image=${PVG_IMAGE:?names the Ubuntu 26.04 arm64 cloud image}
	[ -f "$vm/build.img" ] || python3 -I "$here/to-raw.py" "$qemu" "$image" "$vm/build.img" 40
	cp "$firmware/edk2-aarch64-vars.fd" "$vm/vars.fd"
	rm -rf "$vm/seed" "$vm/seed.iso"
	mkdir -p "$vm/seed"
	printf 'instance-id: pvg-build-1\nlocal-hostname: pvg-build\n' > "$vm/seed/meta-data"
	sed "s|@KEY@|$(cat "$vm/key.pub")|" "$here/vm-user-data" > "$vm/seed/user-data"
	hdiutil makehybrid -quiet -iso -joliet -default-volume-name cidata -o "$vm/seed.iso" "$vm/seed"
	ls -l "$vm"
	;;
start)
	seconds=$1
	if [ -f "$vm/qemu.pid" ] && kill -0 "$(cat "$vm/qemu.pid")" 2>/dev/null; then
		echo "the build VM runs already, QEMU $(cat "$vm/qemu.pid")" >&2
		exit 1
	fi
	cd "$vm"
	rm -f qmp.sock
	"$qemu" -M virt -accel hvf -cpu host -smp 4 -m 6144 \
		-drive "if=pflash,format=raw,readonly=on,file=$firmware/edk2-aarch64-code.fd" \
		-drive "if=pflash,format=raw,file=$vm/vars.fd" \
		-drive "if=virtio,format=raw,file=$vm/build.img,discard=unmap" \
		-drive "if=virtio,format=raw,readonly=on,file=$vm/seed.iso" \
		-netdev "user,id=net,hostfwd=tcp:127.0.0.1:$port-:22" -device virtio-net-pci,netdev=net,romfile= \
		-display none -monitor none -serial "file:$vm/serial.log" \
		-qmp "unix:$vm/qmp.sock,server,wait=off" > "$vm/qemu.out" 2>&1 &
	pid=$!
	echo "$pid" > qemu.pid
	echo "build VM: QEMU $pid, for $seconds s at most (kill $pid ends it)"
	# The time limit, on the clock, by the process number.
	(
		end=$(($(date +%s) + seconds))
		while kill -0 "$pid" 2>/dev/null; do
			if [ "$(date +%s)" -ge "$end" ]; then
				echo "build VM: $seconds s are up, ending QEMU $pid" >> "$vm/qemu.out"
				kill "$pid"
				sleep 10
				kill -9 "$pid" 2>/dev/null
				break
			fi
			# The serial log is small; it may not pass 50 MB.
			if [ "$(wc -c < "$vm/serial.log" 2>/dev/null || echo 0)" -gt 50000000 ]; then
				echo "build VM: serial.log past 50 MB, ending QEMU $pid" >> "$vm/qemu.out"
				kill "$pid"
				break
			fi
			sleep 5
		done
	) > /dev/null 2>&1 &
	n=0
	until ssh_to true 2>/dev/null; do
		n=$((n + 1))
		if [ $n -gt 120 ] || ! kill -0 "$pid" 2>/dev/null; then
			echo "build VM: no ssh after $((n * 5)) s" >&2
			tail -20 "$vm/serial.log" >&2
			exit 1
		fi
		sleep 5
	done
	echo "build VM: ssh answers on port $port"
	;;
ssh)
	ssh_to "$@"
	;;
stop)
	pid=$(cat "$vm/qemu.pid" 2>/dev/null || true)
	[ -n "$pid" ] && kill -0 "$pid" 2>/dev/null || { echo "build VM is not running"; exit 0; }
	ssh_to sudo poweroff 2>/dev/null || true
	n=0
	while kill -0 "$pid" 2>/dev/null && [ $n -lt 60 ]; do sleep 1; n=$((n + 1)); done
	if kill -0 "$pid" 2>/dev/null; then
		echo "build VM: QEMU $pid still there after 60 s, TERM"
		kill "$pid"
	fi
	rm -f "$vm/qemu.pid"
	echo "build VM: down"
	;;
*)
	echo "usage: vm.sh PROJECT_ROOT make|start SECONDS|ssh COMMAND|stop" >&2
	exit 2
	;;
esac
