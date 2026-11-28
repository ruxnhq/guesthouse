use super::*;
use crate::network::{Forward, Protocol};
use crate::testing::{Scratch, scratch_context};

fn sample() -> Machine {
    Machine::new("sample", "Sample", "/d", "/v")
}

#[test]
fn a_plain_machine_with_an_opencore_image_is_a_mac_and_says_so() {
    let plain = Machine { kind: Kind::Plain, ..sample() };
    assert!(!plain.macos() && plain.summary().ends_with("no GPU acceleration"));
    let mac = Machine { opencore: "/m/mac/opencore.qcow2".into(), ..plain };
    assert!(mac.macos());
    assert_eq!(mac.summary(), "macOS, 8 cores, 16 GiB, 1920x1080, GPU preview");
    assert!(!Machine { opencore: "/o".into(), ..sample() }.macos(), "only a plain machine");
}

#[test]
fn a_windows_guest_on_an_amd_card_names_it_and_a_linux_one_does_not_count() {
    assert_eq!((sample().windows_maker(), sample().windows_card_address()), (Some(GpuMaker::Nvidia), None));
    let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..sample() };
    assert_eq!(on_amd.windows_maker(), Some(GpuMaker::Amd));
    assert_eq!(on_amd.windows_card_address(), Some("0000:7c:00.0"));
    let linux = Machine { kind: Kind::Linux, ..on_amd };
    assert_eq!((linux.windows_maker(), linux.windows_card_address()), (None, None));
}

#[test]
fn a_windows_guest_moves_only_to_a_card_of_the_same_maker() {
    use GpuMaker::{Amd, Intel, Nvidia};
    let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..sample() };
    let on_intel = Machine { gpu_maker: Some(Intel), ..on_amd.clone() };
    for (machine, own) in [(sample(), Nvidia), (on_amd, Amd), (on_intel, Intel)] {
        for card in GpuMaker::ALL {
            let problem = machine.card_change_problem(card);
            assert_eq!(problem.is_none(), card == own, "{own:?} to {card:?}");
            assert!(problem.is_none_or(|text| text.contains("make a new machine")));
        }
    }
    let linux = Machine { kind: Kind::Linux, ..sample() };
    assert_eq!(linux.card_change_problem(Amd), None, "a Linux guest builds its Mesa again");
}

#[test]
fn a_file_from_before_other_makers_says_nvidia_for_no_address_and_amd_for_one() {
    let nvidia: Machine = serde_json::from_str(&without_maker(&sample())).unwrap();
    assert_eq!(nvidia.windows_maker(), Some(GpuMaker::Nvidia));
    let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..sample() };
    let amd: Machine = serde_json::from_str(&without_maker(&on_amd)).unwrap();
    assert_eq!(
        (amd.windows_maker(), amd.windows_card_address()),
        (Some(GpuMaker::Amd), Some("0000:7c:00.0"))
    );
    let linux = Machine { kind: Kind::Linux, ..on_amd };
    assert_eq!(linux.windows_maker(), None, "only Windows has a GPU-PV card");
}

fn without_maker(machine: &Machine) -> String {
    let mut file: serde_json::Value = serde_json::to_value(machine).unwrap();
    file.as_object_mut().unwrap().remove("gpu_maker");
    file.to_string()
}

#[test]
fn nvidia_and_amd_machines_are_written_as_the_released_app_reads_them() {
    let mut machine = sample();
    for (maker, pci) in [(GpuMaker::Nvidia, ""), (GpuMaker::Amd, "0000:7c:00.0")] {
        machine.use_windows_card(maker, pci);
        let text = serde_json::to_string(&machine).unwrap();
        assert!(!text.contains("gpu_maker"), "{maker:?}: {text}");
        assert_eq!(text.contains("amd_gpu"), !pci.is_empty());
        let read: Machine = serde_json::from_str(&text).unwrap();
        assert_eq!(read.windows_maker(), Some(maker));
    }
}

