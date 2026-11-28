use super::*;
use crate::machine::load;
use crate::network::Mode;
use crate::remote_access::{Access, NO_OPENSSH, read_access};

fn refused(context: &Context, words: &str) -> String {
    match said(context, words).1 {
        Err(Problem::Failed(failure)) => failure.0,
        other => panic!("{words:?} gave {other:?}"),
    }
}

#[test]
fn a_setting_is_changed_and_kept() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    assert_eq!(said(&context, "set work cores 1"), ("cores is now 1.\n".to_owned(), Ok(())));
    assert_eq!(said(&context, "set work ram 2").1, Ok(()));
    let machine = load(&context, "work").unwrap();
    assert_eq!((machine.cores, machine.ram_gib), (1, 2));
}

#[test]
fn a_setting_out_of_range_is_refused_with_the_range() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    assert!(refused(&context, "set work cores 0").starts_with("Cores must be from 1 to "));
    assert!(refused(&context, "set work ram 9999").starts_with("RAM in GiB must be from 1 to "));
    assert_eq!(refused(&context, "set work cores lots"), "cores must be a whole number.");
    assert_eq!(load(&context, "work").unwrap().cores, 8, "a refusal changes nothing");
}

#[test]
fn what_the_app_greys_out_is_refused_on_a_running_machine() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = make(&context, "work", "Office PC", Kind::Plain);
    running(&context, &machine, 40);
    for words in [
        "set work cores 1",
        "set work ram 2",
        "set work network none",
        "set work share none",
        "forward work add tcp 8080 80",
    ] {
        assert_eq!(refused(&context, words), "The VM must be off to change this.", "{words}");
    }
}

#[test]
fn a_setting_a_kind_of_machine_does_not_have_is_refused() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    make(&context, "desk", "Desk", Kind::Linux);
    assert_eq!(refused(&context, "set work vram 1"), "vram is only for a Windows machine with GPU-PV.");
    assert_eq!(
        refused(&context, "set work screen 1920x1080"),
        "screen is only for a Windows machine with GPU-PV."
    );
    assert_eq!(
        refused(&context, "set desk network none"),
        "An Ubuntu machine's network is fixed, with its SSH port forwarded."
    );
    assert_eq!(
        refused(&context, "forward desk add tcp 8080 80"),
        "An Ubuntu machine's network is fixed, with its SSH port forwarded."
    );
    assert!(
        refused(&context, "set work colour red")
            .starts_with("There is no setting called colour. They are cores, ram,")
    );
}

#[test]
fn the_network_the_bridge_and_the_shared_folder_are_set_by_the_pages_calls() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    assert_eq!(said(&context, "set work network bridged").1, Ok(()));
    let machine = load(&context, "work").unwrap();
    assert_eq!(machine.network_mode, Mode::Bridged);
    assert!(!machine.mac.is_empty(), "a bridged machine gets an address of its own");
    assert_eq!(said(&context, "set work bridge br1").1, Ok(()));
    assert_eq!(load(&context, "work").unwrap().bridge, "br1");
    assert_eq!(refused(&context, "set work network wifi"), "The network is nat, none or bridged.");
    assert_eq!(refused(&context, "set work passt on"), "passt is for the NAT network.");
    let folder = scratch.path("home/shared");
    fs::create_dir_all(&folder).unwrap();
    assert_eq!(said(&context, &format!("set work share {}", folder.display())).1, Ok(()));
    assert_eq!(load(&context, "work").unwrap().share, folder.display().to_string());
    assert_eq!(said(&context, "set work share none").1, Ok(()));
    assert_eq!(load(&context, "work").unwrap().share, "");
    assert!(
        refused(&context, &format!("set work share {}", scratch.path("home").display()))
            .starts_with("This folder can't be shared.")
    );
}

#[test]
fn the_screen_and_the_rate_of_a_windows_machine_are_set() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "win", "Win", Kind::Windows);
    assert_eq!(said(&context, "set win screen 2560x1440").1, Ok(()));
    assert_eq!(said(&context, "set win refresh 100").1, Ok(()));
    let machine = load(&context, "win").unwrap();
    assert_eq!((machine.screen.as_str(), machine.refresh_hz), ("2560x1440", 100));
    assert_eq!(
        refused(&context, "set win screen 800x600"),
        "The screen size must be one of 1920x1080, 2560x1440, 3840x2160."
    );
    assert_eq!(refused(&context, "set win refresh fast"), crate::machine::refresh_limits());
}

