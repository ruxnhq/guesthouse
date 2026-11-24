//! `guesthouse-hcs-boot`: Guesthouse on a Windows host from the command line. It makes a Windows or
//! Ubuntu guest's disk from an ISO and boots it with GPU-PV through HCS, with only Virtual Machine
//! Platform on, so it runs on Windows 11 Home, and with no Hyper-V and none of its tools. The work
//! is `guesthouse-host-windows`, which the Guesthouse service uses too; this is its options and
//! its printing.
//!
//! The guest has no screen anyone can see yet, so it tells the host over its serial port what it
//! sees of its card, and the program prints that.

// Elsewhere it only says it runs on Windows, and builds so the workspace does, so the parts the
// Windows commands use go unused there.
#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
mod windows;

use guesthouse_host_windows::settings::{BootSettings, GpuChoice, PrepareSettings};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: guesthouse-hcs-boot COMMAND [OPTIONS]

  gpus                     Lists the cards that can be shared with a guest.
  document --disk VHDX     Prints the machine document boot would send, without sending it.
  prepare --image FILE --disk VHDX
                           Makes VHDX from a Windows ISO, or its install.wim or install.esd,
                           and puts the card's driver on it. Needs an elevated prompt.
  boot --disk VHDX         Copies the card's driver to the disk again if it changed, boots the
                           machine, gives it the card, and prints what the guest says on COM1
                           until it stops. Ctrl+C asks the guest to shut down, and a second
                           Ctrl+C stops it at once. Needs an elevated prompt.

options:
  --name NAME              The machine's name in HCS (guesthouse-windows).
  --memory-mb N            Its memory (8192).
  --cpus N                 Its processors (4).
  --gpu TEXT               The card whose name or ID holds TEXT. Without it, the card with the most memory of its own.
  --no-gpu                 No card, and no driver copy.
  --no-network             No network card.
  --no-tpm                 No TPM.
  --no-nvapi-wrapper       boot: NVIDIA's own NVAPI in System32, not Guesthouse's wrapper.
  --for-minutes N          boot: asks the guest to shut down after N minutes, and stops it
                           three minutes later if it hasn't.
  --until-report           boot: asks the guest to shut down once it has said what it sees of its
                           card, as its report's last line. A first boot that's still setting
                           up runs on until then.
  --no-jail                Reads the guest's serial port in this process, not in the jailed
                           relay beside it.
  --size-gb N              prepare: the disk's largest size (128).
  --edition NAME           prepare: the edition in the image (Pro if it has it, else its first).
  --user NAME              prepare: the guest's administrator (guesthouse). Its password goes in a file
                           beside the disk.";

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub command: Command,
    pub disk: Option<PathBuf>,
    pub image: Option<PathBuf>,
    pub name: String,
    pub memory_mb: u64,
    pub processors: u32,
    pub gpu: Option<String>,
    pub no_gpu: bool,
    pub network: bool,
    pub tpm: bool,
    pub nvapi_wrapper: bool,
    pub jail: bool,
    pub for_minutes: Option<u64>,
    pub until_report: bool,
    pub size_gb: u64,
    pub edition: String,
    pub user: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Gpus,
    Document,
    Prepare,
    Boot,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            command: Command::Gpus,
            disk: None,
            image: None,
            name: "guesthouse-windows".into(),
            memory_mb: 8192,
            processors: 4,
            gpu: None,
            no_gpu: false,
            network: true,
            tpm: true,
            nvapi_wrapper: true,
            jail: true,
            for_minutes: None,
            until_report: false,
            size_gb: 128,
            edition: String::new(),
            user: "guesthouse".into(),
        }
    }
}

impl Options {
    fn gpu_choice(&self) -> GpuChoice {
        match (&self.gpu, self.no_gpu) {
            (_, true) => GpuChoice::None,
            (Some(text), false) => GpuChoice::Matching(text.clone()),
            (None, false) => GpuChoice::Default,
        }
    }

    fn prepare_settings(&self) -> PrepareSettings {
        PrepareSettings {
            disk: self.disk.clone().unwrap_or_default(),
            image: self.image.clone().unwrap_or_default(),
            user: self.user.clone(),
            edition: self.edition.clone(),
            size_gb: self.size_gb,
            gpu: self.gpu_choice(),
        }
    }

