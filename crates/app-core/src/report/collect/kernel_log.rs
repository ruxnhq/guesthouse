//! The kernel's lines about GPUs and KVM, where this user may read them. Many systems keep the
//! kernel's log from users without root, and then the manifest says how to get it by hand.

use super::Collection;
use super::command::output_of;
use regex::Regex;

const NAME: &str = "host/kernel-log.txt";
/// The drivers and subsystems a GPU guest goes through.
const WANTED: &str = r"(?i)nvidia|nvrm|amdgpu|drm|kvm|vfio|iommu|gpu|udmabuf|landlock";
/// The newest lines are the ones about the failure.
const MOST_LINES: usize = 3000;
const SECONDS: u32 = 20;
/// The same, for a tester to run, as sudo can't be asked for here.
const BY_HAND: &str = "sudo dmesg | grep -iE 'nvidia|nvrm|amdgpu|drm|kvm|vfio|iommu|gpu|udmabuf|landlock'";

pub fn collect(found: &mut Collection) {
    // dmesg first, as journalctl puts the host's name on every line. journalctl is the way in for a
    // user in the systemd-journal group where dmesg is kept from users.
    let read = output_of("dmesg", &[], SECONDS).or_else(|dmesg| {
        output_of(
            "journalctl",
            &["-k", "-b", "--no-hostname", "--no-pager", "-o", "short-monotonic"],
            SECONDS,
        )
        .map_err(|journal| format!("{dmesg}; {journal}"))
    });
    match read {
        Ok(text) => {
            let lines = wanted_lines(&text);
            if lines.is_empty() {
                // journalctl says it has nothing, rather than failing, to a user who may not read it.
                found.miss(
                    NAME,
                    "the kernel's log had no lines about GPUs or KVM that this user could read",
                    Some(BY_HAND),
                );
            } else {
                found.add(NAME, lines);
            }
        }
        Err(reason) => {
            found.miss(NAME, &format!("this user may not read the kernel's log ({reason})"), Some(BY_HAND))
        }
    }
}

fn wanted_lines(text: &str) -> String {
    let Ok(wanted) = Regex::new(WANTED) else { return String::new() };
    let lines: Vec<&str> = text.lines().filter(|line| wanted.is_match(line)).collect();
    let newest = &lines[lines.len().saturating_sub(MOST_LINES)..];
    newest.iter().map(|line| format!("{line}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_lines_about_gpus_and_kvm_are_kept() {
        let log = "[1.0] usb 1-1: new device\n[2.0] NVRM: loading NVIDIA UNIX Open Kernel Module\n\
                   [3.0] amdgpu 0000:03:00.0: ring gfx timeout\n[4.0] EXT4-fs mounted\n[5.0] kvm: enabled\n";
        assert_eq!(
            wanted_lines(log),
            "[2.0] NVRM: loading NVIDIA UNIX Open Kernel Module\n[3.0] amdgpu 0000:03:00.0: ring gfx timeout\n[5.0] kvm: enabled\n"
        );
    }
}