#[test]
fn a_port_is_forwarded_listed_and_taken_back() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    assert_eq!(
        said(&context, "forward work add tcp 28389 3389"),
        ("Forwarded TCP 28389 to 3389.\n".to_owned(), Ok(()))
    );
    assert!(said(&context, "show work").0.contains("forwards  TCP 28389 to 3389\n"));
    assert!(refused(&context, "forward work add tcp 28389 80").contains("already forwarded"));
    assert_eq!(refused(&context, "forward work add icmp 1 2"), "The protocol is tcp or udp.");
    assert_eq!(refused(&context, "forward work remove tcp 28389 80"), "TCP 28389 to 80 is not forwarded.");
    assert_eq!(said(&context, "forward work remove tcp 28389 3389").1, Ok(()));
    assert!(said(&context, "show work").0.contains("forwards  none\n"));
    assert!(usage_of("forward work toggle tcp 1 2").contains("forward is add or remove, not toggle."));
}

#[test]
fn window_speed_is_on_until_set_off_and_only_a_windows_machine_has_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    make(&context, "win", "Win", Kind::Windows);
    assert!(said(&context, "show win").0.contains("window-speed on\n"));
    assert_eq!(said(&context, "set win window-speed off"), ("window-speed is now off.\n".to_owned(), Ok(())));
    assert!(said(&context, "show win").0.contains("window-speed off\n"));
    assert!(!crate::machine::load(&context, "win").unwrap().window_speed());
    assert_eq!(said(&context, "set win window-speed on").1, Ok(()));
    assert!(crate::machine::load(&context, "win").unwrap().window_speed());
    assert!(refused(&context, "set work window-speed off").contains("only for a Windows machine"));
    assert!(!said(&context, "show work").0.contains("window-speed"));
}

#[test]
fn dlss_is_on_until_set_off_and_only_an_nvidia_windows_machine_has_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    make(&context, "win", "Win", Kind::Windows);
    assert!(said(&context, "show win").0.contains("dlss      on\n"));
    assert_eq!(said(&context, "set win dlss off"), ("dlss is now off.\n".to_owned(), Ok(())));
    assert!(said(&context, "show win").0.contains("dlss      off\n"));
    assert!(!crate::machine::load(&context, "win").unwrap().nvapi_wrapper());
    assert_eq!(said(&context, "set win dlss on").1, Ok(()));
    assert!(crate::machine::load(&context, "win").unwrap().nvapi_wrapper());
    assert!(refused(&context, "set work dlss off").contains("only for a Windows machine on an NVIDIA card"));
    assert!(!said(&context, "show work").0.contains("dlss"));
}

#[test]
fn the_one_interface_and_its_dns_server_are_set_and_listed() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    assert_eq!(said(&context, "set work only-through wg0").0, "only-through is now wg0.\n");
    assert_eq!(said(&context, "set work dns 10.64.0.1").1, Ok(()));
    let machine = load(&context, "work").unwrap();
    assert_eq!((machine.only_through.as_str(), machine.dns.as_str()), ("wg0", "10.64.0.1"));
    let listing = said(&context, "show work").0;
    assert!(listing.contains("only-through wg0\ndns       10.64.0.1\n"), "{listing}");
    assert_eq!(said(&context, "set work only-through OFF").1, Ok(()));
    assert_eq!(said(&context, "set work dns auto").1, Ok(()));
    let machine = load(&context, "work").unwrap();
    assert_eq!((machine.only_through.as_str(), machine.dns.as_str()), ("", ""));
}

#[test]
fn the_one_interface_is_refused_where_it_cannot_work_or_is_not_a_name() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    make(&context, "desk", "Desk", Kind::Linux);
    assert!(refused(&context, "set work only-through wg/0").contains("up to 15"));
    assert!(refused(&context, "set work dns fc00::1").contains("IPv4"));
    assert_eq!(
        refused(&context, "set desk only-through wg0"),
        "An Ubuntu machine's network is fixed, with its SSH port forwarded."
    );
    assert_eq!(said(&context, "set work network none").1, Ok(()));
    assert_eq!(refused(&context, "set work only-through wg0"), "only-through is for the NAT network.");
    let machine = make(&context, "busy", "Busy", Kind::Plain);
    running(&context, &machine, 41);
    assert_eq!(refused(&context, "set busy only-through wg0"), "The VM must be off to change this.");
    assert_eq!(refused(&context, "set busy dns 10.0.0.1"), "The VM must be off to change this.");
}

#[test]
fn show_lists_what_the_kind_of_machine_has() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    make(&context, "win", "Win", Kind::Windows);
    let plain = said(&context, "show work").0;
    assert!(plain.starts_with("name      Office PC\nid        work\nfolder    "), "{plain}");
    assert!(
        plain.contains("network   NAT\npasst     off\nonly-through off\ndns       auto\nshare     none\n")
            && !plain.contains("vram"),
        "{plain}"
    );
    let windows = said(&context, "show win").0;
    assert!(
        windows.contains("vram      no cap\nscreen    1920x1080\nrefresh   144 Hz\nsound     on\n"),
        "{windows}"
    );
}

