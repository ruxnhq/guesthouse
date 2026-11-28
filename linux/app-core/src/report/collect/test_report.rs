//! What the dxdiag task gathered for an Intel machine's tester: Windows' own report, the API check's,
//! and the host's view of the card. Windows' two reports name the guest's account and computer, which
//! are taken out as the guest's logs take them out. The machine needn't be running for these.

use super::Collection;
use super::guest_logs::{GUEST_PC_MARK, GUEST_USER_MARK};
use crate::actions::{API_FILE, GATHERED_FOLDER, PROBE_FILE, REPORT_FILE};
use crate::context::Context;
use crate::gpu_maker::GpuMaker;
use crate::machine::Machine;
use crate::report::scrub::scrub_names;
use std::fs;

/// How dxdiag names the computer it ran on.
const COMPUTER_LINE: &str = "Machine name:";

pub fn collect(context: &Context, machine: &Machine, found: &mut Collection) {
    if machine.windows_maker() != Some(GpuMaker::Intel) {
        return;
    }
    let folder = machine.folder(context).join(GATHERED_FOLDER);
    let read =
        |name: &str| fs::read(folder.join(name)).ok().map(|data| String::from_utf8_lossy(&data).into_owned());
    let dxdiag = read(REPORT_FILE);
    let computer = dxdiag.as_deref().and_then(computer_name);
    let mut names = vec![(GUEST_USER_MARK, machine.account())];
    names.extend(computer.as_deref().map(|name| (GUEST_PC_MARK, name)));
    for name in [REPORT_FILE, API_FILE, PROBE_FILE] {
        let text = if name == REPORT_FILE { dxdiag.clone() } else { read(name) };
        if let Some(text) = text {
            found.add(&format!("machine/{GATHERED_FOLDER}/{name}"), scrub_names(&text, &names));
        }
    }
}

fn computer_name(dxdiag: &str) -> Option<String> {
    let line = dxdiag.lines().find_map(|line| line.trim().strip_prefix(COMPUTER_LINE))?;
    Some(line.trim().to_owned()).filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    fn on(maker: GpuMaker) -> Machine {
        let mut machine = Machine { user: "quillon".into(), ..Machine::new("arc", "Arc", "/d", "/v") };
        machine.use_windows_card(maker, "0000:03:00.0");
        machine
    }

    #[test]
    fn an_intel_testers_files_go_in_without_the_guests_names() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = on(GpuMaker::Intel);
        let folder = machine.folder(&context).join(GATHERED_FOLDER);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(REPORT_FILE), "  Machine name: DESKTOP-QUILL7\nUser Name: quillon\n").unwrap();
        fs::write(folder.join(API_FILE), "Windows 10.0.26100, user quillon, on DESKTOP-QUILL7\n").unwrap();
        fs::write(folder.join(PROBE_FILE), "device 0xe20b\n").unwrap();
        let mut found = Collection::default();
        collect(&context, &machine, &mut found);
        let names: Vec<&str> = found.names().collect();
        assert_eq!(
            names,
            [
                "machine/test-report/dxdiag.txt",
                "machine/test-report/api-check.txt",
                "machine/test-report/intel-probe.txt"
            ]
        );
        for item in &found.items {
            assert!(!item.text.contains("quillon") && !item.text.contains("QUILL7"), "{}", item.text);
        }
        assert!(found.items[1].text.contains("user <guest-user>, on <guest-pc>"), "{}", found.items[1].text);
    }

    #[test]
    fn another_makers_machine_adds_nothing() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = on(GpuMaker::Amd);
        let folder = machine.folder(&context).join(GATHERED_FOLDER);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(REPORT_FILE), "dxdiag").unwrap();
        let mut found = Collection::default();
        collect(&context, &machine, &mut found);
        assert_eq!(found.names().count(), 0);
    }
}
