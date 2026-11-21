//! What the app asks of Windows itself, beyond the window: the file dialog, the clipboard, Remote
//! Desktop, the icon's file, and cancelling a read on the pipe. Every `unsafe` call of the app is
//! here.

use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::process::Command;
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
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