#[test]
fn a_machine_given_an_nvidia_card_by_its_address_writes_its_maker_so_it_is_not_taken_for_amd() {
    let mut machine = sample();
    machine.use_windows_card(GpuMaker::Nvidia, "0000:02:00.0");
    let text = serde_json::to_string(&machine).unwrap();
    assert!(text.contains(r#""gpu_maker":"nvidia""#), "{text}");
    let read: Machine = serde_json::from_str(&text).unwrap();
    assert_eq!(
        (read.windows_maker(), read.windows_card_address()),
        (Some(GpuMaker::Nvidia), Some("0000:02:00.0"))
    );
    machine.use_windows_card(GpuMaker::Nvidia, "");
    assert!(!serde_json::to_string(&machine).unwrap().contains("gpu_maker"), "setup's card, as before");
}

#[test]
fn an_intel_machine_keeps_its_maker_through_a_round_trip_and_an_old_file_reader_refuses_it() {
    let mut machine = sample();
    machine.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
    let text = serde_json::to_string(&machine).unwrap();
    assert!(text.contains(r#""gpu_maker":"intel""#), "{text}");
    let read: Machine = serde_json::from_str(&text).unwrap();
    assert_eq!(Machine { id: machine.id.clone(), ..read.clone() }, machine, "the id is the folder's name");
    assert_eq!(read.windows_maker(), Some(GpuMaker::Intel));
    // The released app's Machine rejects fields it doesn't know, which is what keeps it from taking
    // this one for an AMD machine. This build rejects an unknown field the same way.
    let mut unknown: serde_json::Value = serde_json::from_str(&text).unwrap();
    unknown["a_later_field"] = true.into();
    assert!(serde_json::from_value::<Machine>(unknown).is_err());
}

#[test]
fn an_older_build_can_read_a_machine_that_keeps_its_jail() {
    let text = serde_json::to_string(&sample()).unwrap();
    assert!(!text.contains("start_without_jail"), "{text}");
    let allowed = serde_json::to_string(&Machine { start_without_jail: true, ..sample() }).unwrap();
    let read: Machine = serde_json::from_str(&allowed).unwrap();
    assert!(read.start_without_jail);
}

#[test]
fn a_machine_on_sata_is_one_an_older_build_can_read() {
    let text = serde_json::to_string(&sample()).unwrap();
    assert!(!text.contains("nvme"), "{text}");
    assert!(!serde_json::from_str::<Machine>(&text).unwrap().nvme);
    let on_nvme = serde_json::to_string(&Machine { nvme: true, ..sample() }).unwrap();
    assert!(serde_json::from_str::<Machine>(&on_nvme).unwrap().nvme);
}

#[test]
fn programs_in_a_window_are_sped_up_unless_turned_off() {
    let text = serde_json::to_string(&sample()).unwrap();
    assert!(!text.contains("window_speed"), "an older build can't read this: {text}");
    assert!(serde_json::from_str::<Machine>(&text).unwrap().window_speed());
    let turned_off = serde_json::to_string(&Machine { window_speed_off: true, ..sample() }).unwrap();
    assert!(!serde_json::from_str::<Machine>(&turned_off).unwrap().window_speed());
}

#[test]
fn files_can_be_dragged_to_and_from_a_windows_guest_unless_turned_off() {
    let text = serde_json::to_string(&sample()).unwrap();
    assert!(!text.contains("file_drop"), "an older build can't read this: {text}");
    assert!(serde_json::from_str::<Machine>(&text).unwrap().file_drop());
    let turned_off = serde_json::to_string(&Machine { file_drop_off: true, ..sample() }).unwrap();
    assert!(!serde_json::from_str::<Machine>(&turned_off).unwrap().file_drop());
    assert!(!Machine { kind: Kind::Linux, ..sample() }.file_drop());
    assert!(!Machine { installing: true, ..sample() }.file_drop());
}

#[test]
fn the_nvapi_wrapper_goes_in_unless_turned_off() {
    let text = serde_json::to_string(&sample()).unwrap();
    assert!(!text.contains("nvapi"), "an older build can't read this: {text}");
    assert!(serde_json::from_str::<Machine>(&text).unwrap().nvapi_wrapper());
    let turned_off = serde_json::to_string(&Machine { nvapi_wrapper_off: true, ..sample() }).unwrap();
    assert!(!serde_json::from_str::<Machine>(&turned_off).unwrap().nvapi_wrapper());
}

#[test]
fn an_nvidia_card_s_clock_is_held_for_a_viewer_unless_turned_off() {
    let nvidia = Machine { gpu_maker: Some(GpuMaker::Nvidia), ..sample() };
    let text = serde_json::to_string(&nvidia).unwrap();
    assert!(!text.contains("stream_clocks"), "an older build can't read this: {text}");
    assert!(serde_json::from_str::<Machine>(&text).unwrap().stream_clocks());
    let turned_off = serde_json::to_string(&Machine { stream_clocks_off: true, ..nvidia }).unwrap();
    assert!(!serde_json::from_str::<Machine>(&turned_off).unwrap().stream_clocks());
    assert!(!Machine { amd_gpu: "0000:03:00.0".into(), ..sample() }.stream_clocks());
}

#[test]
fn direct_frames_are_on_whatever_an_old_file_says_and_off_only_when_turned_off() {
    let text = serde_json::to_string(&sample()).unwrap();
    assert!(!text.contains("direct_frames"), "an older build reads this as off: {text}");
    let old_file = text.replacen('{', r#"{"direct_frames":false,"#, 1);
    assert!(serde_json::from_str::<Machine>(&old_file).unwrap().direct_frames());
    let turned_off = serde_json::to_string(&Machine { direct_frames_off: true, ..sample() }).unwrap();
    assert!(!serde_json::from_str::<Machine>(&turned_off).unwrap().direct_frames());
}

#[test]
fn the_bar_stays_unless_the_file_says_it_hides() {
    let text = serde_json::to_string(&sample()).unwrap();
    assert!(!text.contains("bar_stays"), "the default stays out: {text}");
    assert!(!serde_json::from_str::<Machine>(&text).unwrap().bar_hides());
    let older = r#"{"name": "Old", "disk": "/d", "vars": "/v"}"#;
    assert!(!serde_json::from_str::<Machine>(older).unwrap().bar_hides(), "nobody chose");
    let stayed = text.replacen('{', r#"{"bar_stays":true,"#, 1);
    assert!(!serde_json::from_str::<Machine>(&stayed).unwrap().bar_hides());
    let hides = serde_json::to_string(&Machine { bar_stays: false, ..sample() }).unwrap();
    assert!(hides.contains(r#""bar_stays":false"#), "an older build knows the field: {hides}");
    assert!(serde_json::from_str::<Machine>(&hides).unwrap().bar_hides());
}

#[test]
fn the_screen_sizes_to_the_window_unless_the_file_says_not_to() {
    let text = serde_json::to_string(&sample()).unwrap();
    assert!(!text.contains("window_size"), "an older build refuses a field it doesn't know: {text}");
    assert!(serde_json::from_str::<Machine>(&text).unwrap().sizes_to_window());
    let off = serde_json::to_string(&Machine { window_size_off: true, ..sample() }).unwrap();
    assert!(!serde_json::from_str::<Machine>(&off).unwrap().sizes_to_window());
    assert!(!Machine { kind: Kind::Plain, ..sample() }.sizes_to_window(), "no driver to ask");
}

#[test]
fn the_viewer_opens_in_a_window_unless_the_file_says_fullscreen() {
    let text = serde_json::to_string(&sample()).unwrap();
    assert!(!text.contains("opens_fullscreen"), "an older build refuses a field it doesn't know: {text}");
    assert!(!serde_json::from_str::<Machine>(&text).unwrap().opens_fullscreen);
    let on = serde_json::to_string(&Machine { opens_fullscreen: true, ..sample() }).unwrap();
    assert!(serde_json::from_str::<Machine>(&on).unwrap().opens_fullscreen);
}

#[test]
fn sound_is_on_unless_the_file_says_off() {
    assert!(!serde_json::to_string(&sample()).unwrap().contains("sound"), "the default stays out");
    let quiet = serde_json::to_string(&Machine { sound: false, ..sample() }).unwrap();
    assert!(!serde_json::from_str::<Machine>(&quiet).unwrap().sound);
    let older = r#"{"name": "Old", "disk": "/d", "vars": "/v"}"#;
    assert!(serde_json::from_str::<Machine>(older).unwrap().sound);
}

#[test]
fn what_is_saved_loads_the_same() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut machine = sample();
    machine.ram_gib = 24;
    machine.screen = "3840x2160".into();
    machine.refresh_hz = 240;
    machine.vcache_pin = true;
    machine.forwards = vec![Forward::new(Protocol::Tcp, 2222, 22)];
    machine.save(&context).unwrap();
    assert_eq!(load(&context, "sample").unwrap(), machine);
    assert!(!machine.folder(&context).join("machine.json.new").exists());
}

#[test]
fn a_save_from_an_older_copy_keeps_the_disk_where_a_start_put_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut older = sample();
    older.save(&context).unwrap();
    Machine { nvme: true, ..older.clone() }.save_disk_place(&context).unwrap();
    older.overlay = !older.overlay;
    older.save(&context).unwrap();
    let saved = load(&context, "sample").unwrap();
    assert!(saved.nvme, "the app's copy from before the move doesn't put the disk back on SATA");
    assert_eq!(saved.overlay, older.overlay, "what it changed is saved");
    Machine { nvme: false, ..saved }.save_disk_place(&context).unwrap();
    Machine { nvme: true, ..older }.save(&context).unwrap();
    assert!(!load(&context, "sample").unwrap().nvme, "nor on NVMe");
}

#[test]
fn a_new_machine_is_saved_with_its_disk_where_it_says() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    Machine { nvme: true, ..sample() }.save(&context).unwrap();
    assert!(load(&context, "sample").unwrap().nvme);
}

#[test]
fn bad_settings_are_refused_and_change_nothing() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let good = sample();
    good.save(&context).unwrap();
    type Change = fn(&mut Machine);
    let cases: [(Change, &str); 10] = [
        (|m| m.cores = 1, "Cores"),
        (|m| m.ram_gib = 3, "RAM"),
        (|m| m.screen = "1024x768".into(), "screen"),
        (|m| m.refresh_hz = MIN_REFRESH_HZ - 1, "refresh"),
        (|m| m.refresh_hz = MAX_REFRESH_HZ + 1, "refresh"),
        (|m| m.name = " ".into(), "name"),
        (|m| m.id = "../x".into(), "id"),
        (|m| m.disk = "json:{}".into(), "full path"),
        (|m| m.iso = "-x".into(), "full path"),
        (|m| m.share = "shared".into(), "full path"),
    ];
    for (change, text) in cases {
        let mut bad = good.clone();
        change(&mut bad);
        let error = bad.save(&context).unwrap_err();
        assert!(error.0.contains(text), "{error}");
    }
    assert_eq!(load(&context, "sample").unwrap(), good);
}

#[test]
fn a_size_only_the_driver_has_boots_at_one_the_basic_display_has() {
    let mut machine = sample();
    machine.screen = "2560x1440".into();
    assert_eq!(machine.basic_screen(), "1920x1080");
    machine.screen = "3840x2160".into();
    assert_eq!(machine.basic_screen(), "3840x2160");
}

#[test]
fn a_broken_file_is_reported_and_not_fatal() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut good = sample();
    good.refresh_hz = 240;
    good.save(&context).unwrap();
    let path = good.folder(&context).join("machine.json");
    let text = fs::read_to_string(&path).unwrap();
    for (broken, message) in [
        ("{".to_owned(), "Could not read"),
        (r#"{"name": 1}"#.to_owned(), "Could not read"),
        (text.replace("240", "1000"), "refresh"),
        (text.replace("name", "nom"), "Could not read"),
    ] {
        fs::write(&path, broken).unwrap();
        assert!(load(&context, "sample").unwrap_err().0.contains(message));
        let (machines, errors) = load_all(&context);
        assert!(machines.is_empty() && errors.len() == 1);
    }
    fs::write(&path, text).unwrap();
    assert_eq!(load_all(&context), (vec![good], vec![]));
}

#[test]
fn any_whole_rate_in_the_limits_is_kept_as_given() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut machine = sample();
    for hz in [MIN_REFRESH_HZ, 60, 75, 165, 175, 180, 240, MAX_REFRESH_HZ] {
        machine.refresh_hz = hz;
        machine.save(&context).unwrap();
        assert_eq!(load(&context, "sample").unwrap().refresh_hz, hz);
        assert!(machine.summary().contains(&format!("{hz} Hz")));
    }
}

#[test]
fn a_typed_rate_is_read_strictly() {
    assert_eq!(read_refresh(" 165 "), Ok(165));
    assert_eq!(read_refresh(&MAX_REFRESH_HZ.to_string()), Ok(MAX_REFRESH_HZ));
    let huge = "9".repeat(5000);
    for text in ["", " ", "abc", "16.5", "-144", "+144", "1e2", "0x90", "١٤٤", "144 Hz", "29", "361", &huge]
    {
        assert!(read_refresh(text).unwrap_err().0.contains("from 30 to 360 Hz"), "{text}");
    }
}

#[test]
fn a_machine_from_before_these_settings_stays_as_it_was() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write(
        "home/.local/share/guesthouse/machines/old/machine.json",
        r#"{"name":"Old","disk":"/d","vars":"/v","cores":8,"ram_gib":16,"screen":"1920x1080","refresh_hz":144,"vcache_pin":false}"#,
    );
    let old = load(&context, "old").unwrap();
    assert!(old.kind == Kind::Windows && old.has_gpu() && !old.tpm && !old.overlay && !old.clipboard);
    assert_eq!((old.network_mode, old.passt, old.mac.as_str()), (Mode::Nat, false, ""));
    assert_eq!((old.only_through.as_str(), old.dns.as_str()), ("", ""));
    assert_eq!(old.vram_gib, 0);
    assert!(old.forwards.is_empty());
}

#[test]
fn a_bridged_machine_needs_its_own_mac() {
    let mut machine = sample();
    machine.network_mode = Mode::Bridged;
    assert!(machine.problems(0).iter().any(|text| text.contains("own MAC")));
}

#[test]
fn the_account_is_the_scripts_name_unless_the_user_chose_one() {
    assert_eq!(sample().account(), "gpupv");
    assert_eq!(Machine { kind: Kind::Linux, ..sample() }.account(), "desk");
    assert_eq!(Machine { user: "ana".into(), ..sample() }.account(), "ana");
    assert!(!serde_json::to_string(&sample()).unwrap().contains("user"), "the default stays out");
    let named = serde_json::to_string(&Machine { user: "ana".into(), ..sample() }).unwrap();
    assert_eq!(serde_json::from_str::<Machine>(&named).unwrap().user, "ana");
    let problems = Machine { user: "ana/b".into(), ..sample() }.problems(32);
    assert!(problems.iter().any(|problem| problem.contains("user name")), "{problems:?}");
}

#[test]
fn each_kind_keeps_the_password_file_it_always_had() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    assert!(sample().password_file(&context).ends_with("sample/windows-password"));
    let ubuntu = Machine { kind: Kind::Linux, ..sample() };
    assert!(ubuntu.password_file(&context).ends_with("sample/seed/desk-password"));
}

#[test]
fn a_machines_token_is_in_its_own_folder() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    assert!(sample().token_file(&context).ends_with("machines/sample/guest.token"));
}

