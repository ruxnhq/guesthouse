//! What the app asks of Windows itself, beyond the window: the file dialog, the clipboard, Remote
//! Desktop, the icon's file, cancelling a read on the pipe, what opens at sign-in, the file drop,
//! for a newer Guesthouse, and restarting Windows when the person says. Every `unsafe` call of the app is
//! here, but for the console's, which are in console.rs.

use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::process::Command;
use std::thread::{self, JoinHandle};

use std::fs::File;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use guesthouse_control::updates::Latest;

use crate::update::{self, DownloadError, files};

use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::Shell::{
    BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListW,
};

/// The clipboard's format for text in UTF-16.
const UNICODE_TEXT: u32 = 13;
/// The longest path the file dialog gives back, in UTF-16 units: Windows' longest path.
const LONGEST_PATH: usize = 32 * 1024;

/// Ends a read that `thread` is waiting in, if it's in one. The read then fails, as if the pipe had
/// broken.
pub fn cancel_read(thread: &JoinHandle<()>) {
    // SAFETY: the handle is the thread's own, held open by `thread` for the length of the call.
    // Cancelling another thread's synchronous I/O is what the call is for, and only fails when
    // there's nothing to cancel.
    unsafe {
        CancelSynchronousIo(thread.as_raw_handle());
    }
}

/// Puts `text` on the clipboard. `owner` is the window's handle. Returns whether it's there.
pub fn copy(owner: isize, text: &str) -> bool {
    let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let bytes = units.len() * size_of::<u16>();
    // SAFETY: the clipboard is opened and closed here, and the memory handed to it is a movable
    // block of `bytes`, filled while locked. Once `SetClipboardData` takes the block it's the
    // system's, so it's freed here only when that fails.
    unsafe {
        if OpenClipboard(owner as HWND) == 0 {
            return false;
        }
        let mut put = false;
        if EmptyClipboard() != 0 {
            let block = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if !block.is_null() {
                let at = GlobalLock(block) as *mut u16;
                if !at.is_null() {
                    std::ptr::copy_nonoverlapping(units.as_ptr(), at, units.len());
                    GlobalUnlock(block);
                    put = !SetClipboardData(UNICODE_TEXT, block).is_null();
                }
                if !put {
                    GlobalFree(block);
                }
            }
        }
        CloseClipboard();
        put
    }
}

/// Windows' file dialog for a machine's image, on a thread of its own, as it runs its own loop of
/// window messages, which mustn't run inside the window's update. `owner` is the window's handle,
/// so the dialog stays in front of it. `chosen` gets the path, or `None` when nothing was chosen.
/// Returns whether the dialog could open.
pub fn pick_image(owner: isize, chosen: impl FnOnce(Option<PathBuf>) + Send + 'static) -> bool {
    thread::Builder::new().name("file dialog".into()).spawn(move || chosen(open_dialog(owner))).is_ok()
}

fn open_dialog(owner: isize) -> Option<PathBuf> {
    let filter: Vec<u16> =
        "Windows or Ubuntu images (*.iso, *.wim, *.esd)\0*.iso;*.wim;*.esd\0All files\0*.*\0\0"
            .encode_utf16()
            .collect();
    let title: Vec<u16> = "Choose the image to make the machine from\0".encode_utf16().collect();
    let mut file = vec![0u16; LONGEST_PATH];
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner as HWND,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrTitle: title.as_ptr(),
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // SAFETY: COM is started on this thread for the dialog, which needs it for the shell's folders,
    // and ended after it. Every pointer in `dialog` is to a buffer above that outlives the call, the
    // strings end in nulls as the call wants, and `nMaxFile` is the path buffer's length.
    let picked = unsafe {
        let com = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let picked = GetOpenFileNameW(&mut dialog) != 0;
        if com >= 0 {
            CoUninitialize();
        }
        picked
    };
    if !picked {
        return None;
    }
    let end = file.iter().position(|&unit| unit == 0).unwrap_or(file.len());
    Some(PathBuf::from(String::from_utf16_lossy(&file[..end])))
}

