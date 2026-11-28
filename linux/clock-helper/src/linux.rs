//! The card through NVIDIA's management library, the holders through a lock file in `/run`, and the
//! wait on standard input.

use guesthouse_clock_helper::{
    Card, Exit, Fault, Holders, Order, Swept, Verb, hold, is_nvidia_display, parse, reset,
};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::fs;
use std::io::{self, Write};
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::ExitCode;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

const NAME: &str = "guesthouse-clock-helper";
/// Root's folder for the holders' lock files and the marks. `/run` is emptied at boot, as the
/// card's clocks are.
const STATE_FOLDER: &CStr = c"/run/guesthouse-clocks";
/// Root's alone: a user who could open the lock file could take a lock on it and stop the count.
const FOLDER_MODE: libc::mode_t = 0o700;
const FILE_MODE: libc::mode_t = 0o600;
const PCI_DEVICES: &str = "/sys/bus/pci/devices";
const NVIDIA_CARDS: &str = "/proc/driver/nvidia/gpus";
/// The library by its standard name, which the loader looks for only in the system's own places for
/// a setuid program, then where NixOS keeps the driver's libraries.
const LIBRARIES: [&CStr; 2] = [c"libnvidia-ml.so.1", c"/run/opengl-driver/lib/libnvidia-ml.so.1"];
/// How long a wait on standard input lasts before it looks at whether it was told to end.
const LOOK_AGAIN_MS: c_int = 1000;

/// From the library's header.
const SUCCESS: c_int = 0;
const NOT_SUPPORTED: c_int = 3;
const NO_PERMISSION: c_int = 4;
const INSUFFICIENT_SIZE: c_int = 7;
const CLOCK_MEMORY: c_int = 2;
/// Cards list five or six memory clocks.
const CLOCK_ROOM: usize = 64;

static TOLD_TO_END: AtomicBool = AtomicBool::new(false);

extern "C" fn note_end(_signal: c_int) {
    TOLD_TO_END.store(true, Ordering::Release);
}

pub fn main() -> ExitCode {
    // SAFETY: nothing else runs yet, so nothing else reads the environment while it goes.
    unsafe { libc::clearenv() };
    // SAFETY: plain calls with no pointers.
    unsafe { libc::umask(0o022) };
    let order = match parse(std::env::args_os().skip(1)) {
        Ok(order) => order,
        Err(refusal) => return fail(Exit::Usage, &refusal.to_string()),
    };
    match run(&order) {
        Ok(()) => ExitCode::SUCCESS,
        Err(fault) => fail(fault.exit, &fault.reason),
    }
}

fn fail(exit: Exit, reason: &str) -> ExitCode {
    eprintln!("{NAME}: {reason}");
    ExitCode::from(exit as u8)
}

fn say(line: &str) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{NAME}: {line}");
    let _ = out.flush();
}

fn run(order: &Order) -> Result<(), Fault> {
    // SAFETY: plain calls with no pointers.
    if unsafe { libc::geteuid() } != 0 {
        return Err(Fault::new(
            Exit::NotRoot,
            "It isn't running as root, so it can't change a card's clocks.",
        ));
    }
    let folder = format!("{PCI_DEVICES}/{}", order.card);
    let read = |name: &str| fs::read_to_string(format!("{folder}/{name}")).unwrap_or_default();
    if !is_nvidia_display(&read("vendor"), &read("class")) {
        return Err(Fault::new(Exit::NotNvidia, format!("{} is not an NVIDIA display card.", order.card)));
    }
    if !real_user_can_open(&order.card) {
        return Err(Fault::new(
            Exit::NotTheirs,
            format!("The user who started this can't open the device file of {}.", order.card),
        ));
    }
    // All of root, so the user who started it can't stop it half way with a signal: only the end of
    // its input, which is theirs to give, ends a hold.
    // SAFETY: plain calls with no pointers.
    if unsafe { libc::setresgid(0, 0, 0) } != 0 || unsafe { libc::setresuid(0, 0, 0) } != 0 {
        return Err(Fault::new(
            Exit::NotRoot,
            format!("It couldn't become root: {}", io::Error::last_os_error()),
        ));
    }
    let mut holders = LockFile::open(&order.card)?;
    match order.verb {
        Verb::Hold => {
            let mut card = Nvml::open(&order.card)?;
            watch_for_end();
            let mhz = hold(&mut card, &mut holders, |mhz| {
                say(&format!("holding the memory clock of {} at {mhz} MHz", order.card));
                wait_for_end();
            })?;
            say(&format!("put the memory clock of {} back, after {mhz} MHz", order.card));
        }
        Verb::Reset => match reset(&mut holders, || Nvml::open(&order.card))? {
            Swept::PutBack => say(&format!("put back the memory clock a holder left on {}", order.card)),
            Swept::Held | Swept::Nothing => {}
        },
    }
    Ok(())
}

