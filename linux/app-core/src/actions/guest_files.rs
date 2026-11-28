//! Puts the driver's files in an existing guest's system folders. The driver's own folder reaches the
//! guest at install, but a program like a benchmark's launcher looks in System32 by name. The same for
//! NVIDIA's settings store, which the driver's installer would make and a game reads its profile from,
//! and for its Media Foundation transforms, which the driver's INF would register.

use super::Task;
use super::desktop::{GUEST_FOLDER, put_guest_file};
use super::driver_store::HOST_DRIVER_STORE;
use super::guest_upload::{put_big_file, quoted};
use crate::agent::guest_script;
use crate::failure::Result;
use crate::gpu_identity::{INTEL_RENDER, Shown, read_state};
use crate::log::write_log;
use crate::setup::amd_system_files::runtime_file_list;
use crate::setup::cards::nvidia_card_at;
use crate::setup::guest_driver::guest_driver;
use crate::setup::guest_driver::{INTEL_STORE_FOLDER, NVIDIA_STORE_FOLDER};
use crate::setup::guest_files::{guest_file_list, setup_card_device};
use crate::setup::intel::intel_driver_folder;
use crate::setup::intel_system_list::read_system_list;
use crate::text::base64_encode;
use guesthouse_backend_intel::protocol::record::{DEVICE_KEY, DEVICE_KEY_VALUES};
use sha2::{Digest, Sha256};
use std::fs;

