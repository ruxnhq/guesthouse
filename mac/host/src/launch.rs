// SPDX-License-Identifier: GPL-2.0-or-later
//! What starting a machine runs: the QEMU command line for a Windows guest with QEMU's own
//! graphics device and frame export, the same one `win/run.sh` builds for `WIN_PVG=qemu
//! WIN_RAMFB=no WIN_RMHOST=NAME`. Where the lab script is given a run folder and changes into
//! it, the paths here are absolute, so nothing depends on where the agent was started.
//!
//! The guarded path (`WIN_PVG=host`) runs the device in `guesthouse-pvg-host`, a process of its
//! own, and QEMU forwards to it through `pvg-fwd` over shared guest memory. `host_args` is the
//! process's command line and `qemu_args` makes QEMU's for either path, by `Launch::graphics`.
//! Lines of run.sh that are only for debugging (the `-trace` options) are left out.
//!
//! A Linux machine (`kind` `linux`) gets three things more: QEMU's user-mode network with a
//! virtio card, which its packages come over; its clock in UTC, as Linux keeps it; and the
//! scale its desktop should draw at, as a firmware file the guest reads (`SCALE_FILE`). A
//! Windows machine's line is run.sh's, as before.

use std::path::{Path, PathBuf};

/// The modes the device offers when a machine names none, as in run.sh.
pub const DEFAULT_MODES: [&str; 4] = ["640x480@60", "1024x768@60", "1280x800@60", "1920x1080@60"];

/// The `kind` in `machine.json` that gets a network and a scale.
pub const LINUX: &str = "linux";

/// A Linux machine's network card, on QEMU's user-mode network. With no option ROM: the
/// firmware has no use for booting from the network, and QEMU then needs no `efi-virtio.rom`
/// from its data folder, which a QEMU copied out of its build folder does not find.
pub const NETWORK_DEVICE: &str = "virtio-net-pci,netdev=net,romfile=";
/// QEMU's own NAT, with the Mac's loopback out of the guest's reach: 10.0.2.2 leads to no service on
/// it. `host-loopback=off` is in qemu/mac/patches/0007-host-loopback.patch.
pub const NETWORK: &str = "user,id=net,host-loopback=off";

/// The firmware file (QEMU's `-fw_cfg`) a Linux guest reads its scale from: `1` or `2`.
pub const SCALE_FILE: &str = "opt/com.ruxn.guesthouse/scale";

/// The firmware file a Linux guest reads its keyboard's layout from, as XKB names it: the layout,
/// and the variant after a space (`de`, `ch fr`). Only when the Mac's is known.
pub const KEYBOARD_FILE: &str = "opt/com.ruxn.guesthouse/keyboard";

/// The scales a guest is started with: 2 for Sharp, 1 for Standard.
pub const SCALES: [u32; 2] = [1, 2];

/// The scale when the app says nothing: Sharp, as the app's own default is.
pub const DEFAULT_SCALE: u32 = 2;

/// The longest socket path macOS takes, less its ending nul.
pub const MAX_SOCKET_PATH: usize = 103;

/// Where a machine's graphics device lives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Graphics {
    /// In `guesthouse-pvg-host`, a process of its own behind a guard, which QEMU forwards to.
    /// What a machine gets unless it says otherwise.
    #[default]
    Guarded,
    /// QEMU's own device, inside QEMU. Kept for comparison and for tests.
    Qemu,
}

impl Graphics {
    pub fn name(self) -> &'static str {
        match self {
            Graphics::Guarded => "guarded",
            Graphics::Qemu => "qemu",
        }
    }

    pub fn from_name(name: &str) -> Option<Graphics> {
        match name {
            "guarded" => Some(Graphics::Guarded),
            "qemu" => Some(Graphics::Qemu),
            _ => None,
        }
    }
}