#[test]
fn show_says_the_display_driver_and_the_vendor_id_of_a_gpu_machine() {
    use crate::display::{DisplayState, Driver};
    use crate::gpu_identity::{IdentityState, Shown};
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    let win = make(&context, "win", "Win", Kind::Windows);
    let plain = said(&context, "show work").0;
    assert!(!plain.contains("display-driver") && !plain.contains("vendor-id"), "{plain}");
    let fresh = said(&context, "show win").0;
    assert!(fresh.contains("display-driver none in this build\n"), "{fresh}");
    assert!(fresh.contains("vendor-id 1414:008e, Microsoft's\n"), "{fresh}");
    let display = DisplayState { driver: Driver::Installed, ..DisplayState::default() };
    crate::display::write_state(&context, &win, &display).unwrap();
    let identity = IdentityState { shown: Shown::Maker, restart_due: true, ..IdentityState::default() };
    crate::gpu_identity::write_state(&context, &win, &identity).unwrap();
    let moved = said(&context, "show win").0;
    assert!(moved.contains("display-driver in Windows\n"), "{moved}");
    assert!(moved.contains("vendor-id 10de:2bfe, NVIDIA's, and Windows restarts once on it\n"), "{moved}");
    let failed = DisplayState { driver: Driver::Failed, reason: "It sent no picture.".into(), ..display };
    crate::display::write_state(&context, &win, &failed).unwrap();
    let said_now = said(&context, "show win").0;
    assert!(said_now.contains("display-driver taken out, as it failed: It sent no picture.\n"), "{said_now}");
}

#[test]
fn a_machine_can_be_let_start_without_a_jail_and_says_so() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "win", "Win", Kind::Windows);
    assert!(said(&context, "show win").0.contains("start-unjailed off\n"));
    let answer = said(&context, "set win start-unjailed on");
    assert_eq!(answer, ("start-unjailed is now on.\n".to_owned(), Ok(())));
    assert!(load(&context, "win").unwrap().start_without_jail);
    assert!(said(&context, "show win").0.contains("start-unjailed on\n"));
    assert_eq!(said(&context, "set win start-unjailed off").1, Ok(()));
    assert!(!load(&context, "win").unwrap().start_without_jail);
    assert_eq!(refused(&context, "set win start-unjailed maybe"), "start-unjailed is on or off.");
}

#[test]
fn remote_desktop_and_ssh_are_chosen_for_windows_and_reach_it_at_the_next_start() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "win", "Win", Kind::Windows);
    let show = said(&context, "show win").0;
    assert!(show.contains("remote-desktop not set\n") && show.contains("ssh       not set\n"), "{show}");
    let answer = said(&context, "set win remote-desktop yes");
    let reaches = "remote-desktop is now on.\nWindows gets it at the next start.\n";
    assert_eq!(answer, (reaches.to_owned(), Ok(())));
    let machine = load(&context, "win").unwrap();
    assert_eq!(read_access(&context, &machine), Access { remote_desktop: Some(true), ssh: None });
    let show = said(&context, "show win").0;
    assert!(show.contains("remote-desktop on\n") && show.contains("ssh       not set\n"), "{show}");
    assert_eq!(refused(&context, "set win ssh on"), NO_OPENSSH);
    assert_eq!(said(&context, "set win ssh off").1, Ok(()));
    assert!(said(&context, "show win").0.contains("ssh       off\n"));
    assert_eq!(refused(&context, "set win remote-desktop maybe"), "remote-desktop is on or off.");
}

#[test]
fn a_machine_still_to_install_gets_the_change_at_its_install() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut machine = make(&context, "win", "Win", Kind::Windows);
    machine.installing = true;
    machine.save(&context).unwrap();
    let answer = said(&context, "set win remote-desktop off").0;
    assert_eq!(answer, "remote-desktop is now off.\nWindows gets it when it is installed.\n");
}

#[test]
fn only_windows_has_remote_desktop_and_ssh() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "plain", "Plain", Kind::Plain);
    assert_eq!(refused(&context, "set plain ssh off"), "ssh is only for a Windows machine with GPU-PV.");
    assert!(!said(&context, "show plain").0.contains("remote-desktop"));
}