/// Windows' Save As dialog for a report, on a thread of its own, as the open dialog is. `name` is the
/// file name it starts with. `chosen` gets the path, or `None` when nothing was chosen. Returns
/// whether the dialog could open.
pub fn pick_save(owner: isize, name: &str, chosen: impl FnOnce(Option<PathBuf>) + Send + 'static) -> bool {
    let name = name.to_owned();
    thread::Builder::new().name("save dialog".into()).spawn(move || chosen(save_dialog(owner, &name))).is_ok()
}

fn save_dialog(owner: isize, name: &str) -> Option<PathBuf> {
    let filter: Vec<u16> =
        "Guesthouse report (*.tar.gz)\0*.tar.gz\0All files\0*.*\0\0".encode_utf16().collect();
    let title: Vec<u16> = "Save the report\0".encode_utf16().collect();
    let extension: Vec<u16> = "gz\0".encode_utf16().collect();
    let mut file = vec![0u16; LONGEST_PATH];
    // The name it starts with, which always fits, as a file's name is far shorter than the buffer.
    for (unit, put) in name.encode_utf16().take(file.len() - 1).zip(file.iter_mut()) {
        *put = unit;
    }
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner as HWND,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrDefExt: extension.as_ptr(),
        Flags: OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // SAFETY: as in `open_dialog`: COM is started on this thread for the dialog and ended after it,
    // and every pointer in `dialog` is to a buffer above that outlives the call, the strings end in
    // nulls as the call wants, and `nMaxFile` is the path buffer's length.
    let picked = unsafe {
        let com = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let picked = GetSaveFileNameW(&mut dialog) != 0;
        if com >= 0 {
            CoUninitialize();
        }
        picked
    };
    if !picked {
        return None;
    }
    let end = file.iter().position(|&unit| unit == 0).unwrap_or(file.len());
    Some(PathBuf::from(String::from_utf16_lossy(&file[..end])))
}

/// Starts a program with no console window of its own, as the app has none to share.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Whether this is an installed app, beside the installer's uninstaller. Only an installed app is
/// offered updates: a build of the repo is whatever it was built from.
pub fn installed() -> bool {
    install_folder().is_some()
}

/// The curl Windows has, which needs no library of ours for HTTPS.
fn curl() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join(r"System32\curl.exe")
}

/// The newest Windows host's file, fetched with the curl Windows has. Only HTTPS, from the file
/// drop's own address with no redirect followed, with a time limit, and no more than the file can be.
fn fetch_latest() -> update::Fetched {
    use std::os::windows::process::CommandExt;
    let limit = guesthouse_control::updates::LONGEST.to_string();
    let output = Command::new(curl())
        .args(["--fail", "--silent", "--globoff", "--proto", "=https", "--max-time", "20"])
        .args(["--max-filesize", &limit, guesthouse_control::updates::LATEST_URL])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            update::Fetched::Text(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        _ => update::Fetched::Unreached,
    }
}

/// The updater that reaches the file drop, and downloads into the app's own folder for updates,
/// which it empties as it starts: nothing in it is trusted from one run of the app to the next.
pub struct Network {
    /// Where downloads go, if Windows says where this user's app data is.
    folder: Option<PathBuf>,
    /// The download that ended last, by its attempt, and its file.
    downloaded: Mutex<Option<(u64, PathBuf)>>,
    /// That file, held open from its check until it's let go, so that no other program can change,
    /// move or delete it in between: what was checked is what's run.
    held: Mutex<Option<(u64, File)>>,
}

impl Network {
    pub fn new(app_folder: Option<&Path>) -> Network {
        let folder = app_folder.map(files::folder);
        if let Some(folder) = &folder {
            files::clear(folder);
        }
        Network { folder, downloaded: Mutex::new(None), held: Mutex::new(None) }
    }

    /// The file download `attempt` ended with, if it's the last one's.
    fn file_of(&self, attempt: u64) -> Option<PathBuf> {
        let downloaded = self.downloaded.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        downloaded.as_ref().filter(|(of, _)| *of == attempt).map(|(_, file)| file.clone())
    }
}

impl update::Updater for Network {
    fn fetch(&self) -> update::Fetched {
        fetch_latest()
    }