const SCRIPT_NAME: &str = "nv-files.ps1";
const PROFILES_SCRIPT: &str = "nv-profiles.ps1";
const PROFILES_SECONDS: u64 = 30;
const TRANSFORMS_SCRIPT: &str = "nv-media-foundation.ps1";
const TRANSFORMS_SECONDS: u64 = 30;
/// How the transforms script's answer starts when it wrote nothing: the registry had it all, or the
/// driver's INF asks for none for this card.
const TRANSFORMS_UNCHANGED: [&str; 2] = ["kept", "nothing to register"];
const DISPLAY_NAME_SCRIPT: &str = "display-name.ps1";
const DISPLAY_NAME_SECONDS: u64 = 20;
const SETMODE_SCRIPT: &str = "setmode.ps1";
const SCRIPT_PLACED: &str = "script placed";
const TAKE_OVER_SCRIPT: &str = "take-over-display.ps1";
/// Where the display driver's install leaves its take-over script.
const TAKE_OVER_IN_GUEST: &str = "C:\\gpupv\\idd\\take-over-display.ps1";
/// The name the display driver's INF gives the device.
const INF_DISPLAY_NAME: &str = "RDPUDD Chained DD";
/// The name the guest's scripts also know it by, see display-name.ps1.
const OWN_DISPLAY_NAME: &str = "Guesthouse Display";
const LIST_NAME: &str = "nv-files.txt";
const LOG_NAME: &str = "nv-files.log";
const CHECK_SECONDS: u64 = 20;
const PLACE_SECONDS: u64 = 110;
/// How the placing script's check, and the count without it, answer: the files a run would copy.
const TO_PLACE_MARK: &str = "to place ";
const FAILED_MARK: &str = "FAILED";
const KEY_SECONDS: u64 = 20;
const KEY_MARK: &str = "device key set";
/// Where the display devices' software keys are, each named by its device's driver property.
const DISPLAY_CLASS_KEYS: &str = r"HKLM:\SYSTEM\CurrentControlSet\Control\Class\";
/// What the Level Zero loader reads from a display device's software key, and Intel's driver it names.
const LEVEL_ZERO_VALUE: &str = "LevelZeroDriverPath";
const LEVEL_ZERO_DRIVER: &str = "ze_intel_gpu64.dll";
/// Where Windows keeps what a 32-bit program sees as SOFTWARE.
/// Guesthouse's NVAPI wrapper, from the program's guest folder. It goes in System32 under NVIDIA's
/// name, in place of NVIDIA's copy, and loads NVIDIA's from the driver's folder. Without it NVIDIA's
/// NVAPI refuses the call DLSS needs on a paravirtualized card.
const NVAPI_WRAPPER: &str = "nvapi64.dll";
const NVAPI_FOLDER: &str = "System32";
const NVAPI_SECONDS: u64 = 30;
const NVAPI_KEPT: &str = "nvapi kept";
const NVAPI_NEEDED: &str = "nvapi needed";
const NVAPI_PLACED: &str = "nvapi placed";
const NVAPI_RESTORED: &str = "nvapi restored";
const NVAPI_NO_SOURCE: &str = "nvapi no source";
const SOFTWARE_32: (&str, &str) = (r"SOFTWARE\", r"SOFTWARE\WOW6432Node\");
const NO_SCRIPT: &str = "Windows lacks some of the driver's files, so a program that looks for them may crash. \
                         This install has no script to place them.";

fn guest_path(name: &str) -> String {
    format!("{GUEST_FOLDER}\\{name}")
}

fn send_list(task: &Task, list: &str) {
    let data = base64_encode(list.as_bytes());
    let path = guest_path(LIST_NAME);
    task.ask_guest(
        &format!("[IO.File]::WriteAllBytes('{path}', [Convert]::FromBase64String('{data}'))"),
        CHECK_SECONDS,
    );
}

/// The guest counts for itself, as only it knows what is in its folders. The placing script says
/// how many files it would copy, by the same comparison it copies by, so a file that is there but
/// older than the driver's counts as well as one that is missing. It goes as the request itself, so a
/// start with nothing to place sends no file. `None` when the guest didn't say.
fn count_to_place(task: &Task, script: &str, store: Option<&str>) -> Option<u32> {
    let request = format!("& {{\n{script}\n}} {} -Check", script_arguments(store));
    let said = task.ask_guest(&request, CHECK_SECONDS);
    said.split(TO_PLACE_MARK).nth(1)?.trim().parse().ok()
}

/// Without the script, which only an incomplete program folder lacks, the guest can still say which
/// files are missing.
fn count_missing(task: &Task) -> Option<u32> {
    let list = guest_path(LIST_NAME);
    let script = format!(
        "$count = 0; foreach ($line in Get-Content -LiteralPath '{list}') {{ \
         $folder, $name, $target = $line -split \"`t\"; \
         if (-not (Test-Path -LiteralPath (Join-Path (Join-Path $env:windir $folder) $target))) {{ $count++ }} }}; \
         \"{TO_PLACE_MARK}$count\""
    );
    let said = task.ask_guest(&script, CHECK_SECONDS);
    said.split(TO_PLACE_MARK).nth(1)?.trim().parse().ok()
}

/// The list, and the driver's folder in the guest's driver store, where NVIDIA's is the script's own.
fn script_arguments(store: Option<&str>) -> String {
    let store = store.map(|name| format!(" -StoreFolder {name}")).unwrap_or_default();
    format!("{}{store}", guest_path(LIST_NAME))
}

fn run_placing_script(task: &Task, store: Option<&str>) -> String {
    let (script, log) = (guest_path(SCRIPT_NAME), guest_path(LOG_NAME));
    task.ask_guest(&format!("& {script} {} | Tee-Object {log}", script_arguments(store)), PLACE_SECONDS)
}

/// The folder NVIDIA's settings store lives in, with the driver's own profile database in it. A guest
/// that has it keeps it, as the files change once a program has used them. The same script sets the
/// driver's Vulkan and OpenGL present method by the machine's "Faster programs in a window", as that
/// is the store it writes.
pub(super) fn place_profile_store(task: &mut Task) -> Result<()> {
    let script = match guest_script(task.context, PROFILES_SCRIPT) {
        Ok(script) => script,
        Err(failure) => {
            write_log(task.context, &format!("profile store: {}", failure.0))?;
            return Ok(());
        }
    };
    let script = format!("$on = ${}\n{script}", task.machine.window_speed());
    let said = task.ask_guest(&script, PROFILES_SECONDS);
    if said.contains(FAILED_MARK) {
        task.error = "NVIDIA's settings store did not reach Windows. The log says why.".to_owned();
    }
    if !all_kept(&said) {
        write_log(task.context, &format!("profile store: {}", said.trim()))?;
    }
    Ok(())
}

/// The PCI device number of the NVIDIA card the machine runs on: the one it was given, or setup's.
fn nvidia_device(task: &Task) -> Option<u32> {
    match task.machine.windows_card_address() {
        Some(pci) => nvidia_card_at(task.context, pci).map(|card| card.device),
        None => setup_card_device(task.context),
    }
}

/// NVIDIA's Media Foundation transforms, which a program that asks Media Foundation for a hardware
/// encoder finds the card's through. The driver's INF registers them on a PC, and a guest never
/// installs it. The guest reads what to register from the INF in its driver folder, at every start, so
/// a driver changed since is followed. The INF gives each card the transforms it has, and Windows
/// sees the device under the number Guesthouse gave it, so the script is told the card's own. A
/// guest without them is as it was before this, so what goes wrong is logged and the start says
/// nothing.
pub(super) fn register_media_transforms(task: &mut Task) -> Result<()> {
    let Some(device) = nvidia_device(task) else {
        write_log(task.context, "media transforms: the machine's card isn't known, so none are registered")?;
        return Ok(());
    };
    let script = match guest_script(task.context, TRANSFORMS_SCRIPT) {
        Ok(script) => script,
        Err(failure) => {
            write_log(task.context, &format!("media transforms: {}", failure.0))?;
            return Ok(());
        }
    };
    let said = task.ask_guest(&format!("$card = '{device:04X}'\n{script}"), TRANSFORMS_SECONDS);
    if !TRANSFORMS_UNCHANGED.iter().any(|start| said.trim().starts_with(start)) {
        write_log(task.context, &format!("media transforms: {}", said.trim()))?;
    }
    Ok(())
}

/// The name our display gets on an NVIDIA machine, by the vendor number Windows sees on the render
/// device: on Microsoft's, NVIDIA's OpenGL and Vulkan driver finds its GPU only through a display
/// named like a remote session's, which the INF names it; on NVIDIA's, that name folds the GPU into
/// the display's adapter, and the other one lets the driver keep them apart.
fn display_name(shown: Shown) -> &'static str {
    match shown {
        Shown::Microsoft => INF_DISPLAY_NAME,
        Shown::Maker => OWN_DISPLAY_NAME,
    }
}