#[test]
fn ubuntu_says_its_gpu_is_a_preview_and_a_machine_can_go_without() {
    let mut machine = Machine { kind: Kind::Linux, ..sample() };
    assert!(machine.summary().starts_with("Ubuntu, ") && machine.summary().contains("GPU preview"));
    assert!(machine.wants_forwarded_gpu() && !machine.has_gpu(), "it has no daemon");
    machine.gpu_preview = false;
    assert!(machine.summary().contains("no GPU") && !machine.wants_forwarded_gpu());
}

#[test]
fn an_install_has_no_forwarded_gpu() {
    let machine = Machine { kind: Kind::Linux, installing: true, ..sample() };
    assert!(!machine.wants_forwarded_gpu());
}

#[test]
fn a_plain_machine_can_ask_for_the_linux_gpu_and_an_older_build_still_reads_one_that_does_not() {
    let plain = Machine { kind: Kind::Plain, ..sample() };
    assert!(!plain.own_linux_gpu());
    assert!(!serde_json::to_string(&plain).unwrap().contains("linux_gpu"), "the default stays out");
    let with_gpu = Machine { linux_gpu: true, ..plain };
    let read: Machine = serde_json::from_str(&serde_json::to_string(&with_gpu).unwrap()).unwrap();
    assert!(read.own_linux_gpu() && !read.wants_forwarded_gpu() && !read.has_gpu());
    assert!(read.summary().starts_with("Plain VM, ") && read.summary().contains("Linux GPU preview"));
}