    fn download(
        &self,
        latest: &Latest,
        attempt: u64,
        progress: &mut dyn FnMut(u64, Option<u64>),
        stop: &AtomicBool,
    ) -> Result<(), DownloadError> {
        let folder = self.folder.as_deref().ok_or_else(|| {
            DownloadError::Disk("Windows doesn't say where this user's app data is".to_owned())
        })?;
        std::fs::create_dir_all(folder).map_err(|error| DownloadError::Disk(error.to_string()))?;
        let part = files::partial(folder, &latest.version, attempt);
        let headers = files::headers(folder, &latest.version, attempt);
        let ended = download_to(&latest.installer, &part, &headers, progress, stop);
        let _ = std::fs::remove_file(&headers);
        if let Err(failed) = ended {
            let _ = std::fs::remove_file(&part);
            return Err(failed);
        }
        let installer = files::installer(folder, &latest.version, attempt);
        if let Err(error) = std::fs::rename(&part, &installer) {
            let _ = std::fs::remove_file(&part);
            return Err(DownloadError::Disk(error.to_string()));
        }
        *self.downloaded.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((attempt, installer));
        Ok(())
    }

    fn hash(&self, attempt: u64) -> Result<String, String> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
        let file = self.file_of(attempt).ok_or("it isn't there")?;
        // Others may read it, as Windows does to run it, but none may write, rename or delete it
        // while it's held.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&file)
            .map_err(|error| error.to_string())?;
        let hash = files::sha256(&held).map_err(|error| error.to_string())?;
        self.keep(attempt, &file, held)?;
        Ok(hash)
    }

    fn launch(&self, attempt: u64) -> Result<Box<dyn update::Running>, String> {
        let held = self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if held.as_ref().is_none_or(|(of, _)| *of != attempt) {
            return Err("it wasn't the file that was checked".to_owned());
        }
        let file = self.file_of(attempt).ok_or("it isn't there")?;
        // As the person, with nothing on its command line: the installer's own starter asks Windows
        // for an administrator, which shows Windows' prompt, and opens Guesthouse again at its end.
        let child = Command::new(&file)
            .current_dir(file.parent().unwrap_or(Path::new(".")))
            .spawn()
            .map_err(|error| error.to_string())?;
        Ok(Box::new(Installer(child)))
    }

    fn discard(&self) {
        // In the order a check takes them, so the two never wait on each other.
        let mut downloaded = self.downloaded.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *downloaded = None;
        drop(downloaded);
        if let Some(folder) = &self.folder {
            files::clear(folder);
        }
    }
}

impl Network {
    /// Holds download `attempt`'s `file` until it's let go of, if it's still the download that's
    /// wanted. One put aside while it was read (by Cancel, say) is let go of and deleted, as the
    /// discard couldn't delete it while it was held.
    fn keep(&self, attempt: u64, file: &Path, held: File) -> Result<(), String> {
        let downloaded = self.downloaded.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if downloaded.as_ref().is_some_and(|(of, kept)| *of == attempt && kept == file) {
            *self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((attempt, held));
            return Ok(());
        }
        drop(held);
        let _ = std::fs::remove_file(file);
        Err("it was put aside while it was checked".to_owned())
    }
}

/// An installer that's been started.
struct Installer(std::process::Child);

impl update::Running for Installer {
    fn wait(mut self: Box<Self>) -> i32 {
        self.0.wait().ok().and_then(|status| status.code()).unwrap_or(-1)
    }
}

/// How often a download's file is looked at, for how far it's got and whether it's to stop.
const DOWNLOAD_LOOK: Duration = Duration::from_millis(250);