/// Whether the user who started this, and not root, can open the card's device file for reading and
/// writing, as a program of theirs on the card does.
fn real_user_can_open(card: &str) -> bool {
    let information = fs::read_to_string(format!("{NVIDIA_CARDS}/{card}/information")).unwrap_or_default();
    guesthouse_clock_helper::device_file(&information).is_some_and(|file| real_user_may(&file))
}

/// `access` asks with the real user and group, which are still the caller's here.
fn real_user_may(file: &str) -> bool {
    let Ok(file) = std::ffi::CString::new(file) else { return false };
    // SAFETY: a plain call with a string that lives across it.
    unsafe { libc::access(file.as_ptr(), libc::R_OK | libc::W_OK) == 0 }
}

// ---- Ending ----

fn watch_for_end() {
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT] {
        // SAFETY: the action is zeroed and given a handler that only stores to an atomic, with no
        // SA_RESTART, so the wait in poll returns to look at it.
        unsafe {
            let mut action: libc::sigaction = mem::zeroed();
            action.sa_sigaction = note_end as extern "C" fn(c_int) as usize;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, ptr::null_mut());
        }
    }
    // SAFETY: setting a signal to be ignored.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        libc::signal(libc::SIGTSTP, libc::SIG_IGN);
    }
}

/// Waits until standard input ends, a signal says to end, or the program that started it has gone.
fn wait_for_end() {
    // SAFETY: plain calls with no pointers.
    let parent = unsafe { libc::getppid() };
    let mut buffer = [0u8; 256];
    loop {
        // SAFETY: as above.
        if TOLD_TO_END.load(Ordering::Acquire) || unsafe { libc::getppid() } != parent {
            return;
        }
        let mut wanted = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd, which outlives the call.
        let ready = unsafe { libc::poll(&mut wanted, 1, LOOK_AGAIN_MS) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if ready == 0 {
            continue;
        }
        if wanted.revents & libc::POLLIN != 0 {
            // SAFETY: reading into a buffer of that length, which outlives the call.
            let got = unsafe { libc::read(0, buffer.as_mut_ptr().cast(), buffer.len()) };
            if got > 0 {
                continue;
            }
            if got < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if wanted.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return;
        }
    }
}

// ---- The holders ----

fn os_fault(what: &str) -> Fault {
    Fault::new(Exit::Failed, format!("{what}: {}", io::Error::last_os_error()))
}

/// Whether a file is root's, of the kind wanted, and then made root's alone with `mode`, whatever mode
/// it was made with. One that isn't root's is refused, never changed.
fn roots_own(fd: c_int, kind: libc::mode_t, mode: libc::mode_t) -> bool {
    // SAFETY: a zeroed stat filled in by fstat, which outlives the call.
    let mut found: libc::stat = unsafe { mem::zeroed() };
    let roots = (unsafe { libc::fstat(fd, &mut found) } == 0)
        && found.st_uid == 0
        && found.st_mode & libc::S_IFMT == kind;
    // SAFETY: an open descriptor, and a mode.
    roots && (found.st_mode & 0o7777 == mode || unsafe { libc::fchmod(fd, mode) } == 0)
}

struct LockFile {
    folder: OwnedFd,
    lock: OwnedFd,
    mark_name: CString,
}

impl LockFile {
    fn open(card: &str) -> Result<Self, Fault> {
        // SAFETY: a path that ends in a zero.
        if unsafe { libc::mkdir(STATE_FOLDER.as_ptr(), FOLDER_MODE) } != 0
            && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists
        {
            return Err(os_fault("Making /run/guesthouse-clocks"));
        }
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: as above.
        let fd = unsafe { libc::open(STATE_FOLDER.as_ptr(), flags) };
        if fd < 0 {
            return Err(os_fault("Opening /run/guesthouse-clocks"));
        }
        // SAFETY: a descriptor just opened, owned from here.
        let folder = unsafe { OwnedFd::from_raw_fd(fd) };
        if !roots_own(folder.as_raw_fd(), libc::S_IFDIR, FOLDER_MODE) {
            return Err(Fault::new(
                Exit::Failed,
                "/run/guesthouse-clocks isn't root's alone, so it isn't used.",
            ));
        }
        // The card's address was checked to be digits, colons and a dot, so it names a file here.
        let name = |suffix: &str| {
            CString::new(format!("{card}{suffix}")).map_err(|_| Fault::new(Exit::Usage, "bad address"))
        };
        let lock_name = name(".lock")?;
        let flags = libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: the folder is open, and the name ends in a zero.
        let fd = unsafe { libc::openat(folder.as_raw_fd(), lock_name.as_ptr(), flags, FILE_MODE as c_uint) };
        if fd < 0 {
            return Err(os_fault("Opening the card's lock file"));
        }
        // SAFETY: as above.
        let lock = unsafe { OwnedFd::from_raw_fd(fd) };
        if !roots_own(lock.as_raw_fd(), libc::S_IFREG, FILE_MODE) {
            return Err(Fault::new(
                Exit::Failed,
                "The card's lock file isn't root's alone, so it isn't used.",
            ));
        }
        Ok(Self { folder, lock, mark_name: name(".held")? })
    }

    fn flock(&self, how: c_int) -> bool {
        loop {
            // SAFETY: an open descriptor.
            if unsafe { libc::flock(self.lock.as_raw_fd(), how) } == 0 {
                return true;
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return false;
            }
        }
    }
}

impl Holders for LockFile {
    fn join(&mut self) -> Result<(), Fault> {
        if self.flock(libc::LOCK_SH) { Ok(()) } else { Err(os_fault("Joining the card's holders")) }
    }

    fn leave_last(&mut self) -> bool {
        // Linux lets go of the shared lock before it tries for the whole one, so of two leaving at
        // once at least one gets it.
        self.flock(libc::LOCK_EX | libc::LOCK_NB)
    }

    fn take_if_free(&mut self) -> bool {
        self.flock(libc::LOCK_EX | libc::LOCK_NB)
    }

    fn mark(&mut self, mhz: u32) {
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: the folder is open, and the name ends in a zero.
        let fd = unsafe {
            libc::openat(self.folder.as_raw_fd(), self.mark_name.as_ptr(), flags, FILE_MODE as c_uint)
        };
        if fd >= 0 {
            // SAFETY: a descriptor just opened, owned from here.
            let mut file = fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
            let _ = writeln!(file, "{mhz}");
        }
    }

    fn marked(&self) -> bool {
        // SAFETY: a zeroed stat filled in by fstatat, which outlives the call.
        let mut found: libc::stat = unsafe { mem::zeroed() };
        let there = unsafe {
            libc::fstatat(
                self.folder.as_raw_fd(),
                self.mark_name.as_ptr(),
                &mut found,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0;
        there && found.st_uid == 0 && found.st_mode & libc::S_IFMT == libc::S_IFREG
    }

    fn unmark(&mut self) {
        // SAFETY: as above.
        unsafe { libc::unlinkat(self.folder.as_raw_fd(), self.mark_name.as_ptr(), 0) };
    }
}

// ---- The card ----

type Device = *mut c_void;
type Init = unsafe extern "C" fn() -> c_int;
type HandleByBusId = unsafe extern "C" fn(*const c_char, *mut Device) -> c_int;
type SupportedClocks = unsafe extern "C" fn(Device, *mut c_uint, *mut c_uint) -> c_int;
type MaxClock = unsafe extern "C" fn(Device, c_int, *mut c_uint) -> c_int;
type LockClocks = unsafe extern "C" fn(Device, c_uint, c_uint) -> c_int;
type ResetClocks = unsafe extern "C" fn(Device) -> c_int;

struct Nvml {
    device: Device,
    supported: Option<SupportedClocks>,
    max_clock: Option<MaxClock>,
    lock: Option<LockClocks>,
    reset: Option<ResetClocks>,
}

/// Looks a function up by name.
///
/// # Safety
///
/// `F` must be a function pointer type matching the library's signature for `name`.
unsafe fn symbol<F: Copy>(library: *mut c_void, name: &CStr) -> Option<F> {
    // SAFETY: the library handle is open, and the name ends in a zero.
    let address = unsafe { libc::dlsym(library, name.as_ptr()) };
    // SAFETY: a function pointer and a data pointer are the same size here, and the caller vouches
    // for the signature.
    (!address.is_null()).then(|| unsafe { mem::transmute_copy(&address) })
}

fn answer(what: &str, code: c_int) -> Result<(), Fault> {
    match code {
        SUCCESS => Ok(()),
        NOT_SUPPORTED => {
            Err(Fault::new(Exit::NotSupported, format!("{what}: this card or driver doesn't support it.")))
        }
        NO_PERMISSION => Err(Fault::new(Exit::NotRoot, format!("{what}: the driver refused root."))),
        code => Err(Fault::new(Exit::Failed, format!("{what}: NVIDIA's library answered {code}."))),
    }
}

impl Nvml {
    fn open(card: &str) -> Result<Self, Fault> {
        let missing =
            || Fault::new(Exit::NoLibrary, "NVIDIA's management library, libnvidia-ml.so.1, isn't there.");
        let library = LIBRARIES
            .iter()
            // SAFETY: loading a library by name; nothing else in this process depends on it.
            .map(|name| unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) })
            .find(|library| !library.is_null())
            .ok_or_else(missing)?;
        // SAFETY: each type is that function's documented signature.
        unsafe {
            let init: Init = symbol(library, c"nvmlInit_v2").ok_or_else(missing)?;
            let by_bus: HandleByBusId =
                symbol(library, c"nvmlDeviceGetHandleByPciBusId_v2").ok_or_else(missing)?;
            if init() != SUCCESS {
                return Err(Fault::new(Exit::NoLibrary, "NVIDIA's management library couldn't start."));
            }
            let address = CString::new(card).map_err(|_| missing())?;
            let mut device = ptr::null_mut();
            if by_bus(address.as_ptr(), &mut device) != SUCCESS {
                return Err(Fault::new(
                    Exit::NoLibrary,
                    format!("NVIDIA's management library doesn't know {card}."),
                ));
            }
            Ok(Self {
                device,
                supported: symbol(library, c"nvmlDeviceGetSupportedMemoryClocks"),
                max_clock: symbol(library, c"nvmlDeviceGetMaxClockInfo"),
                lock: symbol(library, c"nvmlDeviceSetMemoryLockedClocks"),
                reset: symbol(library, c"nvmlDeviceResetMemoryLockedClocks"),
            })
        }
    }
}

impl Card for Nvml {
    fn supported_memory_clocks(&mut self) -> Result<Vec<u32>, Fault> {
        if let Some(supported) = self.supported {
            let mut clocks = vec![0 as c_uint; CLOCK_ROOM];
            let mut count = CLOCK_ROOM as c_uint;
            // SAFETY: the count is the room in `clocks`, and both outlive the call.
            let code = unsafe { supported(self.device, &mut count, clocks.as_mut_ptr()) };
            if code == SUCCESS {
                clocks.truncate((count as usize).min(CLOCK_ROOM));
                return Ok(clocks);
            }
            if code != NOT_SUPPORTED && code != INSUFFICIENT_SIZE {
                answer("Reading the card's memory clocks", code)?;
            }
        }
        // A card that won't list them still says its highest.
        let max_clock = self.max_clock.ok_or_else(|| {
            Fault::new(Exit::NotSupported, "The driver can't say the card's memory clocks.")
        })?;
        let mut mhz = 0;
        // SAFETY: as above.
        answer("Reading the card's highest memory clock", unsafe {
            max_clock(self.device, CLOCK_MEMORY, &mut mhz)
        })?;
        Ok(vec![mhz])
    }

    fn lock_memory_clock(&mut self, mhz: u32) -> Result<(), Fault> {
        let lock = self
            .lock
            .ok_or_else(|| Fault::new(Exit::NotSupported, "The driver is too old to lock a memory clock."))?;
        // SAFETY: the device came from the library, which is still loaded.
        answer("Locking the memory clock", unsafe { lock(self.device, mhz, mhz) })
    }

    fn reset_memory_clock(&mut self) -> Result<(), Fault> {
        let reset = self.reset.ok_or_else(|| {
            Fault::new(Exit::NotSupported, "The driver is too old to reset a memory clock.")
        })?;
        // SAFETY: as above.
        answer("Putting the memory clock back", unsafe { reset(self.device) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_a_file_the_real_user_can_read_and_write_counts() {
        let folder = std::env::temp_dir().join(format!("gh-clock-helper-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let (theirs, not) = (folder.join("theirs"), folder.join("not"));
        fs::write(&theirs, "").unwrap();
        fs::write(&not, "").unwrap();
        fs::set_permissions(&not, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(real_user_may(&theirs.display().to_string()));
        // SAFETY: a plain call.
        if unsafe { libc::getuid() } != 0 {
            assert!(!real_user_may(&not.display().to_string()), "read alone isn't enough");
        }
        assert!(!real_user_may("/dev/nvidia-no-such-card"));
        fs::remove_dir_all(folder).unwrap();
    }
}