/// An installer started by the firmware from a kernel QEMU hands it (`-kernel`), with the kernel
/// line the installer needs to run on its own. Ubuntu's installer asks before it writes the disk
/// unless its kernel line says `autoinstall`, and its own boot menu does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerBoot {
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub command_line: String,
}

/// Everything the command line is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub graphics: Graphics,
    /// `machine.json`'s `kind`: `windows`, `linux`, or a label of the person's own.
    pub kind: String,
    /// The scale a Linux guest draws its desktop at, from the app's Sharp or Standard when it
    /// starts the machine (`SCALES`). Not passed to other guests.
    pub scale: u32,
    /// The Mac's keyboard layout for a Linux guest, as XKB names it (`KEYBOARD_FILE`), when it is
    /// known: set when the machine starts. Not passed to other guests.
    pub keyboard: Option<String>,
    pub cores: u32,
    pub memory_mb: u64,
    pub disk: PathBuf,
    pub firmware_code: PathBuf,
    pub firmware_vars: PathBuf,
    pub stick: Option<PathBuf>,
    /// An installer disc to boot from, while a machine is being installed.
    pub installer: Option<PathBuf>,
    /// The answers the installer reads, on a disc of their own.
    pub answer_disc: Option<PathBuf>,
    /// The installer's kernel, when the installer disc is not to be booted by its own menu.
    pub installer_boot: Option<InstallerBoot>,
    pub option_rom: Option<PathBuf>,
    pub modes: Vec<String>,
    /// The bootstrap name the device publishes its frames under.
    pub frame_name: String,
    /// The machine's run folder: sockets, the log and the pid file.
    pub run_dir: PathBuf,
    /// The QMP socket the agent controls the machine through.
    pub control_socket: PathBuf,
    /// The QMP socket the app's input goes to.
    pub input_socket: PathBuf,
    /// A QMP socket for the screen sharing process. A QMP socket takes one client at a time, so
    /// it cannot be the app's input socket: viewing from another device would lock the app's
    /// own window out of the keyboard, or the other way round.
    pub remote_socket: PathBuf,
    /// Guarded path: where QEMU's `pvg-fwd` finds the graphics process, and the socket that
    /// process is controlled and asked its `status` through.
    pub host_socket: PathBuf,
    pub host_control: PathBuf,
}

/// Why a socket cannot live at one of `sockets`, if one is too long for a socket address.
pub fn socket_problem(sockets: &[&Path]) -> Option<String> {
    sockets.iter().find(|socket| socket.as_os_str().len() > MAX_SOCKET_PATH).map(|socket| {
        format!("{} is too long for a socket; give the machine's folder a shorter name", socket.display())
    })
}

impl Launch {
    /// Why this cannot be started as it stands, if it cannot.
    pub fn problem(&self) -> Option<String> {
        let mut sockets = vec![&self.control_socket, &self.input_socket, &self.remote_socket];
        if self.graphics == Graphics::Guarded {
            sockets.extend([&self.host_socket, &self.host_control]);
        }
        let sockets: Vec<&Path> = sockets.into_iter().map(PathBuf::as_path).collect();
        if let Some(problem) = socket_problem(&sockets) {
            return Some(problem);
        }
        for (what, file) in [
            ("disk image", &self.disk),
            ("firmware code", &self.firmware_code),
            ("firmware variables", &self.firmware_vars),
        ] {
            if !file.exists() {
                return Some(format!("the {what} {} is not there", file.display()));
            }
        }
        None
    }
}