/// Downloads `url` to `part` with the curl Windows has, as `files::curl_words` says, telling
/// `progress` how far it's got, until it ends or `stop` is set. The file's size and the time are
/// watched here too, as an answer may not say its length. curl runs in a job that ends it with the
/// app, so a download never goes on, or holds its file, once the app has gone.
fn download_to(
    url: &str,
    part: &Path,
    headers: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
    stop: &AtomicBool,
) -> Result<(), DownloadError> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    let mut child = Command::new(curl())
        .args(files::curl_words(url, part, headers))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| DownloadError::Other(format!("curl didn't start: {error}")))?;
    end_with_the_app(&child);
    // What curl writes is a few words, read on threads of their own so it never waits on them.
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let status = read(child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let said = read(child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let started = Instant::now();
    let mut total = None;
    let ended = loop {
        let stopped = if stop.load(Ordering::SeqCst) {
            Some(DownloadError::Other("it was cancelled".to_owned()))
        } else {
            None
        };
        match child.try_wait() {
            Ok(Some(exit)) if stopped.is_none() => break Ok(exit.code()),
            Err(error) => break Err(DownloadError::Other(error.to_string())),
            _ => {}
        }
        let got = std::fs::metadata(part).map_or(0, |file| file.len());
        if total.is_none() {
            total = std::fs::read_to_string(headers).ok().as_deref().and_then(files::content_length);
        }
        let too_large = got > files::LARGEST || total.is_some_and(|total| total > files::LARGEST);
        let too_long = started.elapsed() > Duration::from_secs(files::LONGEST_SECONDS + 60);
        let failed = if stopped.is_some() {
            stopped
        } else if too_large {
            Some(DownloadError::TooLarge)
        } else if too_long {
            Some(DownloadError::TooLong)
        } else {
            None
        };
        if let Some(failed) = failed {
            let _ = child.kill();
            let _ = child.wait();
            break Err(failed);
        }
        progress(got, total);
        thread::sleep(DOWNLOAD_LOOK);
    };
    let (status, said) = (status.join().unwrap_or_default(), said.join().unwrap_or_default());
    let answer = files::curl_answer(ended?, &status, &said);
    if answer.is_ok() {
        let got = std::fs::metadata(part).map_or(0, |file| file.len());
        if got > files::LARGEST {
            return Err(DownloadError::TooLarge);
        }
        progress(got, total.or(Some(got)));
    }
    answer
}

/// Puts `child` in a job that ends it when the app ends, however the app ends. The job is made
/// once, and closed only by Windows as the app goes. Only a download's curl goes in it, never the
/// installer, which must outlive the app.
fn end_with_the_app(child: &std::process::Child) {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // SAFETY: a job with no name and the default security; its handle is this process's, kept
        // for as long as the process runs.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return 0;
        }
        // SAFETY: an all-zero limit block is valid, and only the flag is set in it.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the job is open, and the block is the type and size the class names.
        unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
        }
        job as usize
    });
    if job != 0 {
        // SAFETY: the job's handle is open for the life of the process, and the child's is held by
        // `child` for the length of the call. If it fails, curl's own time limit still ends it.
        unsafe {
            AssignProcessToJobObject(job as _, child.as_raw_handle());
        }
    }
}

/// The install folder, if this is an installed app: the folder above the app's own.
pub fn install_folder() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let folder = exe.parent()?.parent()?;
    folder.join("uninstall.exe").is_file().then(|| folder.to_path_buf())
}

/// Opens the licences of what's installed with Guesthouse, `THIRD-PARTY.md` in the install folder,
/// in Notepad, as Windows may have nothing else for a .md file. A build of the repo has no install
/// folder, so the repo's own file is opened in the browser. Returns words for the person when it
/// couldn't.
pub fn open_licences() -> Result<(), String> {
    let Some(file) =
        install_folder().map(|folder| folder.join("THIRD-PARTY.md")).filter(|file| file.is_file())
    else {
        return open_link(LICENCES_PAGE);
    };
    Command::new("notepad.exe")
        .arg(&file)
        .spawn()
        .map(drop)
        .map_err(|error| format!("Notepad didn't open: {error}. The licences are in {}", file.display()))
}

/// The licences of what Guesthouse is built with, in the repo.
const LICENCES_PAGE: &str = "https://github.com/ruxnlabs/guesthouse/blob/main/THIRD-PARTY.md";

/// Where Windows keeps what opens when this user signs in, and the app's name there.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "Guesthouse";

/// What the Run key holds to open this app: its path, in quotes as a path with spaces needs.
fn run_command() -> Option<String> {
    Some(format!("\"{}\"", std::env::current_exe().ok()?.display()))
}