#[test]
fn an_amd_card_is_only_a_linux_guests_gpu_and_an_older_build_still_reads_a_machine_with_none() {
    let ubuntu = Machine { kind: Kind::Linux, ..sample() };
    assert!(!serde_json::to_string(&ubuntu).unwrap().contains("amd_gpu"), "the default stays out");
    let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..ubuntu };
    let read: Machine = serde_json::from_str(&serde_json::to_string(&on_amd).unwrap()).unwrap();
    assert_eq!(read.amd_card(), Some("0000:7c:00.0"));
    assert_eq!(Machine { gpu_preview: false, ..read.clone() }.amd_card(), None);
    assert_eq!(Machine { kind: Kind::Windows, ..read.clone() }.amd_card(), None);
    let plain = Machine { kind: Kind::Plain, ..read };
    assert_eq!(plain.amd_card(), None, "a plain machine has no Linux GPU until it's turned on");
    assert_eq!(Machine { linux_gpu: true, ..plain }.amd_card(), Some("0000:7c:00.0"));
}

#[test]
fn only_a_plain_machine_takes_the_linux_gpu() {
    for kind in [Kind::Windows, Kind::Linux] {
        assert!(!Machine { kind, linux_gpu: true, ..sample() }.own_linux_gpu(), "{kind:?}");
    }
}