/// The arguments for QEMU, in run.sh's order.
pub fn qemu_args(launch: &Launch) -> Vec<String> {
    let mut args = Vec::new();
    let mut add = |items: &[&str]| args.extend(items.iter().map(|item| item.to_string()));
    let path = |path: &Path| path.display().to_string();

    if launch.graphics == Graphics::Guarded {
        // Shared guest memory, which the graphics process maps too. run.sh puts these first,
        // before its own plain `-M virt`, which QEMU merges with this one.
        add(&["-M", "virt,memory-backend=ram"]);
        add(&["-object", &format!("memory-backend-shm,id=ram,size={}M", launch.memory_mb)]);
    }
    add(&["-M", "virt", "-accel", "hvf", "-cpu", "host"]);
    add(&["-m", &launch.memory_mb.to_string()]);
    add(&["-smp", &launch.cores.to_string()]);
    add(&["-drive", &format!("if=pflash,format=raw,readonly=on,file={}", path(&launch.firmware_code))]);
    add(&["-drive", &format!("if=pflash,format=raw,file={}", path(&launch.firmware_vars))]);
    add(&["-display", "none"]);
    add(&["-qmp", &format!("unix:{},server,nowait", path(&launch.control_socket))]);
    add(&["-serial", &format!("file:{}", path(&launch.run_dir.join("serial.log")))]);
    // Only a Linux machine has a network, and its scale as a file for the guest to read.
    let linux = launch.kind == LINUX;
    if linux {
        add(&["-netdev", NETWORK, "-device", NETWORK_DEVICE]);
    } else {
        add(&["-nic", "none"]);
    }
    // Windows keeps the clock in local time, Linux in UTC.
    add(&["-rtc", if linux { "base=utc" } else { "base=localtime" }]);
    if linux {
        add(&["-fw_cfg", &format!("name={SCALE_FILE},string={}", launch.scale)]);
        if let Some(keyboard) = &launch.keyboard {
            add(&["-fw_cfg", &format!("name={KEYBOARD_FILE},string={keyboard}")]);
        }
    }
    add(&["-device", "qemu-xhci,id=xhci", "-device", "usb-kbd", "-device", "usb-tablet"]);
    add(&["-drive", &format!("if=none,id=hd,format=raw,file={},discard=unmap", path(&launch.disk))]);
    add(&["-device", "nvme,drive=hd,serial=win0,bootindex=1"]);
    // No ramfb: Windows draws into the PV device's own framebuffer.
    // The installer boots first, as run.sh's WIN_ISO does; the answers are a second disc.
    if let Some(installer) = &launch.installer {
        add(&[
            "-drive",
            &format!("if=none,id=cd,format=raw,media=cdrom,readonly=on,file={}", path(installer)),
        ]);
        add(&["-device", "usb-storage,drive=cd,bootindex=0"]);
    }
    // The firmware starts a kernel it is handed before it looks at any disk.
    if let Some(boot) = &launch.installer_boot {
        add(&["-kernel", &path(&boot.kernel), "-initrd", &path(&boot.initrd)]);
        add(&["-append", &boot.command_line]);
    }
    if let Some(answers) = &launch.answer_disc {
        add(&[
            "-drive",
            &format!("if=none,id=answers,format=raw,media=cdrom,readonly=on,file={}", path(answers)),
        ]);
        add(&["-device", "usb-storage,drive=answers"]);
    }
    if let Some(stick) = &launch.stick {
        add(&["-drive", &format!("if=none,id=stick,format=raw,file={}", path(stick))]);
        add(&["-device", "usb-storage,drive=stick,removable=on"]);
    }
    add(&["-qmp", &format!("unix:{},server,nowait", path(&launch.input_socket))]);
    // Always there, so the screen can be shared with a machine that is already running. With
    // nobody connected it costs nothing.
    add(&["-qmp", &format!("unix:{},server,nowait", path(&launch.remote_socket))]);
    match launch.graphics {
        Graphics::Qemu => add(&["-device", &graphics_device(launch)]),
        Graphics::Guarded => add(&["-device", &forwarding_device(launch)]),
    }
    args
}

/// The arguments for `guesthouse-pvg-host`, in run.sh's order. No `--window`: the picture goes
/// to the app through the frames it publishes under the machine's name.
pub fn host_args(launch: &Launch) -> Vec<String> {
    vec![
        "--socket".into(),
        launch.host_socket.display().to_string(),
        "--control".into(),
        launch.host_control.display().to_string(),
        "--frames".into(),
        launch.frame_name.clone(),
        "--modes".into(),
        launch.modes.join(","),
    ]
}