/// Windows reads the name at boot, so what this writes counts from the next start. The start that
/// moves a machine to NVIDIA's number restarts Windows anyway, so that boot has it already.
pub(super) fn name_display(task: &mut Task) -> Result<()> {
    if give_display_its_name(task)? {
        task.error =
            "Windows could not be given the display's name for NVIDIA's driver. The log says why.".to_owned();
    }
    Ok(())
}

/// Names the display for the vendor number the machine's next start shows, and says whether that
/// failed. Also for a start whose GPU failed, which moves the machine back to Microsoft's number:
/// with NVIDIA's name left on it, OpenGL and Vulkan would find no GPU on the start after.
pub(super) fn give_display_its_name(task: &Task) -> Result<bool> {
    let script = match guest_script(task.context, DISPLAY_NAME_SCRIPT) {
        Ok(script) => script,
        Err(failure) => {
            write_log(task.context, &format!("display name: {}", failure.0))?;
            return Ok(false);
        }
    };
    let name = display_name(read_state(task.context, &task.machine).shown);
    // The logon task's script and the display driver's take-over script find our display by its
    // name too, and a guest keeps the copies it was installed with, which know only the INF's.
    if name != INF_DISPLAY_NAME {
        let setmode = task.context.paths.guest_files().join(SETMODE_SCRIPT);
        refresh_guest_script(task, &setmode, &guest_path(SETMODE_SCRIPT))?;
        let take_over = task.context.paths.display_driver().join(TAKE_OVER_SCRIPT);
        if take_over.is_file() {
            refresh_guest_script(task, &take_over, TAKE_OVER_IN_GUEST)?;
        }
    }
    let said = task.ask_guest(&format!("$name = '{name}'\n{script}"), DISPLAY_NAME_SECONDS);
    if !all_kept(&said) {
        write_log(task.context, &format!("display name: {}", said.trim()))?;
    }
    Ok(said.contains(FAILED_MARK))
}

