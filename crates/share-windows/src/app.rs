//! The program: its command line, the connection to the host, and the drive.
//!
//! It is run as SYSTEM by a scheduled task, and a drive mounted from there is in the global
//! namespace, so every user of the guest sees it. It runs until it's killed or the connection to
//! the host drops, and then it unmounts and exits with 1, so the task that runs it starts it again.

use crate::ffi::{
    DISK_DEVICE_NAME, FLAG_CASE_PRESERVED_NAMES, FLAG_CASE_SENSITIVE_SEARCH, FLAG_READ_ONLY_VOLUME,
    FLAG_UNICODE_ON_DISK, VolumeParams,
};
use crate::fs::{self, Shared};
use crate::hv::{CONNECT_PATIENCE, HvStream};
use crate::logic::{self, Args, Command};
use crate::winfsp::{Api, wide};
use guesthouse_p9::{Client, dt, open};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::ptr::null_mut;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;

/// The message size asked for, as App Sandbox asks. The server may choose a smaller one.
const MSIZE: u32 = 64 * 1024;
/// What Hyper-V's Plan 9 server accepts in an attach, as App Sandbox's guest client sends it.
const UNAME: &str = "nobody";
const N_UNAME: u32 = 65534;
/// The volume's file system name, as Windows shows it.
const FILE_SYSTEM_NAME: &str = "guesthouse";
/// How long Windows may keep what it has been told of a file, in milliseconds.
const FILE_INFO_TIMEOUT: u32 = 1000;

/// Where messages go: standard output, and a file if there is one.
struct Log(Option<Mutex<File>>);

impl Log {
    fn open(path: Option<&Path>) -> Log {
        let file = path.and_then(|path| match OpenOptions::new().create(true).append(true).open(path) {
            Ok(file) => Some(Mutex::new(file)),
            Err(error) => {
                eprintln!("guesthouse-guest-share: cannot write the log {}: {error}", path.display());
                None
            }
        });
        Log(file)
    }

    fn line(&self, message: &str) {
        println!("{message}");
        if let Some(file) = &self.0 {
            let mut file = file.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = writeln!(file, "{message}");
        }
    }
}

pub fn run(args: impl IntoIterator<Item = String>) -> i32 {
    let args = match logic::parse_args(args) {
        Ok(Command::Help) => {
            print!("{}", logic::USAGE);
            return 0;
        }
        Ok(Command::Run(args)) => args,
        Err(message) => {
            eprintln!("guesthouse-guest-share: {message}\n\n{}", logic::USAGE);
            return 2;
        }
    };
    let log = Log::open(args.log.as_deref());
    match serve(&args, &log) {
        Ok(code) => code,
        Err(message) => {
            log.line(&format!("guesthouse-guest-share: {message}"));
            1
        }
    }
}

/// Connects to the host, agrees on 9P2000.L and attaches the share. It gives the client and the
/// attach fid, which is never opened.
fn connect(args: &Args, log: &Log) -> Result<(Client<HvStream>, u32), String> {
    let stream = HvStream::connect_patiently(args.port, CONNECT_PATIENCE, |error| {
        log.line(&format!("waiting for the host on port {}: {error}", args.port));
    })
    .map_err(|e| format!("cannot connect to the host on port {}: {e}", args.port))?;
    let mut client = Client::new(stream);
    let msize = client.version(MSIZE).map_err(|e| format!("9P version: {e}"))?;
    let root = client.new_fid();
    client
        .attach(root, &args.share, UNAME, N_UNAME)
        .map_err(|e| format!("cannot attach {:?}: {e}", args.share))?;
    log.line(&format!("attached {:?} on port {}, msize {msize}", args.share, args.port));
    Ok((client, root))
}

/// `--check`: lists the share's root.
fn check(mut client: Client<HvStream>, root: u32) -> Result<i32, String> {
    // The attach fid is never opened, so a copy of it is.
    let dir = client.new_fid();
    client.walk(root, dir, &[]).map_err(|e| format!("walk to the root: {e}"))?;
    client.lopen(dir, open::O_RDONLY | open::O_DIRECTORY).map_err(|e| format!("open the root: {e}"))?;
    let (mut offset, mut count) = (0, 0usize);
    loop {
        let entries =
            client.readdir(dir, offset, client.msize()).map_err(|e| format!("list the root: {e}"))?;
        let Some(last) = entries.last().map(|e| e.offset) else { break };
        if last == offset {
            break;
        }
        offset = last;
        for entry in entries {
            let kind = match entry.ty {
                dt::DIR => "dir",
                dt::LNK => "link",
                dt::REG => "file",
                _ => "other",
            };
            println!("{kind}\t{}", entry.name);
            count += 1;
        }
    }
    let _ = client.clunk(dir);
    println!("ok: {count} entries, msize {}", client.msize());
    Ok(0)
}