    fn boot_settings(&self) -> BootSettings {
        BootSettings {
            name: self.name.clone(),
            disk: self.disk.clone().unwrap_or_default(),
            memory_mb: self.memory_mb,
            processors: self.processors,
            tpm: self.tpm,
            // Nesting is a setting of the app's, through the service.
            nested_virtualization: false,
            network: self.network,
            jail: self.jail,
            gpu: self.gpu_choice(),
            for_minutes: self.for_minutes,
            until_report: self.until_report,
            // The console is the service's, for the app; from the command line there's the serial
            // port and the report.
            console_access: Vec::new(),
            nvapi_wrapper: self.nvapi_wrapper,
            // Sharing a folder, and bridging, are the app's, through the service.
            shares: Vec::new(),
            bridge: None,
            // The loopback's port for SSH is the service's, as it keeps each machine's.
            ssh_port: None,
        }
    }
}

fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut arguments = arguments.into_iter();
    let command = match arguments.next().as_deref() {
        Some("gpus") => Command::Gpus,
        Some("document") => Command::Document,
        Some("prepare") => Command::Prepare,
        Some("boot") => Command::Boot,
        Some(other) => return Err(format!("{other} is not a command")),
        None => return Err("no command".into()),
    };
    let mut options = Options { command, ..Options::default() };
    while let Some(flag) = arguments.next() {
        let mut value = || arguments.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--disk" => options.disk = Some(PathBuf::from(value()?)),
            "--image" => options.image = Some(PathBuf::from(value()?)),
            "--name" => options.name = value()?,
            "--memory-mb" => options.memory_mb = number(&flag, &value()?)?,
            "--cpus" => options.processors = number(&flag, &value()?)?,
            "--gpu" => options.gpu = Some(value()?),
            "--size-gb" => options.size_gb = number(&flag, &value()?)?,
            "--edition" => options.edition = value()?,
            "--user" => options.user = value()?,
            "--no-gpu" => options.no_gpu = true,
            "--no-network" => options.network = false,
            "--no-tpm" => options.tpm = false,
            "--no-nvapi-wrapper" => options.nvapi_wrapper = false,
            "--no-jail" => options.jail = false,
            "--for-minutes" => options.for_minutes = Some(number(&flag, &value()?)?),
            "--until-report" => options.until_report = true,
            _ => return Err(format!("{flag} is not an option")),
        }
    }
    let needs_disk = command != Command::Gpus;
    if needs_disk && options.disk.is_none() {
        return Err("--disk is needed".into());
    }
    if command == Command::Prepare && options.image.is_none() {
        return Err("--image is needed".into());
    }
    if options.name.is_empty()
        || !options.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("--name takes letters, digits, - and _".into());
    }
    Ok(options)
}

fn number<T: std::str::FromStr>(flag: &str, text: &str) -> Result<T, String> {
    text.parse().map_err(|_| format!("{flag} takes a number, not {text}"))
}

fn main() -> ExitCode {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(reason) => {
            eprintln!("{reason}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    run(&options)
}

#[cfg(windows)]
fn run(options: &Options) -> ExitCode {
    match windows::run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("{reason}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run(_options: &Options) -> ExitCode {
    eprintln!("guesthouse-hcs-boot runs on a Windows host only.");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Result<Options, String> {
        parse(text.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn boot_takes_its_disk_and_settings() {
        let options =
            parsed("boot --disk D:\\a.vhdx --memory-mb 16384 --cpus 8 --gpu 5090 --no-tpm").unwrap();
        assert_eq!(options.command, Command::Boot);
        assert_eq!(options.disk, Some(PathBuf::from("D:\\a.vhdx")));
        assert_eq!((options.memory_mb, options.processors), (16384, 8));
        assert_eq!(options.gpu.as_deref(), Some("5090"));
        assert!(!options.tpm && options.network);
    }

    #[test]
    fn options_become_the_librarys_settings() {
        let boot =
            parsed("boot --disk a.vhdx --gpu 5090 --until-report --for-minutes 30").unwrap().boot_settings();
        assert_eq!(boot.gpu, GpuChoice::Matching("5090".into()));
        assert_eq!((boot.until_report, boot.for_minutes), (true, Some(30)));
        let none = parsed("boot --disk a.vhdx --gpu 5090 --no-gpu").unwrap().boot_settings();
        assert_eq!(none.gpu, GpuChoice::None, "--no-gpu wins");
        let prepare = parsed("prepare --image u.iso --disk a.vhdx").unwrap().prepare_settings();
        assert_eq!((prepare.user.as_str(), prepare.gpu), ("guesthouse", GpuChoice::Default));
    }

    #[test]
    fn a_command_without_what_it_needs_is_refused() {
        assert!(parsed("boot").is_err());
        assert!(parsed("prepare --disk a.vhdx").is_err());
        assert!(parsed("boot --disk a.vhdx --cpus many").is_err());
        assert!(parsed("boot --disk a.vhdx --name a/b").is_err());
        assert!(parsed("start").is_err());
        assert!(parsed("gpus").is_ok());
    }
}