/// Writes a guest's copy of one of our scripts again, where the guest has one. The install copied
/// some from read-only media, and those kept the read-only mark.
fn refresh_guest_script(task: &Task, local: &std::path::Path, path: &str) -> Result<()> {
    let data = base64_encode(&fs::read(local)?);
    let said = task.ask_guest(
        &format!(
            "$file = '{path}'; if (Test-Path -LiteralPath $file) {{ (Get-Item -LiteralPath $file).IsReadOnly = $false; \
             [IO.File]::WriteAllBytes($file, [Convert]::FromBase64String('{data}')) }}; '{SCRIPT_PLACED}'"
        ),
        DISPLAY_NAME_SECONDS,
    );
    if !said.contains(SCRIPT_PLACED) {
        write_log(task.context, &format!("display name: {path} stayed as it was: {}", said.trim()))?;
    }
    Ok(())
}

/// Whether the script said something, and every line of it is about something left as it was.
fn all_kept(said: &str) -> bool {
    let mut lines = said.lines().map(str::trim).filter(|line| !line.is_empty()).peekable();
    lines.peek().is_some() && lines.all(|line| line.starts_with("kept"))
}

/// Windows is up by now, so anything that goes wrong here is said and doesn't stop the machine.
pub(super) fn place_driver_files(task: &mut Task) -> Result<()> {
    let list = match guest_file_list(task.context) {
        Ok(list) => list,
        Err(failure) => {
            write_log(task.context, &format!("driver files: {}", failure.0))?;
            return Ok(());
        }
    };
    // With the wrapper to put there, NVIDIA's copy would only replace it. Without it, or with it
    // turned off, NVIDIA's goes in as before, so the guest has NVAPI either way.
    let list =
        if task.machine.nvapi_wrapper() && task.context.paths.guest_files().join(NVAPI_WRAPPER).is_file() {
            without_nvapi(&list)
        } else {
            list
        };
    place_listed_files(task, &list, None)
}

/// The list without NVIDIA's System32 nvapi64.dll. The 32-bit one in SysWOW64 stays NVIDIA's, as DLSS
/// is 64-bit only.
fn without_nvapi(list: &str) -> String {
    let is_nvapi = |line: &str| {
        let mut fields = line.split('\t');
        let folder = fields.next().unwrap_or("");
        let target = fields.nth(1).unwrap_or("").trim();
        folder.eq_ignore_ascii_case(NVAPI_FOLDER) && target.eq_ignore_ascii_case(NVAPI_WRAPPER)
    };
    list.lines().filter(|line| !is_nvapi(line)).map(|line| format!("{line}\n")).collect()
}

fn nvapi_check_script(hash: &str) -> String {
    format!(
        "$t = Join-Path $env:windir '{NVAPI_FOLDER}\\{NVAPI_WRAPPER}'; \
         if ((Test-Path -LiteralPath $t) -and (Get-FileHash -LiteralPath $t -Algorithm SHA256).Hash -eq '{hash}') \
         {{ '{NVAPI_KEPT}' }} else {{ '{NVAPI_NEEDED}' }}"
    )
}

/// Moves whatever is there aside first, as a program that has it loaded keeps Windows from writing
/// over it but not from renaming it. Copies moved aside earlier go once nothing has them loaded.
fn nvapi_place_script(staged: &str) -> String {
    format!(
        "$t = Join-Path $env:windir '{NVAPI_FOLDER}\\{NVAPI_WRAPPER}'; \
         try {{ Get-ChildItem -LiteralPath (Split-Path $t) -Filter '{NVAPI_WRAPPER}.*.old' -ErrorAction SilentlyContinue | \
         Remove-Item -Force -ErrorAction SilentlyContinue; \
         if (Test-Path -LiteralPath $t) {{ Move-Item -LiteralPath $t -Destination \"$t.$([DateTime]::UtcNow.Ticks).old\" -Force -ErrorAction Stop }}; \
         Copy-Item -LiteralPath {} -Destination $t -Force -ErrorAction Stop; '{NVAPI_PLACED}' }} \
         catch {{ \"{FAILED_MARK} nvapi: $($_.Exception.Message)\" }}",
        quoted(staged)
    )
}

