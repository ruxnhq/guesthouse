//! What the compositor says a window of its can do (`xdg_toplevel.wm_capabilities`), as GTK 4.20 and
//! later read it. Older GTK has no way to say, and then nothing is known.

use gtk::gdk;
use gtk::glib::translate::ToGlibPtr;
use std::ffi::{c_char, c_void};
use std::sync::OnceLock;

/// `GDK_TOPLEVEL_CAPABILITIES_MINIMIZE`.
const MINIMIZE: u32 = 1 << 6;

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type GetCapabilities = unsafe extern "C" fn(*mut gdk::ffi::GdkToplevel) -> u32;

/// GTK's `gdk_toplevel_get_capabilities`, if the GTK this runs with has it.
fn get_capabilities() -> Option<GetCapabilities> {
    static FOUND: OnceLock<Option<usize>> = OnceLock::new();
    let address = *FOUND.get_or_init(|| {
        // SAFETY: a lookup of a symbol by name in what the process has loaded (a null handle is
        // RTLD_DEFAULT on glibc); the name is a terminated string.
        let found = unsafe { dlsym(std::ptr::null_mut(), c"gdk_toplevel_get_capabilities".as_ptr()) };
        (!found.is_null()).then_some(found as usize)
    });
    // SAFETY: the symbol is GTK's function of this signature (GdkToplevel* in, a flags value out), as
    // GTK 4.20's header declares it.
    address.map(|address| unsafe { std::mem::transmute::<usize, GetCapabilities>(address) })
}

/// Whether the compositor minimizes this window when asked. `None` when GTK cannot say.
pub fn can_minimize(toplevel: &gdk::Toplevel) -> Option<bool> {
    let get = get_capabilities()?;
    // SAFETY: the pointer is a live GdkToplevel for the length of the call.
    let capabilities = unsafe { get(toplevel.to_glib_none().0) };
    Some(capabilities & MINIMIZE != 0)
}