/// Whether this app opens when the person signs in: the Run key names this app's own exe.
pub fn opens_at_sign_in() -> bool {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let (key, name) = (wide(RUN_KEY), wide(RUN_NAME));
    let mut buffer = vec![0u16; LONGEST_PATH];
    let mut bytes = (buffer.len() * size_of::<u16>()) as u32;
    // SAFETY: both names end in a null, and `buffer` holds `bytes` bytes.
    let found = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if found != ERROR_SUCCESS {
        return false;
    }
    let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
    let held = String::from_utf16_lossy(&buffer[..end]);
    run_command().is_some_and(|command| held.eq_ignore_ascii_case(&command))
}

/// Makes this app open when the person signs in, or not. Returns words for the person when Windows
/// wouldn't.
pub fn set_opens_at_sign_in(on: bool) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
    };
    let (key, name) = (wide(RUN_KEY), wide(RUN_NAME));
    let done = if on {
        let command = wide(&run_command().ok_or("The app's own path isn't known.")?);
        // SAFETY: both names end in a null, and the value is `command`'s bytes with its null, as a
        // REG_SZ is kept.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * size_of::<u16>()) as u32,
            )
        }
    } else {
        // SAFETY: both names end in a null.
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) } {
            // Not there is off already.
            ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
            other => other,
        }
    };
    if done == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!(
            "Windows didn't keep that (error {done}). Try again, or use Task Manager's Startup apps."
        ))
    }
}

/// A string as Windows' calls take it, with its null.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Opens a web page in the person's browser. Returns words for the person when it couldn't.
pub fn open_link(link: &str) -> Result<(), String> {
    Command::new("explorer.exe")
        .arg(link)
        .spawn()
        .map(drop)
        .map_err(|error| format!("The browser didn't open: {error}. The page is {link}"))
}

/// Opens Windows' folder picker on a thread of its own, as it waits until the person chooses, and
/// hands `chosen` the folder, or `None` if they cancelled. Returns whether the picker opened.
pub fn pick_folder(owner: isize, chosen: impl FnOnce(Option<PathBuf>) + Send + 'static) -> bool {
    thread::Builder::new().name("folder dialog".into()).spawn(move || chosen(folder_dialog(owner))).is_ok()
}

fn folder_dialog(owner: isize) -> Option<PathBuf> {
    let title: Vec<u16> = "Choose the folder the machine shares with this PC\0".encode_utf16().collect();
    let info = BROWSEINFOW {
        hwndOwner: owner as HWND,
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        ..Default::default()
    };
    let mut path = vec![0u16; LONGEST_PATH];
    // SAFETY: COM is started on this thread for the picker, which needs it, and ended after it. The
    // title ends in a null and outlives the call. The item list the picker returns is the caller's,
    // freed here once its path is read into a buffer of MAX_PATH units at least, as the call wants.
    let picked = unsafe {
        let com = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let list = SHBrowseForFolderW(&info);
        let picked = !list.is_null() && SHGetPathFromIDListW(list, path.as_mut_ptr()) != 0;
        if !list.is_null() {
            CoTaskMemFree(list.cast());
        }
        if com >= 0 {
            CoUninitialize();
        }
        picked
    };
    if !picked {
        return None;
    }
    let end = path.iter().position(|&unit| unit == 0).unwrap_or(path.len());
    Some(PathBuf::from(String::from_utf16_lossy(&path[..end])))
}

/// A time in seconds since 1970 in UTC as this PC's own clock shows it, such as `2026-11-23 14:05`.
pub fn local_time(seconds: u64) -> String {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    /// FILETIME counts tenths of a microsecond from 1601.
    const FROM_1601: u64 = 11_644_473_600;
    const TICKS: u64 = 10_000_000;
    let ticks = (seconds + FROM_1601) * TICKS;
    let file = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: every pointer is to a value on the stack that outlives the call, and a null time zone
    // is this PC's own.
    let converted = unsafe {
        FileTimeToSystemTime(&file, &mut utc) != 0
            && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0
    };
    if !converted {
        return format!("{seconds} s after 1970");
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
    )
}

/// Opens Remote Desktop on `address`. Returns words for the person when it couldn't.
pub fn remote_desktop(address: &str) -> Result<(), String> {
    Command::new("mstsc")
        .arg(format!("/v:{address}"))
        .spawn()
        .map(drop)
        .map_err(|error| format!("Remote Desktop didn't open: {error}"))
}