/// Puts NVIDIA's NVAPI back in System32 when what is there isn't it, as the wrapper is when the
/// user has just turned it off. Moved aside first, as in the place script.
fn nvapi_restore_script() -> String {
    format!(
        "$t = Join-Path $env:windir '{NVAPI_FOLDER}\\{NVAPI_WRAPPER}'; \
         $s = '{HOST_DRIVER_STORE}\\{NVIDIA_STORE_FOLDER}\\{NVAPI_WRAPPER}'; \
         if (-not (Test-Path -LiteralPath $s)) {{ '{NVAPI_NO_SOURCE}' }} \
         elseif ((Test-Path -LiteralPath $t) -and (Get-FileHash -LiteralPath $t).Hash -eq (Get-FileHash -LiteralPath $s).Hash) {{ '{NVAPI_KEPT}' }} \
         else {{ try {{ Get-ChildItem -LiteralPath (Split-Path $t) -Filter '{NVAPI_WRAPPER}.*.old' -ErrorAction SilentlyContinue | \
         Remove-Item -Force -ErrorAction SilentlyContinue; \
         if (Test-Path -LiteralPath $t) {{ Move-Item -LiteralPath $t -Destination \"$t.$([DateTime]::UtcNow.Ticks).old\" -Force -ErrorAction Stop }}; \
         Copy-Item -LiteralPath $s -Destination $t -Force -ErrorAction Stop; '{NVAPI_RESTORED}' }} \
         catch {{ \"{FAILED_MARK} nvapi: $($_.Exception.Message)\" }} }}"
    )
}

/// With the wrapper turned off, NVIDIA's NVAPI goes back where the wrapper was.
fn restore_nvidias_nvapi(task: &mut Task) -> Result<()> {
    let said = task.ask_guest(&nvapi_restore_script(), NVAPI_SECONDS);
    if said.contains(NVAPI_KEPT) {
        return Ok(());
    }
    write_log(task.context, &format!("nvapi wrapper off: {}", said.trim()))?;
    if !said.contains(NVAPI_RESTORED) && !said.contains(NVAPI_NO_SOURCE) {
        task.error =
            "NVIDIA's NVAPI did not go back in System32, so the NVAPI wrapper may still be there.".to_owned();
    }
    Ok(())
}

/// Puts the NVAPI wrapper in System32, unless the guest has this program's copy already or the user
/// turned it off, when NVIDIA's goes back instead.
pub(super) fn place_nvapi_wrapper(task: &mut Task) -> Result<()> {
    if !task.machine.nvapi_wrapper() {
        return restore_nvidias_nvapi(task);
    }
    let local = task.context.paths.guest_files().join(NVAPI_WRAPPER);
    let Ok(bytes) = fs::read(&local) else {
        write_log(task.context, "nvapi wrapper: this program folder has none, so NVIDIA's stays")?;
        return Ok(());
    };
    let hash: String = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02X}")).collect();
    let said = task.ask_guest(&nvapi_check_script(&hash), NVAPI_SECONDS);
    if said.contains(NVAPI_KEPT) {
        return Ok(());
    }
    if !said.contains(NVAPI_NEEDED) {
        write_log(
            task.context,
            &format!("nvapi wrapper: the guest didn't say what it has: {}", said.trim()),
        )?;
        return Ok(());
    }
    task.step("Placing the NVAPI wrapper");
    let staged = guest_path(NVAPI_WRAPPER);
    if !put_big_file(task, &local, &staged)? {
        write_log(task.context, "nvapi wrapper: it didn't reach the guest")?;
        task.error = "The NVAPI wrapper did not reach Windows, so DLSS may not find the card.".to_owned();
        return Ok(());
    }
    let said = task.ask_guest(&nvapi_place_script(&staged), NVAPI_SECONDS);
    write_log(task.context, &format!("nvapi wrapper: {}", said.trim()))?;
    if !said.contains(NVAPI_PLACED) {
        task.error = "The NVAPI wrapper did not reach System32, so DLSS may not find the card.".to_owned();
    }
    Ok(())
}

