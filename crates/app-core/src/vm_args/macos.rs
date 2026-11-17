//! A macOS guest's arguments, which have little in common with the others. Its GPU is reims-vgpu's
//! device, which only that project's QEMU has, and the machine line is the one its own tool starts a
//! Mac with. A guest was installed on these devices at these places, so they stay as they are.
//!
//! The device shows its screen on QEMU's console, so the viewer gets it through the D-Bus display as
//! it gets a plain machine's, and the keyboard and pointer are QEMU's.

use super::arguments::{MONITOR_SOCKETS, words};
use super::{Boot, VmPlan};
use crate::text::qemu_value;
use std::path::PathBuf;

/// Where the program folder keeps reims-vgpu's QEMU and the device's option ROM.
pub const MACOS_QEMU: &str = "macos/qemu/bin/qemu-system-x86_64";
pub const MACOS_ROM: &str = "macos/ReimsGraphics.rom";

/// macOS puts every 64-bit BAR below 4 GiB, so the guest's RAM leaves room for the device's there.
const MACHINE: &str = "q35,memory-backend=ram,dump-guest-core=off,max-ram-below-4g=1G";
/// A CPU macOS knows, whatever the host's is, with what its kernel asks for at boot.
const CPU: &str = "Skylake-Client-noTSX-IBRS,kvm=on,vendor=GenuineIntel,+invtsc,vmware-cpuid-freq=on,+ssse3,\
                   +sse4.2,+popcnt,+avx,+aes,+xsave,+xsaveopt,check";
/// The key macOS reads from the system management controller of every Mac.
const SMC: &str = "isa-applesmc,osk=ourhardworkbythesewordsguardedpleasedontsteal(c)AppleComputerInc";
const LOG_VARIABLE: &str = "REIMS_VGPU_LOG";
const LOG_LEVEL: &str = "warn";

/// What a macOS guest boots from beside its disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosBoot {
    /// The OpenCore image, which starts macOS and adds the device's drivers to it.
    pub opencore: PathBuf,
    /// The firmware OpenCore was made for, which isn't the one the other machines use.
    pub firmware_code: PathBuf,
    /// Apple's installer as a disk, or empty once macOS is installed.
    pub installer: String,
    pub rom: PathBuf,
}

impl VmPlan {
    fn macos_boot(&self) -> Option<&MacosBoot> {
        match &self.boot {
            Boot::Macos(boot) => Some(boot),
            _ => None,
        }
    }

    /// The device reaches the guest's memory through the GPU, so the memory has to be shareable.
    fn macos_machine(&self) -> Vec<String> {
        let memory = format!(
            "memory-backend-memfd,id=ram,size={},share=on,prealloc=on,prealloc-threads={}",
            self.ram, self.cores
        );
        words(&[
            "-name",
            &self.name,
            "-machine",
            MACHINE,
            "-object",
            &memory,
            "-accel",
            "kvm",
            "-m",
            &self.ram,
            "-cpu",
            CPU,
            "-smp",
            &format!("{0},sockets=1,cores={0},threads=1", self.cores),
            "-global",
            "ICH9-LPC.acpi-pci-hotplug-with-bridge-support=off",
            "-global",
            "ICH9-LPC.disable_s3=1",
            "-device",
            "pcie-root-port,bus=pcie.0,addr=0x1,slot=1,speed=16,width=32",
            "-device",
            SMC,
        ])
    }