/// The exe's icon, as make-icon.ps1 draws it: a PNG for each size.
const ICON: &[u8] = include_bytes!("../assets/guesthouse.ico");
/// An icon file's header: where it says how many sizes follow, and its length. Each size's entry
/// after it has its width (one byte, as is its height), and where its picture is in the file and
/// how long, as little-endian u32s.
const ICON_COUNT_AT: usize = 4;
const ICON_HEADER: usize = 6;
const ICON_ENTRY: usize = 16;
const ENTRY_WIDTH_AT: usize = 0;
const ENTRY_LENGTH_AT: usize = 8;
const ENTRY_OFFSET_AT: usize = 12;

/// One size of the icon, as a PNG. The small ones are drawn plainer than the SVG, so they stay
/// clear at a title bar's size, and they're the same picture the taskbar shows.
pub fn icon_frame(size: u8) -> Option<&'static [u8]> {
    let field = |at: usize, len: usize| ICON.get(at..at + len);
    let word = |bytes: &[u8], at: usize| -> Option<usize> {
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    let count = u16::from_le_bytes(field(ICON_COUNT_AT, 2)?.try_into().ok()?) as usize;
    (0..count).find_map(|n| {
        let entry = field(ICON_HEADER + n * ICON_ENTRY, ICON_ENTRY)?;
        if entry[ENTRY_WIDTH_AT] != size {
            return None;
        }
        field(word(entry, ENTRY_OFFSET_AT)?, word(entry, ENTRY_LENGTH_AT)?)
    })
}

/// The window's icon, which WinUI takes as a file: written once into this user's temporary folder,
/// as it's part of the exe and nothing ships beside it.
pub fn icon_file() -> Option<String> {
    let folder = std::env::temp_dir().join("guesthouse-app");
    let path = folder.join("guesthouse.ico");
    if std::fs::read(&path).ok().as_deref() != Some(ICON) {
        std::fs::create_dir_all(&folder).ok()?;
        std::fs::write(&path, ICON).ok()?;
    }
    Some(path.to_string_lossy().into_owned())
}

/// Where a panic is written, as the app has no console.
pub fn panic_file() -> PathBuf {
    std::env::temp_dir().join("guesthouse-app").join("panic.txt")
}