/// What AMD's INF puts in System32 and SysWOW64 for the media runtime, which programs load by name:
/// ffmpeg's AMF encoders, RustDesk and OBS. It goes in whether or not the video switch is on, as it
/// does on a PC with the driver installed. The list is read from the driver's INF, so a newer driver
/// is followed. A guest that has the files is left alone.
pub(super) fn place_amd_runtime(task: &mut Task) -> Result<()> {
    let Ok(driver) = guest_driver(task.context, &task.machine) else { return Ok(()) };
    match runtime_file_list(&driver.folder) {
        Ok(list) => place_listed_files(task, &list, Some(driver.store_folder)),
        Err(why) => {
            write_log(task.context, &format!("driver files: AMD's media runtime: {why}"))?;
            Ok(())
        }
    }
}

/// What Intel's INF puts in the system folders, from the list unpacked with its driver: the loaders
/// the guest's programs find Intel's OpenCL, Vulkan and Level Zero drivers through, and the C++
/// runtime they import. A guest that has them all is left alone.
pub(super) fn place_system_files(task: &mut Task) -> Result<()> {
    let list = intel_driver_folder(task.context).and_then(|folder| read_system_list(&folder));
    match list.filter(|list| !list.trim().is_empty()) {
        Some(list) => place_listed_files(task, &list, Some(INTEL_STORE_FOLDER)),
        None => {
            write_log(task.context, "driver files: Intel's driver has no list of system files")?;
            Ok(())
        }
    }
}

/// The registry key the Intel adapter's record names as the device's, with the values Intel's OpenCL
/// and Level Zero drivers look for in it. Without them they offer no device. Windows shows a 32-bit
/// program another SOFTWARE key, so the key goes in both.
///
/// The Level Zero loader finds its driver another way: it reads LevelZeroDriverPath from the software
/// key of each display device present, which Intel's INF would have written. In the guest that is the
/// key Windows made for the render device with Intel's identity, so the path goes there too.
fn device_key_script() -> String {
    let key_script = |path: &str| {
        let key = quoted(&format!(r"HKLM:\{path}"));
        let values: String = DEVICE_KEY_VALUES
            .iter()
            .map(|(name, value)| {
                format!(
                    "New-ItemProperty -Path {key} -Name {} -PropertyType String -Value {} -Force -ea Stop | Out-Null; ",
                    quoted(name),
                    quoted(value)
                )
            })
            .collect();
        format!("New-Item -Path {key} -Force -ea Stop | Out-Null; {values}")
    };
    let keys = key_script(DEVICE_KEY) + &key_script(&DEVICE_KEY.replacen(SOFTWARE_32.0, SOFTWARE_32.1, 1));
    let level_zero = format!(
        "Get-PnpDevice -PresentOnly -Class Display -ea Stop | Where-Object {{ $_.HardwareID -like {} }} | \
         ForEach-Object {{ $key = {} + (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName DEVPKEY_Device_Driver -ea Stop).Data; \
         New-ItemProperty -Path $key -Name {} -PropertyType String -Value {} -Force -ea Stop | Out-Null }}; ",
        quoted(&INTEL_RENDER.windows_pattern()),
        quoted(DISPLAY_CLASS_KEYS),
        quoted(LEVEL_ZERO_VALUE),
        quoted(&format!(r"{HOST_DRIVER_STORE}\{INTEL_STORE_FOLDER}\{LEVEL_ZERO_DRIVER}")),
    );
    format!("try {{ {keys}{level_zero}'{KEY_MARK}' }} catch {{ \"$_\" }}")
}

/// Windows is up by now, so a key that doesn't take is said and doesn't stop the machine.
pub(super) fn write_device_key(task: &mut Task) -> Result<()> {
    let said = task.ask_guest(&device_key_script(), KEY_SECONDS);
    if !said.contains(KEY_MARK) {
        write_log(task.context, &format!("device key: {}", said.trim()))?;
        task.error =
            "Intel's compute drivers will find no GPU, as their registry key did not reach Windows. \
                      The log says why."
                .to_owned();
    }
    Ok(())
}