/// QEMU's end of the guarded path: it forwards the device's registers and interrupts to the
/// graphics process, whose socket it names.
fn forwarding_device(launch: &Launch) -> String {
    let mut device = format!("pvg-fwd,id=pvg,socket={}", launch.host_socket.display());
    if let Some(rom) = &launch.option_rom {
        device += &format!(",romfile={}", rom.display());
    }
    device
}

/// The PV device as the JSON form of `-device`, with its properties in run.sh's order.
fn graphics_device(launch: &Launch) -> String {
    let text = |value: &str| serde_json::to_string(value).expect("a string is JSON");
    let modes: Vec<String> = launch.modes.iter().map(|mode| text(mode)).collect();
    let mut device =
        format!(r#"{{"driver":"apple-gfx-pci","id":"pvg","display-modes":[{}]"#, modes.join(","));
    if let Some(rom) = &launch.option_rom {
        device += &format!(r#","romfile":{}"#, text(&rom.display().to_string()));
    }
    device += &format!(r#","frame-export":{}}}"#, text(&launch.frame_name));
    device
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> Launch {
        Launch {
            kind: "windows".into(),
            scale: DEFAULT_SCALE,
            keyboard: None,
            cores: 4,
            memory_mb: 6144,
            disk: "/m/win/win.img".into(),
            firmware_code: "/m/win/code.fd".into(),
            firmware_vars: "/m/win/vars.fd".into(),
            stick: None,
            installer: None,
            answer_disc: None,
            installer_boot: None,
            option_rom: Some("/m/win/pvgfb.rom".into()),
            modes: vec!["1024x768@60".into()],
            frame_name: "guesthouse.win".into(),
            run_dir: "/s/Run/win".into(),
            control_socket: "/s/Run/win/qmp.sock".into(),
            input_socket: "/s/Run/win/input.sock".into(),
            remote_socket: "/s/Run/win/remote.sock".into(),
            graphics: Graphics::Qemu,
            host_socket: "/s/Run/win/host.sock".into(),
            host_control: "/s/Run/win/control.sock".into(),
        }
    }

    fn guarded() -> Launch {
        Launch { graphics: Graphics::Guarded, ..launch() }
    }

    /// What run.sh passes for `WIN_PVG=host WIN_RMHOST=guesthouse.win WIN_MODES=1024x768@60
    /// WIN_ROM=/m/win/pvgfb.rom` with `-m 6144 -smp 4`: the same as the QEMU path from the
    /// second `-M` to the input sockets, with the shared memory first and `pvg-fwd` last in
    /// place of `apple-gfx-pci`, and the sockets made absolute.
    fn expected_guarded_like_run_sh() -> Vec<String> {
        let mut want: Vec<String> = expected_like_run_sh().iter().map(|a| a.to_string()).collect();
        want.pop();
        want.push("pvg-fwd,id=pvg,socket=/s/Run/win/host.sock,romfile=/m/win/pvgfb.rom".into());
        let mut first: Vec<String> =
            ["-M", "virt,memory-backend=ram", "-object", "memory-backend-shm,id=ram,size=6144M"]
                .iter()
                .map(|a| a.to_string())
                .collect();
        first.extend(want);
        first
    }

    #[test]
    fn the_guarded_qemu_line_matches_run_sh_argument_by_argument() {
        let got = qemu_args(&guarded());
        let want = expected_guarded_like_run_sh();
        for (index, (got, want)) in got.iter().zip(&want).enumerate() {
            assert_eq!(got, want, "argument {index}");
        }
        assert_eq!(got.len(), want.len());
        // Nothing of QEMU's own device is left, and no frame export on QEMU.
        assert!(!got.iter().any(|a| a.contains("apple-gfx") || a.contains("frame-export")));
    }

    #[test]
    fn the_graphics_process_gets_its_sockets_frame_name_and_modes_as_run_sh_gives_them() {
        let mut launch = guarded();
        launch.modes = vec!["1024x768@60".into(), "640x480@60".into()];
        assert_eq!(
            host_args(&launch),
            [
                "--socket",
                "/s/Run/win/host.sock",
                "--control",
                "/s/Run/win/control.sock",
                "--frames",
                "guesthouse.win",
                "--modes",
                "1024x768@60,640x480@60"
            ]
        );
        assert!(!host_args(&launch).contains(&"--window".to_string()));
    }

    #[test]
    fn without_an_option_rom_pvg_fwd_has_no_romfile_and_the_graphics_sockets_count_for_length() {
        let mut plain = guarded();
        plain.option_rom = None;
        assert_eq!(qemu_args(&plain).last().unwrap(), "pvg-fwd,id=pvg,socket=/s/Run/win/host.sock");
        let mut long = guarded();
        long.host_control = PathBuf::from("/x".repeat(60));
        assert!(long.problem().unwrap().contains("too long"));
        // On the QEMU path that socket is not used, so its length does not matter.
        long.graphics = Graphics::Qemu;
        assert!(long.problem().unwrap().contains("is not there"));
    }

    /// What run.sh passes for `WIN_PVG=qemu WIN_RAMFB=no WIN_RMHOST=guesthouse.win
    /// WIN_MODES=1024x768@60 WIN_ROM=/m/win/pvgfb.rom` with `-m 6144 -smp 4`, read off its lines
    /// in order, with its relative names (qmp.sock, serial.log, rmhost-qmp.sock) made absolute
    /// as above. The trace options it adds for debugging (`-trace 'apple_gfx*' -D trace.log`)
    /// are left out on purpose.
    fn expected_like_run_sh() -> Vec<&'static str> {
        vec![
            "-M",
            "virt",
            "-accel",
            "hvf",
            "-cpu",
            "host",
            "-m",
            "6144",
            "-smp",
            "4",
            "-drive",
            "if=pflash,format=raw,readonly=on,file=/m/win/code.fd",
            "-drive",
            "if=pflash,format=raw,file=/m/win/vars.fd",
            "-display",
            "none",
            "-qmp",
            "unix:/s/Run/win/qmp.sock,server,nowait",
            "-serial",
            "file:/s/Run/win/serial.log",
            "-nic",
            "none",
            "-rtc",
            "base=localtime",
            "-device",
            "qemu-xhci,id=xhci",
            "-device",
            "usb-kbd",
            "-device",
            "usb-tablet",
            "-drive",
            "if=none,id=hd,format=raw,file=/m/win/win.img,discard=unmap",
            "-device",
            "nvme,drive=hd,serial=win0,bootindex=1",
            "-qmp",
            "unix:/s/Run/win/input.sock,server,nowait",
            "-qmp",
            "unix:/s/Run/win/remote.sock,server,nowait",
            "-device",
            r#"{"driver":"apple-gfx-pci","id":"pvg","display-modes":["1024x768@60"],"romfile":"/m/win/pvgfb.rom","frame-export":"guesthouse.win"}"#,
        ]
    }

    #[test]
    fn matches_run_sh_argument_by_argument() {
        let got = qemu_args(&launch());
        let want = expected_like_run_sh();
        for (index, (got, want)) in got.iter().zip(&want).enumerate() {
            assert_eq!(got, want, "argument {index}");
        }
        assert_eq!(got.len(), want.len());
    }

    /// A Linux machine's line is the Windows one with the user-mode network and its card in
    /// place of `-nic none`, the clock in UTC and the scale file after `-rtc`, on either path;
    /// nothing else moves.
    #[test]
    fn a_linux_machine_gets_a_network_and_its_scale_and_nothing_else_changes() {
        for (graphics, windows) in
            [(Graphics::Qemu, qemu_args(&launch())), (Graphics::Guarded, qemu_args(&guarded()))]
        {
            let linux = Launch { graphics, kind: LINUX.into(), scale: 1, ..launch() };
            let got = qemu_args(&linux);
            let mut want = windows.clone();
            let nic = want.iter().position(|arg| arg == "-nic").unwrap();
            assert_eq!(want[nic + 1], "none");
            assert_eq!(want[nic + 2], "-rtc");
            let network =
                ["-netdev", "user,id=net,host-loopback=off", "-device", "virtio-net-pci,netdev=net,romfile="];
            want.splice(nic..nic + 2, network.map(String::from));
            let rtc = want.iter().position(|arg| arg == "-rtc").unwrap();
            assert_eq!(want[rtc + 1], "base=localtime");
            want[rtc + 1] = "base=utc".into();
            let scale = ["-fw_cfg", "name=opt/com.ruxn.guesthouse/scale,string=1"];
            want.splice(rtc + 2..rtc + 2, scale.map(String::from));
            for (index, (got, want)) in got.iter().zip(&want).enumerate() {
                assert_eq!(got, want, "argument {index}");
            }
            assert_eq!(got.len(), want.len());
        }
    }

    #[test]
    fn a_linux_machine_is_told_the_mac_s_keyboard_after_its_scale_and_other_kinds_are_not() {
        let linux = Launch { kind: LINUX.into(), keyboard: Some("ch fr".into()), ..guarded() };
        let args = qemu_args(&linux);
        let scale = args.iter().position(|arg| arg.contains("guesthouse/scale")).unwrap();
        assert_eq!(args[scale + 1], "-fw_cfg");
        assert_eq!(args[scale + 2], "name=opt/com.ruxn.guesthouse/keyboard,string=ch fr");
        let windows = Launch { keyboard: Some("de".into()), ..guarded() };
        assert!(!qemu_args(&windows).iter().any(|arg| arg.contains("keyboard")));
    }

    #[test]
    fn a_sharp_linux_machine_is_told_2_and_other_kinds_are_told_nothing() {
        let linux = Launch { kind: LINUX.into(), ..guarded() };
        assert!(qemu_args(&linux).contains(&"name=opt/com.ruxn.guesthouse/scale,string=2".to_string()));
        for kind in ["windows", "Linux", "other"] {
            let other = Launch { kind: kind.into(), scale: 1, ..guarded() };
            let args = qemu_args(&other);
            assert!(!args.iter().any(|arg| arg == "-fw_cfg"), "{kind}");
            assert!(args.windows(2).any(|pair| pair == ["-nic", "none"]), "{kind}");
        }
    }

    #[test]
    fn a_stick_comes_after_the_disk_and_before_the_input_socket() {
        let mut with_stick = launch();
        with_stick.stick = Some("/m/win/stick.img".into());
        let got = qemu_args(&with_stick);
        let mut want = expected_like_run_sh();
        let at = want.iter().position(|arg| arg.starts_with("unix:/s/Run/win/input")).unwrap() - 1;
        want.splice(
            at..at,
            [
                "-drive",
                "if=none,id=stick,format=raw,file=/m/win/stick.img",
                "-device",
                "usb-storage,drive=stick,removable=on",
            ],
        );
        assert_eq!(got, want);
    }

    /// run.sh's WIN_ISO then WIN_STICK, after the disk: the installer boots first, and the answers
    /// are a disc of their own.
    #[test]
    fn an_installer_and_its_answers_come_after_the_disk_and_before_the_stick() {
        let mut installing = launch();
        installing.installer = Some("/m/win/Win11.iso".into());
        installing.answer_disc = Some("/m/win/answers.iso".into());
        installing.stick = Some("/m/win/stick.img".into());
        let got = qemu_args(&installing);
        let mut want: Vec<String> = expected_like_run_sh().iter().map(|a| a.to_string()).collect();
        let at = want.iter().position(|arg| arg.starts_with("unix:/s/Run/win/input")).unwrap() - 1;
        let extra = [
            "-drive",
            "if=none,id=cd,format=raw,media=cdrom,readonly=on,file=/m/win/Win11.iso",
            "-device",
            "usb-storage,drive=cd,bootindex=0",
            "-drive",
            "if=none,id=answers,format=raw,media=cdrom,readonly=on,file=/m/win/answers.iso",
            "-device",
            "usb-storage,drive=answers",
            "-drive",
            "if=none,id=stick,format=raw,file=/m/win/stick.img",
            "-device",
            "usb-storage,drive=stick,removable=on",
        ];
        want.splice(at..at, extra.iter().map(|a| a.to_string()));
        assert_eq!(got, want);
    }

    /// Ubuntu's installer: its disc and answers as Windows', then its kernel, initrd and kernel
    /// line for the firmware, and nothing else changed.
    #[test]
    fn an_installer_kernel_comes_right_after_the_installer_disc() {
        let mut installing = launch();
        installing.kind = LINUX.into();
        installing.installer = Some("/m/u/ubuntu.iso".into());
        installing.answer_disc = Some("/m/u/answers.iso".into());
        installing.installer_boot = Some(InstallerBoot {
            kernel: "/m/u/installer-boot/casper/vmlinuz".into(),
            initrd: "/m/u/installer-boot/casper/initrd".into(),
            command_line: "autoinstall ds=nocloud ---".into(),
        });
        let got = qemu_args(&installing);
        let mut without = installing.clone();
        without.installer_boot = None;
        let mut want = qemu_args(&without);
        let at = want.iter().position(|arg| arg == "usb-storage,drive=cd,bootindex=0").unwrap() + 1;
        let extra = [
            "-kernel",
            "/m/u/installer-boot/casper/vmlinuz",
            "-initrd",
            "/m/u/installer-boot/casper/initrd",
            "-append",
            "autoinstall ds=nocloud ---",
        ];
        want.splice(at..at, extra.iter().map(|a| a.to_string()));
        assert_eq!(got, want);
        assert!(!qemu_args(&launch()).contains(&"-kernel".to_string()));
    }

    #[test]
    fn no_option_rom_leaves_romfile_out_and_several_modes_are_listed_in_order() {
        let mut plain = launch();
        plain.option_rom = None;
        plain.modes = DEFAULT_MODES.iter().map(|mode| mode.to_string()).collect();
        let device = qemu_args(&plain).pop().unwrap();
        assert_eq!(
            device,
            r#"{"driver":"apple-gfx-pci","id":"pvg","display-modes":["640x480@60","1024x768@60","1280x800@60","1920x1080@60"],"frame-export":"guesthouse.win"}"#
        );
        let parsed: serde_json::Value = serde_json::from_str(&device).unwrap();
        assert_eq!(parsed["display-modes"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn paths_with_quotes_or_spaces_stay_valid_json_and_one_argument() {
        let mut odd = launch();
        odd.option_rom = Some("/m/my \"rom\".rom".into());
        odd.disk = "/m/My Disk.img".into();
        let args = qemu_args(&odd);
        let device: serde_json::Value = serde_json::from_str(args.last().unwrap()).unwrap();
        assert_eq!(device["romfile"], "/m/my \"rom\".rom");
        assert!(args.contains(&"if=none,id=hd,format=raw,file=/m/My Disk.img,discard=unmap".to_string()));
    }

    #[test]
    fn long_socket_paths_and_missing_files_are_reported_before_starting() {
        let mut long = launch();
        long.input_socket = PathBuf::from("/x".repeat(60));
        assert!(long.problem().unwrap().contains("too long"));
        // The files in the fixture do not exist.
        assert!(launch().problem().unwrap().contains("is not there"));
    }
}
