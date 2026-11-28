//! What went wrong in a Windows call: what was being done, the code Windows gave, and the document HCS
//! sends with a failure, which says far more than the code does.

use std::fmt;
use windows_sys::Win32::Foundation::{E_ACCESSDENIED, ERROR_ACCESS_DENIED, GetLastError, WIN32_ERROR};
use windows_sys::core::HRESULT;

#[derive(Debug)]
pub struct Error {
    pub doing: String,
    /// An HRESULT, with a Win32 error code folded in the way Windows folds one.
    pub code: HRESULT,
    pub detail: Option<String>,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Win32's own fold of an error code into an HRESULT.
const FACILITY_WIN32: u32 = 7;

pub(crate) fn hresult_from_win32(error: WIN32_ERROR) -> HRESULT {
    if error == 0 { 0 } else { ((error & 0xFFFF) | (FACILITY_WIN32 << 16) | 0x8000_0000) as HRESULT }
}

impl Error {
    pub(crate) fn new(doing: impl Into<String>, code: HRESULT, detail: Option<String>) -> Self {
        Error { doing: doing.into(), code, detail }
    }

    /// Windows' last error on this thread, which a call that returned a bare failure leaves.
    pub(crate) fn last(doing: impl Into<String>) -> Self {
        // SAFETY: GetLastError only reads this thread's error value.
        let error = unsafe { GetLastError() };
        Error::new(doing, hresult_from_win32(error), None)
    }

    pub(crate) fn io(doing: impl Into<String>, error: std::io::Error) -> Self {
        let code = error.raw_os_error().map_or(-1, |code| hresult_from_win32(code as u32));
        Error::new(doing, code, Some(error.to_string()))
    }

    pub fn access_denied(&self) -> bool {
        self.code == E_ACCESSDENIED || self.code == hresult_from_win32(ERROR_ACCESS_DENIED)
    }
}

pub(crate) fn check(code: HRESULT, doing: &str) -> Result<()> {
    if code < 0 { Err(Error::new(doing, code, None)) } else { Ok(()) }
}

pub(crate) fn check_win32(error: WIN32_ERROR, doing: &str) -> Result<()> {
    if error == 0 { Ok(()) } else { Err(Error::new(doing, hresult_from_win32(error), None)) }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: error {:#010x}", self.doing, self.code as u32)?;
        if self.access_denied() {
            write!(f, " (access denied: this needs an elevated prompt)")?;
        }
        if let Some(detail) = &self.detail {
            write!(f, "\n{detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}