    fn macos_disks(&self, boot: &MacosBoot) -> Vec<String> {
        let mut arguments = words(&[
            "-drive",
            &format!("if=pflash,format=raw,readonly=on,file={}", boot.firmware_code.display()),
            "-drive",
            &format!("if=pflash,format=raw,file={}", qemu_value(&self.vars)),
            "-smbios",
            "type=2",
            "-device",
            "ich9-ahci,id=sata,addr=0x2",
            "-drive",
            &format!("id=opencore,if=none,format=qcow2,file={}", boot.opencore.display()),
            "-device",
            "ide-hd,bus=sata.2,drive=opencore,bootindex=1",
            "-drive",
            // The file before the format, which is how a running VM is known by its disk.
            &format!("id=disk,if=none,file={},format=qcow2", qemu_value(&self.disk)),
            "-device",
            "ide-hd,bus=sata.3,drive=disk,rotation_rate=1",
        ]);
        if !boot.installer.is_empty() {
            // Nothing the installer writes to its own disk is kept.
            arguments.extend(words(&[
                "-drive",
                &format!("id=installer,if=none,format=raw,snapshot=on,file={}", qemu_value(&boot.installer)),
                "-device",
                "ide-hd,bus=sata.4,drive=installer",
            ]));
        }
        arguments
    }

    fn macos_network(&self) -> Vec<String> {
        let card = match self.network.mac.as_str() {
            "" => "virtio-net-pci,netdev=net0,id=net0,addr=0x3".to_owned(),
            mac => format!("virtio-net-pci,netdev=net0,id=net0,mac={mac},addr=0x3"),
        };
        words(&["-netdev", "user,id=net0,ipv6=off", "-device", &card])
    }

    fn macos_devices(&self, boot: &MacosBoot) -> Vec<String> {
        let (sound, sound_id) = self.sound_backend();
        let screen = format!(
            "reims-graphics-pci,id=gfx,bus=pcie.0,addr=0x4,resolution={},romfile={}",
            self.screen,
            boot.rom.display()
        );
        words(&[
            "-vga",
            "none",
            "-device",
            &screen,
            "-device",
            "qemu-xhci,id=xhci,addr=0x5,conditional-intr-mapping=on,p2=7,p3=7",
            "-device",
            "usb-kbd,id=kbd,bus=xhci.0",
            "-device",
            "usb-tablet,id=tablet,bus=xhci.0",
            "-audiodev",
            &sound,
            "-device",
            &format!("virtio-sound-pci,id=audio,audiodev={sound_id},addr=0x7,streams=1"),
            "-device",
            &format!("virtio-sound-pci,id=audioin,audiodev={sound_id},addr=0x8,streams=1,input-streams=1"),
            "-device",
            "ich9-intel-hda,id=hda,addr=0x1b",
            "-device",
            &format!("hda-output,bus=hda.0,audiodev={sound_id}"),
        ])
    }

    /// With a display of QEMU's the device opens no window of its own, and shows on the console.
    fn macos_outputs(&self) -> Vec<String> {
        let mut arguments = words(&["-display", "dbus,p2p=yes"]);
        for socket in MONITOR_SOCKETS {
            arguments
                .extend(words(&["-qmp", &format!("unix:{},server=on,wait=off", self.in_run_folder(socket))]));
        }
        arguments.extend(words(&[
            "-serial",
            "none",
            "-monitor",
            "none",
            "-D",
            &self.in_run_folder("qemu.log"),
        ]));
        arguments
    }

    /// The whole line of a macOS guest, or nothing for any other.
    pub(super) fn macos_arguments(&self) -> Option<Vec<String>> {
        let boot = self.macos_boot()?;
        let mut arguments = self.macos_machine();
        arguments.extend(self.macos_disks(boot));
        arguments.extend(self.macos_network());
        arguments.extend(self.macos_devices(boot));
        arguments.extend(self.macos_outputs());
        Some(arguments)
    }

    /// The device's library says what goes wrong on QEMU's output, where a failed start is read from.
    pub(super) fn macos_environment(&self) -> Vec<(String, String)> {
        let log = (LOG_VARIABLE.to_owned(), LOG_LEVEL.to_owned());
        self.macos_boot().map(|_| log).into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{Kind, Machine};
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};

    fn boot(installer: &str) -> Boot {
        Boot::Macos(MacosBoot {
            opencore: "/m/mac/opencore.qcow2".into(),
            firmware_code: "/m/mac/OVMF_CODE.fd".into(),
            installer: installer.to_owned(),
            rom: "/p/macos/ReimsGraphics.rom".into(),
        })
    }