#[test]
fn plain_machines_say_they_have_no_gpu() {
    let mut machine = sample();
    assert!(machine.summary().starts_with("Windows with GPU-PV, "));
    machine.kind = Kind::Plain;
    assert!(machine.summary().starts_with("Plain VM, ") && machine.summary().contains("no GPU"));
}

fn drive(id: &str) -> Drive {
    Drive { id: id.to_owned() }
}

#[test]
fn drives_stay_out_of_the_file_when_there_are_none_and_an_older_file_has_none() {
    assert!(!serde_json::to_string(&sample()).unwrap().contains("drives"), "the default stays out");
    let older = r#"{"name": "Old", "disk": "/d", "vars": "/v"}"#;
    assert!(serde_json::from_str::<Machine>(older).unwrap().drives.is_empty());
    let drives = vec![drive("ata-A_1"), drive("ata-B_2")];
    let given = Machine { drives, ..sample() };
    let text = serde_json::to_string(&given).unwrap();
    assert!(text.contains(r#""drives":[{"id":"ata-A_1"},{"id":"ata-B_2"}]"#), "{text}");
    let read = serde_json::from_str::<Machine>(&text).unwrap();
    assert_eq!(read, Machine { id: String::new(), ..given });
}

#[test]
fn a_machine_has_three_drives_at_most_and_none_twice() {
    let three = Machine { drives: vec![drive("a"), drive("b"), drive("c")], ..sample() };
    assert!(three.problems(0).is_empty());
    let four = Machine { drives: vec![drive("a"), drive("b"), drive("c"), drive("d")], ..sample() };
    assert!(four.problems(0).iter().any(|text| text.contains("3 drives at most")));
    let twice = Machine { drives: vec![drive("a"), drive("a")], ..sample() };
    assert!(twice.problems(0).iter().any(|text| text.contains("a is given twice")));
}

#[test]
fn a_drive_cannot_be_named_to_reach_outside_the_folder_of_names() {
    for id in ["", ".", "..", "../sda", "by-id/x", "/dev/sda"] {
        let problems = Machine { drives: vec![drive(id)], ..sample() }.problems(0);
        assert!(problems.iter().any(|text| text.contains("/dev/disk/by-id")), "{id:?}");
    }
}

#[test]
fn ubuntu_has_no_drives_to_give() {
    let ubuntu = Machine { kind: Kind::Linux, drives: vec![drive("a")], ..sample() };
    assert!(ubuntu.problems(0).iter().any(|text| text.contains("Windows and plain machines")));
    assert!(Machine { kind: Kind::Plain, drives: vec![drive("a")], ..sample() }.problems(0).is_empty());
}

/// A machine.json as the Intel test build 0.4.1-intel.2 wrote it: it has `gpu_maker`, and none of
/// the fields that came to main after that build was cut.
const FROM_INTEL_TEST_BUILD: &str = r#"{
  "name": "Arc",
  "disk": "/home/tester/.local/share/guesthouse/machines/arc/disk.qcow2",
  "vars": "/home/tester/.local/share/guesthouse/machines/arc/OVMF_VARS.fd",
  "cores": 8,
  "ram_gib": 16,
  "screen": "1920x1080",
  "refresh_hz": 144,
  "hdr": false,
  "vcache_pin": false,
  "kind": "windows",
  "iso": "/home/tester/Downloads/Win11.iso",
  "installing": false,
  "clipboard": true,
  "share": "",
  "vram_gib": 0,
  "tpm": true,
  "overlay": false,
  "network_mode": "nat",
  "passt": true,
  "mac": "52:54:00:12:34:56",
  "bridge": "br0",
  "forwards": [],
  "gpu_preview": true,
  "amd_gpu": "0000:03:00.0",
  "gpu_maker": "intel"
}"#;

/// A machine.json as release 0.6.1 wrote it, on an AMD card with the window speed setting off and
/// a drive of the host's.
const FROM_RELEASE_0_6_1: &str = r#"{
  "name": "Radeon",
  "disk": "/home/tester/.local/share/guesthouse/machines/radeon/disk.qcow2",
  "vars": "/home/tester/.local/share/guesthouse/machines/radeon/OVMF_VARS.fd",
  "cores": 6,
  "ram_gib": 12,
  "screen": "2560x1440",
  "refresh_hz": 60,
  "hdr": false,
  "vcache_pin": false,
  "kind": "windows",
  "iso": "/home/tester/Downloads/Win11.iso",
  "installing": false,
  "clipboard": false,
  "share": "",
  "vram_gib": 0,
  "tpm": true,
  "overlay": false,
  "network_mode": "nat",
  "passt": false,
  "mac": "52:54:00:65:43:21",
  "bridge": "br0",
  "forwards": [{"protocol": "tcp", "host": 2222, "guest": 22}],
  "gpu_preview": true,
  "amd_gpu": "0000:7c:00.0",
  "user": "ana",
  "drives": [{"id": "ata-SOME_DISK_SERIAL"}],
  "window_speed_off": true
}"#;