#[test]
fn app_windows_is_off_until_set_on_and_only_a_windows_machine_has_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "win", "Win", Kind::Windows);
    make(&context, "plain", "Plain", Kind::Plain);
    assert!(said(&context, "show win").0.contains("app-windows off\n"));
    assert_eq!(said(&context, "set win app-windows on"), ("app-windows is now on.\n".to_owned(), Ok(())));
    assert!(said(&context, "show win").0.contains("app-windows on\n"));
    let machine = load(&context, "win").unwrap();
    assert!(crate::app_windows::is_on(&context, &machine));
    assert_eq!(said(&context, "set win app-windows no").0, "app-windows is now off.\n");
    assert!(!machine.folder(&context).join(crate::app_windows::SWITCH_FILE).exists());
    assert_eq!(refused(&context, "set win app-windows maybe"), "app-windows is on or off.");
    assert_eq!(
        refused(&context, "set plain app-windows on"),
        "app-windows is only for a Windows machine with GPU-PV."
    );
    assert!(!said(&context, "show plain").0.contains("app-windows"));
}

#[test]
fn a_running_machine_is_told_it_gets_the_way_in_at_its_next_start() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = make(&context, "win", "Win", Kind::Windows);
    running(&context, &machine, 40);
    let (out, result) = said(&context, "set win app-windows on");
    assert_eq!(result, Ok(()));
    assert_eq!(
        out,
        "app-windows is now on.\nA running machine gets the way in that it needs at its next start.\n"
    );
}

#[test]
fn vendor_id_retry_gets_a_machine_kept_on_microsofts_number_tried_again() {
    use crate::gpu_identity::{CHECK_TRIES, FELL_BACK, IdentityState, Shown, read_state, write_state};
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Plain);
    make(&context, "win", "Win", Kind::Windows);
    let machine = load(&context, "win").unwrap();
    let stuck = IdentityState {
        failed_script: "abc".into(),
        reason: FELL_BACK.into(),
        failed_checks: CHECK_TRIES,
        ..IdentityState::default()
    };
    write_state(&context, &machine, &stuck).unwrap();
    assert!(said(&context, "status win").0.contains("guesthouse-vm set win vendor-id retry"));
    assert_eq!(
        said(&context, "set win vendor-id retry"),
        ("The next start tries NVIDIA's vendor ID again.\n".to_owned(), Ok(()))
    );
    assert_eq!(read_state(&context, &machine), IdentityState::default());
    assert!(!said(&context, "status win").0.contains("vendor ID"));
    assert_eq!(refused(&context, "set win vendor-id 10de"), "vendor-id takes retry.");
    assert!(refused(&context, "set work vendor-id retry").contains("only for a Windows machine"));

    let settled = IdentityState { shown: Shown::Maker, ..IdentityState::default() };
    write_state(&context, &machine, &settled).unwrap();
    assert_eq!(
        said(&context, "set win vendor-id retry"),
        ("Windows is already shown NVIDIA's vendor ID.\n".to_owned(), Ok(()))
    );
    assert_eq!(read_state(&context, &machine), settled);
}

/// A disk built on a file by hand is started once its settings name that file, and only that one.
#[test]
fn the_files_a_disk_is_built_on_are_recorded_only_as_its_header_names_them() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let (base, disk) = (scratch.path("bases/base.qcow2"), scratch.path("work/disk.qcow2"));
    for file in [&base, &disk] {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
    }
    let qemu_img = |words: &[&str]| {
        assert!(std::process::Command::new("qemu-img").args(words).status().unwrap().success());
    };
    qemu_img(&["create", "-q", "-f", "qcow2", base.to_str().unwrap(), "1G"]);
    qemu_img(&[
        "create",
        "-q",
        "-f",
        "qcow2",
        "-F",
        "qcow2",
        "-b",
        "../bases/base.qcow2",
        disk.to_str().unwrap(),
    ]);
    let machine =
        Machine { kind: Kind::Plain, ..Machine::new("work", "Office PC", disk.to_str().unwrap(), "/v") };
    fs::create_dir_all(machine.folder(&context)).unwrap();
    machine.save(&context).unwrap();
    let other = scratch.write("other.img", vec![0; 4096]);
    let refused_with = |value: &str| refused(&context, &format!("set work backing-files {value}"));
    assert!(refused_with(other.to_str().unwrap()).starts_with("The disk is built on "));
    assert!(refused_with("none").contains("/bases/base.qcow2 (qcow2)"));
    assert!(load(&context, "work").unwrap().backing_files.is_empty(), "a refusal changes nothing");
    let set = said(&context, &format!("set work backing-files {}", base.display()));
    assert_eq!(set.1, Ok(()));
    let machine = load(&context, "work").unwrap();
    let recorded: Vec<(&str, &str)> =
        machine.backing_files.iter().map(|file| (file.path.as_str(), file.format.as_str())).collect();
    assert_eq!(recorded, [(base.canonicalize().unwrap().to_str().unwrap(), "qcow2")]);
    assert_eq!(
        crate::disk_image::backing::checked_chain(&disk, &machine.backing_files).unwrap().len(),
        1,
        "a start takes it"
    );
    assert!(said(&context, "show work").0.contains("backing-files"));
}