fn place_listed_files(task: &mut Task, list: &str, store: Option<&str>) -> Result<()> {
    task.step("Checking the driver's files");
    send_list(task, list);
    let Ok(script) = guest_script(task.context, SCRIPT_NAME) else {
        if count_missing(task).is_some_and(|count| count > 0) {
            write_log(task.context, "driver files: this program folder has no script to place them")?;
            task.error = NO_SCRIPT.to_owned();
        }
        return Ok(());
    };
    if count_to_place(task, &script, store).is_none_or(|count| count == 0) {
        return Ok(());
    }
    task.step("Placing the driver's files");
    put_guest_file(task, SCRIPT_NAME, &guest_path(SCRIPT_NAME))?;
    let said = run_placing_script(task, store);
    write_log(task.context, &format!("driver files: {}", said.trim()))?;
    if said.contains(FAILED_MARK) {
        task.error = "Some of the driver's files did not reach Windows. The log says which.".to_owned();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_script_that_changed_nothing_is_left_out_of_the_log() {
        assert!(all_kept("kept C:\\ProgramData\\NVIDIA Corporation\\Drs\r\nkept the present method\r\n"));
        assert!(!all_kept("kept C:\\ProgramData\\NVIDIA Corporation\\Drs\r\npresent method set to DXGI\r\n"));
        assert!(!all_kept("made C:\\ProgramData\\NVIDIA Corporation\\Drs\r\nkept the present method\r\n"));
        assert!(!all_kept("FAILED present method: NVAPI has no function\r\n"));
        assert!(!all_kept(""), "no answer is not an answer");
    }

    #[test]
    fn nvidias_system32_nvapi_is_left_out_and_the_32_bit_one_stays() {
        let list = "System32\tnvapi64.dll\tnvapi64.dll\nSysWOW64\tnvapi.dll\tnvapi.dll\nSystem32\tnvml.dll\tnvml.dll\n";
        assert_eq!(without_nvapi(list), "SysWOW64\tnvapi.dll\tnvapi.dll\nSystem32\tnvml.dll\tnvml.dll\n");
    }

    #[test]
    fn the_restore_script_takes_nvidias_from_the_driver_store_and_moves_the_wrapper_aside() {
        let script = nvapi_restore_script();
        let (moved, copied) = (script.find("Move-Item").unwrap(), script.find("Copy-Item").unwrap());
        assert!(moved < copied);
        assert!(
            script.contains(r"HostDriverStore\FileRepository\nv_dispi.inf_amd64_gpupv\nvapi64.dll'"),
            "{script}"
        );
        assert!(script.contains(r"'System32\nvapi64.dll'"));
    }

    #[test]
    fn the_place_script_moves_the_old_copy_aside_before_copying() {
        let script = nvapi_place_script(r"C:\gpupv\nvapi64.dll");
        let (moved, copied) = (script.find("Move-Item").unwrap(), script.find("Copy-Item").unwrap());
        assert!(moved < copied);
        assert!(script.contains(r"'System32\nvapi64.dll'"));
        assert!(script.contains(r"-LiteralPath 'C:\gpupv\nvapi64.dll'"));
    }

    #[test]
    fn the_device_key_script_makes_the_key_and_writes_each_value_as_text() {
        let script = device_key_script();
        assert!(script.contains(r"New-Item -Path 'HKLM:\SOFTWARE\Guesthouse\Intel adapter' -Force"));
        assert!(
            script.contains(r"New-Item -Path 'HKLM:\SOFTWARE\WOW6432Node\Guesthouse\Intel adapter' -Force")
        );
        assert!(script.contains("-Name 'OpenCLDriverName' -PropertyType String -Value 'igdrcl64.dll'"));
        assert!(script.contains(&format!("'{KEY_MARK}' }}")), "the mark comes only after the writes");
        assert!(script.contains(r"-like 'PCI\VEN_8086&DEV_CFFE*'"), "{script}");
        assert!(script.contains(
            r"-Name 'LevelZeroDriverPath' -PropertyType String -Value 'C:\Windows\System32\HostDriverStore\FileRepository\iigd_dch.inf_amd64_gpupv\ze_intel_gpu64.dll'"
        ));
    }
}