fn load_text(text: &str, id: &str) -> Result<Machine> {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let folder = context.paths.machines.join(id);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join(MACHINE_FILE), text).unwrap();
    load(&context, id)
}

#[test]
fn a_machine_from_the_intel_test_build_loads_on_its_intel_card() {
    let machine = load_text(FROM_INTEL_TEST_BUILD, "arc").unwrap();
    assert_eq!(machine.windows_maker(), Some(GpuMaker::Intel));
    assert_eq!(machine.windows_card_address(), Some("0000:03:00.0"));
    assert!(machine.window_speed() && machine.disk_folder.is_empty() && machine.drives.is_empty());
    assert!(machine.only_through.is_empty() && machine.dns.is_empty());
}

#[test]
fn a_machine_from_release_0_6_1_loads_as_it_was() {
    let machine = load_text(FROM_RELEASE_0_6_1, "radeon").unwrap();
    assert_eq!(machine.windows_maker(), Some(GpuMaker::Amd));
    assert_eq!(machine.gpu_maker, None);
    assert!(!machine.window_speed());
    assert_eq!(machine.drives, [Drive { id: "ata-SOME_DISK_SERIAL".into() }]);
    assert_eq!(machine.forwards.len(), 1);
    assert!(machine.only_through.is_empty() && machine.dns.is_empty());
}