/// Asks Windows to restart, as Start's own Restart does: each app is asked to close, and one with
/// unsaved work can hold the restart up until the person says, as nothing is forced. The Guesthouse
/// service is asked to stop as Windows shuts down, which asks its running machines to shut down
/// first. Called only once the person has said yes twice, and never by a preview.
pub fn restart_windows() -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, LUID};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Shutdown::{
        EWX_REBOOT, ExitWindowsEx, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OPERATINGSYSTEM,
        SHTDN_REASON_MINOR_RECONFIG,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo-handle for this process needs no closing, and `token` is a place for the
    // token's handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }
        == 0
    {
        return Err("Windows didn't let the app ask for a restart. Restart from Start instead.".to_owned());
    }
    // A user may restart the PC, but the privilege that lets a program ask for it is off until it's
    // turned on.
    let name: Vec<u16> = "SeShutdownPrivilege".encode_utf16().chain(Some(0)).collect();
    let mut luid = LUID { LowPart: 0, HighPart: 0 };
    // SAFETY: the name ends in a zero and `luid` is a place for the value.
    let found = unsafe { LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) } != 0;
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
    };
    // SAFETY: the token is open for adjusting, and `privileges` is a whole TOKEN_PRIVILEGES.
    let adjusted = found
        && unsafe {
            AdjustTokenPrivileges(token, 0, &privileges, 0, std::ptr::null_mut(), std::ptr::null_mut())
        } != 0
        // SAFETY: a plain question about the call just made.
        && unsafe { GetLastError() } == 0;
    // SAFETY: the token's handle is ours and isn't used after this.
    unsafe { CloseHandle(token) };
    if !adjusted {
        return Err(
            "Windows didn't let this user restart it from the app. Restart from Start instead.".to_owned()
        );
    }
    let reason = SHTDN_REASON_MAJOR_OPERATINGSYSTEM | SHTDN_REASON_MINOR_RECONFIG | SHTDN_REASON_FLAG_PLANNED;
    // SAFETY: a plain request. Without EWX_FORCE or EWX_FORCEIFHUNG, no app is closed unasked.
    if unsafe { ExitWindowsEx(EWX_REBOOT, reason) } == 0 {
        return Err("Windows didn't start the restart. Restart from Start instead.".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::Updater;

    /// An updater on a folder of the test's own, with download `attempt` already there, as a
    /// download that ended would leave it.
    fn downloaded(test: &str, attempt: u64, bytes: &[u8]) -> (Network, PathBuf, PathBuf) {
        let app = std::env::temp_dir().join(format!("gh-app-update-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app);
        let network = Network::new(Some(&app));
        let folder = files::folder(&app);
        std::fs::create_dir_all(&folder).unwrap();
        let file = files::installer(&folder, "0.14.0", attempt);
        std::fs::write(&file, bytes).unwrap();
        *network.downloaded.lock().unwrap() = Some((attempt, file.clone()));
        (network, app, file)
    }

    #[test]
    fn a_checked_file_is_held_so_nothing_can_change_move_or_delete_it_until_its_let_go() {
        let (network, app, file) = downloaded("held", 3, b"abc");
        assert_eq!(
            network.hash(3).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(std::fs::OpenOptions::new().write(true).open(&file).is_err(), "no one may write it");
        assert!(std::fs::rename(&file, file.with_extension("moved")).is_err(), "or move it");
        assert!(std::fs::remove_file(&file).is_err(), "or delete it");
        assert_eq!(std::fs::read(&file).unwrap(), b"abc", "but it can be read, as Windows does to run it");
        network.discard();
        assert!(!file.exists(), "let go of, it's deleted");
        std::fs::remove_dir_all(&app).unwrap();
    }

    #[test]
    fn only_the_file_that_was_checked_is_launched() {
        let (network, app, _) = downloaded("launch", 3, b"not an installer");
        assert!(network.launch(3).is_err(), "not checked yet");
        assert!(network.hash(4).is_err(), "another download's isn't there");
        network.hash(3).unwrap();
        assert!(network.launch(4).is_err(), "another download's wasn't checked");
        network.discard();
        assert!(network.launch(3).is_err(), "nor once it's been let go");
        std::fs::remove_dir_all(&app).unwrap();
    }

    #[test]
    fn a_held_file_can_be_started() {
        // A harmless program of Windows' own in place of an installer, which, with nothing on its
        // command line, says so and ends.
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let program = std::fs::read(PathBuf::from(root).join(r"System32\where.exe")).unwrap();
        let (network, app, _) = downloaded("start-held", 5, &program);
        network.hash(5).unwrap();
        let running = network.launch(5).expect("a held file is started as Windows reads it");
        assert_ne!(running.wait(), -1, "and it ran to its end");
        network.discard();
        std::fs::remove_dir_all(&app).unwrap();
    }

    #[test]
    fn a_file_put_aside_while_it_was_checked_is_let_go_of_and_deleted() {
        let (network, app, file) = downloaded("aside", 6, b"abc");
        // Opened as a check opens it.
        let held = {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
            std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&file).unwrap()
        };
        // Cancel's discard, as if it came while the check was reading it.
        network.discard();
        assert!(file.exists(), "the discard couldn't delete it while it was open");
        assert!(network.keep(6, &file, held).is_err());
        assert!(network.held.lock().unwrap().is_none(), "nothing is held");
        assert!(!file.exists(), "and it's gone");
        std::fs::remove_dir_all(&app).unwrap();
    }

    #[test]
    fn a_new_updater_empties_the_folder_of_what_an_earlier_run_left() {
        let (_, app, file) = downloaded("start", 1, b"left behind");
        let part = files::partial(&files::folder(&app), "0.14.0", 2);
        std::fs::write(&part, b"half").unwrap();
        let network = Network::new(Some(&app));
        assert!(!file.exists() && !part.exists());
        assert!(network.hash(1).is_err(), "nothing from before is checked, let alone run");
        std::fs::remove_dir_all(&app).unwrap();
    }
}