/// The security descriptor every file has, from its SDDL, as the bytes of a self-relative one.
fn security_descriptor(read_only: bool) -> Result<Vec<u8>, String> {
    let sddl = wide(logic::security_descriptor_sddl(read_only));
    let mut descriptor = null_mut();
    let mut length = 0u32;
    // SAFETY: the string is terminated and the outputs are places for the answer.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            &mut length,
        )
    };
    if converted == 0 {
        return Err("cannot build the security descriptor".to_owned());
    }
    // SAFETY: Windows allocated `length` bytes at `descriptor`, which are copied and then freed.
    let bytes = unsafe { std::slice::from_raw_parts(descriptor.cast::<u8>(), length as usize).to_vec() };
    // SAFETY: the buffer is Windows' LocalAlloc, as the call says to free it.
    unsafe { LocalFree(descriptor) };
    Ok(bytes)
}

fn volume_params(args: &Args) -> VolumeParams {
    let mut params = VolumeParams::zeroed();
    params.sector_size = logic::SECTOR_SIZE as u16;
    params.sectors_per_allocation_unit = 1;
    params.max_component_length = 255;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    params.volume_creation_time = logic::unix_to_filetime(now.as_secs(), u64::from(now.subsec_nanos()));
    params.volume_serial_number = args.port;
    params.file_info_timeout = FILE_INFO_TIMEOUT;
    // Names are case-sensitive on the host and the 9P share, and case is kept. Windows' ACLs aren't
    // kept: the same descriptor answers for every file.
    params.flags = FLAG_CASE_SENSITIVE_SEARCH | FLAG_CASE_PRESERVED_NAMES | FLAG_UNICODE_ON_DISK;
    if args.read_only {
        params.flags |= FLAG_READ_ONLY_VOLUME;
    }
    for (slot, unit) in params.file_system_name.iter_mut().zip(FILE_SYSTEM_NAME.encode_utf16()) {
        *slot = unit;
    }
    params
}

fn serve(args: &Args, log: &Log) -> Result<i32, String> {
    // WinFsp is looked for first, so a guest without it says so at once, not after the wait for
    // the host. `--check` doesn't need it.
    let api = if args.check { None } else { Some(Api::load()?) };
    let (client, root) = connect(args, log)?;
    let Some(api) = api else { return check(client, root) };

    // SAFETY: plain call.
    let used = unsafe { GetLogicalDrives() };
    // A drive asked for is the only one tried. Otherwise each free one from S: is, in turn, as
    // another folder's copy of this program may take one between the look and the mount.
    let drives = match &args.drive {
        Some(drive) => vec![drive.clone()],
        None => logic::free_drives(used),
    };
    if drives.is_empty() {
        return Err("every drive from S: to Z: is in use".into());
    }
    let security = security_descriptor(args.read_only)?;
    let shared = Shared::install(client, root, args.read_only, &args.share, security, api);
    let api = shared.api();
    let params = volume_params(args);

    // SAFETY: the callbacks in `fs::interface` are written for WinFsp's threads.
    let file_system = unsafe { api.create(DISK_DEVICE_NAME, &params, fs::interface()) }
        .map_err(|s| format!("WinFsp cannot create the file system (status {s:#010x})"))?;
    let mut mounted = None;
    let mut refused = Vec::new();
    for drive in &drives {
        // SAFETY: `file_system` is live until it is deleted below.
        match unsafe { api.set_mount_point(file_system, drive) } {
            Ok(()) => {
                mounted = Some(drive.clone());
                break;
            }
            Err(s) => refused.push(format!("{drive} (status {s:#010x})")),
        }
    }
    let Some(drive) = mounted else {
        // SAFETY: nothing runs yet.
        unsafe { api.delete(file_system) };
        return Err(format!("WinFsp cannot mount at {}", refused.join(", ")));
    };
    if !refused.is_empty() {
        log.line(&format!("WinFsp could not mount at {}, so tried the next", refused.join(", ")));
    }
    // SAFETY: as above.
    if let Err(s) = unsafe { api.start_dispatcher(file_system) } {
        // SAFETY: nothing runs.
        unsafe { api.delete(file_system) };
        return Err(format!("WinFsp cannot start (status {s:#010x})"));
    }
    log.line(&format!("mounted at {drive}"));

    let why = shared.wait_dead();
    log.line(&format!("the share is lost: {why}; unmounting"));
    // SAFETY: the file system is live, and this is not in a callback.
    unsafe {
        api.stop_dispatcher(file_system);
        api.delete(file_system);
    }
    Ok(1)
}