    fn plan(boot: Boot) -> VmPlan {
        let scratch = Scratch::new();
        let machine = Machine {
            kind: Kind::Plain,
            opencore: "/m/mac/opencore.qcow2".into(),
            cores: 6,
            ram_gib: 12,
            screen: "2560x1440".into(),
            mac: "52:54:00:9b:79:fe".into(),
            ..Machine::new("mac", "Mac", "/m/mac/disk.qcow2", "/m/mac/OVMF_VARS.fd")
        };
        VmPlan::new(&scratch_context(&scratch), &machine, boot, slots::first(), 0)
    }

    fn value_after<'a>(arguments: &'a [String], name: &str) -> &'a str {
        let at = arguments.iter().position(|word| word == name).unwrap();
        &arguments[at + 1]
    }

    #[test]
    fn its_memory_is_shareable_and_its_screen_comes_through_the_dbus_display() {
        let arguments = plan(boot("")).qemu_arguments();
        assert_eq!(value_after(&arguments, "-machine"), MACHINE);
        assert_eq!(
            value_after(&arguments, "-object"),
            "memory-backend-memfd,id=ram,size=12G,share=on,prealloc=on,prealloc-threads=6"
        );
        assert_eq!(value_after(&arguments, "-smp"), "6,sockets=1,cores=6,threads=1");
        assert_eq!(value_after(&arguments, "-display"), "dbus,p2p=yes", "or the device opens a window");
        assert_eq!(value_after(&arguments, "-vga"), "none");
        let device = arguments.iter().find(|word| word.starts_with("reims-graphics-pci,")).unwrap();
        assert!(device.contains(",resolution=2560x1440,romfile=/p/macos/ReimsGraphics.rom"), "{device}");
        assert!(arguments.iter().any(|word| word.ends_with("display.sock,server=on,wait=off")));
    }

    #[test]
    fn it_boots_opencore_on_its_own_firmware_with_the_disk_beside_it() {
        let arguments = plan(boot("")).qemu_arguments();
        let drives: Vec<&String> = arguments.iter().filter(|word| word.contains(",file=")).collect();
        assert_eq!(
            drives,
            [
                "if=pflash,format=raw,readonly=on,file=/m/mac/OVMF_CODE.fd",
                "if=pflash,format=raw,file=/m/mac/OVMF_VARS.fd",
                "id=opencore,if=none,format=qcow2,file=/m/mac/opencore.qcow2",
                "id=disk,if=none,file=/m/mac/disk.qcow2,format=qcow2",
            ]
        );
        assert!(arguments.iter().any(|word| word == "ide-hd,bus=sata.2,drive=opencore,bootindex=1"));
        assert!(!arguments.iter().any(|word| word.contains("installer")), "macOS is installed");
        assert!(arguments.iter().any(|word| word.ends_with(",mac=52:54:00:9b:79:fe,addr=0x3")));
    }

    #[test]
    fn an_installer_is_a_disk_that_keeps_nothing_written_to_it() {
        let arguments = plan(boot("/m/mac/installer.img")).qemu_arguments();
        let installer = "id=installer,if=none,format=raw,snapshot=on,file=/m/mac/installer.img";
        assert!(arguments.iter().any(|word| word == installer));
        assert!(arguments.iter().any(|word| word == "ide-hd,bus=sata.4,drive=installer"));
    }

    #[test]
    fn only_a_macos_guest_gets_the_line_and_the_librarys_log() {
        let mac = plan(boot(""));
        assert_eq!(mac.qemu_environment(), [(LOG_VARIABLE.to_owned(), LOG_LEVEL.to_owned())]);
        let plain = plan(Boot::Plain { iso: String::new() });
        assert!(plain.macos_arguments().is_none());
        assert!(plain.qemu_environment().is_empty());
        assert!(!plain.qemu_arguments().iter().any(|word| word.contains("reims")));
    }
}