#[test]
fn only_through_and_its_dns_server_are_kept_and_left_out_when_empty() {
    let text = serde_json::to_string(&sample()).unwrap();
    assert!(!text.contains("only_through") && !text.contains("dns"), "{text}");
    let through = Machine { only_through: "wg0".into(), dns: "10.64.0.1".into(), ..sample() };
    let read: Machine = serde_json::from_str(&serde_json::to_string(&through).unwrap()).unwrap();
    assert_eq!((read.only_through.as_str(), read.dns.as_str()), ("wg0", "10.64.0.1"));
    let bad = Machine { only_through: "wg0,x".into(), dns: "nobody".into(), ..sample() };
    assert_eq!(bad.network_problems().len(), 2);
}

#[test]
fn a_task_takes_a_machine_by_its_id_or_its_folders_path_and_no_other_path() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    fs::create_dir_all(context.paths.machines.join("windowsarc")).unwrap();
    let folder = context.paths.machines.join("windowsarc");
    assert_eq!(id_from_argument(&context, "windowsarc").unwrap(), "windowsarc");
    assert_eq!(id_from_argument(&context, folder.to_str().unwrap()).unwrap(), "windowsarc");
    assert_eq!(id_from_argument(&context, &format!("{}/", folder.display())).unwrap(), "windowsarc");
    let elsewhere = id_from_argument(&context, "/tmp/windowsarc").unwrap_err();
    assert!(elsewhere.0.contains("folder name"), "{}", elsewhere.0);
}
#[test]
fn settings_chosen_before_a_machine_is_made_are_applied_to_it_and_nothing_it_was_made_with() {
    let seed = Machine::new("", "Draft", "", "");
    let chosen = Machine { passt: true, android_play: true, name: "Other".into(), ..seed.clone() };
    let changes = settings_changes(&seed, &chosen);
    let made = Machine { tpm: true, ..sample() };
    let applied = made.with_changes(&changes).unwrap();
    assert!(applied.passt && applied.android_play && applied.tpm, "chosen ones set, made ones kept");
    assert_eq!((applied.name.as_str(), applied.id.as_str()), (made.name.as_str(), made.id.as_str()));
    let back = settings_changes(&chosen, &seed);
    let undone = applied.with_changes(&back).unwrap();
    assert!(!undone.passt && !undone.android_play, "a setting taken back to its default goes back");
}
