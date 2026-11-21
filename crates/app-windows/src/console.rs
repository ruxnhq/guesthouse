//! A machine's console: its own screen from the moment it's turned on, firmware, Windows setup and
//! the sign-in screen included, with the keyboard and mouse going to the guest.
//!
//! HCS's worker serves each machine's display on its console pipe (`guesthouse_control::
//! console_pipe`). What's on that pipe is Remote Desktop, but not as a plain stream that mstsc could
//! be pointed at: it's opened with RdpBase's named-pipe connector, and the connection it makes is
//! handed to Windows' Remote Desktop control, which draws the screen in a window and sends it the
//! input. This is ported from appsandbox's `src/backend_win/vm_display.c` (MIT, see THIRD-PARTY.md):
//! the same objects, made in the same order, with the same settings, waits and rebuilds.
//!
//! The connector and its context are RdpBase's own and aren't documented, so their methods are
//! declared here by hand, as appsandbox declares them. The control is reached by name, through its
//! `IDispatch`, as appsandbox does, so none of its typed interfaces are needed.
//!
//! Everything is made, used and let go of on one thread of the console's own, which runs the
//! window's messages: the control and the connector's context are single-threaded, and their
//! callbacks come through that thread's messages. Only the window's handle and a flag cross over.
//! This file needs nothing of the app's, so the example that tests it can take it as it is.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    E_NOINTERFACE, E_NOTIMPL, ERROR_FILE_NOT_FOUND, ERROR_SEM_TIMEOUT, GetLastError, HWND, LPARAM, LRESULT,
    RECT, RPC_E_CHANGED_MODE, S_OK, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH};
use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET,
    DISPATCH_PROPERTYPUT, DISPPARAMS, EXCEPINFO,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Ole::{DISPID_PROPERTYPUT, OleInitialize, OleUninitialize};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
use windows_sys::Win32::System::Variant::{
    VARIANT, VT_BSTR, VT_DISPATCH, VT_I2, VT_I4, VT_UNKNOWN, VariantClear,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW, GetWindowLongPtrW, IDC_ARROW, IsIconic,
    LoadCursorW, MSG, MoveWindow, PostMessageW, PostQuitMessage, RegisterClassExW, SW_RESTORE,
    SW_SHOWNOACTIVATE, SW_SHOWNORMAL, SetForegroundWindow, SetTimer, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, WM_APP, WM_CLOSE, WM_CREATE, WM_DESTROY, WM_SIZE, WM_TIMER, WNDCLASSEXW, WS_CHILD,
    WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows_sys::core::{BSTR, GUID, HRESULT};

/// What the console says as it goes. They come on the console's thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The pipe isn't there yet: the machine isn't running, or its worker hasn't made the pipe.
    Waiting,
    /// The pipe answered, and the control is starting its session on it.
    Connecting,
    /// The control's session is up, and the guest's screen shows.
    Connected,
    /// The control's session ended. The numbers are its reason and its extended reason, and the
    /// words are the control's own for them.
    Disconnected { reason: i32, extended: i32, words: Option<String> },
    /// Something went wrong, in words.
    Problem(String),
    /// What's going on, as a line for a log.
    Note(String),
    /// The window has gone, and the thread with it.
    Closed,
}

/// The console's window, open on its own thread until it's closed.
pub struct Console {
    shared: Arc<Shared>,
}

/// What the app's thread and the console's share.
struct Shared {
    /// The console's window, once it's made.
    hwnd: AtomicIsize,
    /// The app wants it closed, which the console's thread checks for until it has a window.
    closing: AtomicBool,
}

impl Console {
    /// Opens a window titled `title` showing the screen served on `pipe`. `quiet` shows it without
    /// making it the active window, for a test that mustn't take the keyboard. `events` hears how it
    /// goes.
    pub fn open(
        pipe: &str,
        title: &str,
        quiet: bool,
        events: impl Fn(Event) + Send + 'static,
    ) -> std::io::Result<Console> {
        let shared = Arc::new(Shared { hwnd: AtomicIsize::new(0), closing: AtomicBool::new(false) });
        let theirs = Arc::clone(&shared);
        let pipe = wide(pipe);
        let title = wide(title);
        thread::Builder::new()
            .name("console".into())
            .spawn(move || run(theirs, pipe, title, quiet, Box::new(events)))?;
        Ok(Console { shared })
    }

    /// The console's window, or 0 until it's made.
    pub fn hwnd(&self) -> isize {
        self.shared.hwnd.load(Ordering::Acquire)
    }

    /// Makes the control and the connector afresh and connects again, as a control can't be used
    /// again once its session has ended.
    pub fn reconnect(&self) {
        self.post(RECONNECT);
    }

    /// Ends the session, for a machine that's stopping, so the console doesn't hold up its
    /// shutdown. The window stays until it's closed.
    pub fn disconnect(&self) {
        self.post(DISCONNECT);
    }

    /// Brings the window to the front, as it's asked for again when it's already open.
    pub fn front(&self) {
        self.post(FRONT);
    }

    /// Closes the window, as its close button does. The machine goes on as it is.
    pub fn close(&self) {
        self.shared.closing.store(true, Ordering::Release);
        self.post(WM_CLOSE);
    }

    fn post(&self, message: u32) {
        let hwnd = self.hwnd();
        if hwnd != 0 {
            // SAFETY: posting takes no pointers. A handle whose window has gone fails harmlessly.
            unsafe {
                PostMessageW(hwnd as HWND, message, 0, 0);
            }
        }
    }
}

impl Drop for Console {
    /// Closes the window without waiting for it, as the window's teardown can take a moment and the
    /// app's thread mustn't wait on it.
    fn drop(&mut self) {
        self.close();
    }
}

// ---- What RdpBase and the control are known by ----

/// Windows' Remote Desktop control, version 10, in the form that isn't marked safe for scripting,
/// as only that one takes a connection made elsewhere.
const CONTROL: GUID = GUID::from_u128(0xA0C63C30_F08D_4AB4_907C_34905D770C7D);
/// RdpBase's single-threaded context, which its connector is made in.
const STA_CONTEXT: GUID = GUID::from_u128(0xFB332AE7_0055_4208_92B7_20410CA8382B);
/// The context's interface: properties, then starting and ending it.
const PLATFORM_CONTEXT: GUID = GUID::from_u128(0xFB332AE7_000E_4208_92B7_20410CA8382B);
/// RdpBase's connector for a named pipe, and its interface.
const PIPE_CONNECTOR: GUID = GUID::from_u128(0xFB332AE7_0088_4208_92B7_20410CA8382B);
const PIPE_CONNECTOR_INTERFACE: GUID = GUID::from_u128(0x4ACF942D_EADC_45BF_8EA8_793FE3CE31E8);
/// What the connector calls back on: connected, or failed.
const PIPE_CALLBACKS: GUID = GUID::from_u128(0xD923EFE9_0A6D_4344_92B3_164229DB8D2D);
/// The control's events, as a dispatch interface.
const CONTROL_EVENTS: GUID = GUID::from_u128(0x336D5562_EFA8_482E_8CB3_C5C0FC7A7DB6);

const IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_C000_000000000046);
const IDISPATCH: GUID = GUID::from_u128(0x00020400_0000_0000_C000_000000000046);
const ICONNECTION_POINT_CONTAINER: GUID = GUID::from_u128(0xB196B284_BAB4_101A_B69C_00AA00341D07);
const NO_GUID: GUID = GUID::from_u128(0);
/// The user's locale, which names are looked up in.
const USER_LOCALE: u32 = 0x0400;

/// The control's events, by their dispatch IDs, as its type library numbers them. appsandbox's
/// numbers differ, and took a disconnect for the start of connecting.
const ON_CONNECTING: i32 = 1;
const ON_CONNECTED: i32 = 2;
const ON_LOGIN_COMPLETE: i32 = 3;
const ON_DISCONNECTED: i32 = 4;
const ON_FATAL_ERROR: i32 = 10;
const ON_LOGON_ERROR: i32 = 22;

/// The connector's errors for a pipe that isn't there, and one this user may not open: Windows'
/// file-not-found and access-denied, as HRESULTs.
const PIPE_MISSING: HRESULT = 0x8007_0002_u32 as HRESULT;
const PIPE_REFUSED: HRESULT = 0x8007_0005_u32 as HRESULT;

/// The control's `Connected`: whether its session is up.
const NOT_CONNECTED: i32 = 0;

// ---- The interfaces, as their method tables ----

type Unknown = *mut c_void;

#[repr(C)]
struct UnknownMethods {
    query_interface: unsafe extern "system" fn(Unknown, *const GUID, *mut Unknown) -> HRESULT,
    add_ref: unsafe extern "system" fn(Unknown) -> u32,
    release: unsafe extern "system" fn(Unknown) -> u32,
}

#[repr(C)]
struct DispatchMethods {
    unknown: UnknownMethods,
    get_type_info_count: unsafe extern "system" fn(Unknown, *mut u32) -> HRESULT,
    get_type_info: unsafe extern "system" fn(Unknown, u32, u32, *mut Unknown) -> HRESULT,
    get_ids_of_names:
        unsafe extern "system" fn(Unknown, *const GUID, *const *const u16, u32, u32, *mut i32) -> HRESULT,
    invoke: unsafe extern "system" fn(
        Unknown,
        i32,
        *const GUID,
        u32,
        DISPATCH_FLAGS,
        *mut DISPPARAMS,
        *mut VARIANT,
        *mut EXCEPINFO,
        *mut u32,
    ) -> HRESULT,
}

#[repr(C)]
struct ConnectionPointContainerMethods {
    unknown: UnknownMethods,
    enum_connection_points: usize,
    find_connection_point: unsafe extern "system" fn(Unknown, *const GUID, *mut Unknown) -> HRESULT,
}

#[repr(C)]
struct ConnectionPointMethods {
    unknown: UnknownMethods,
    get_connection_interface: usize,
    get_connection_point_container: usize,
    advise: unsafe extern "system" fn(Unknown, Unknown, *mut u32) -> HRESULT,
    unadvise: unsafe extern "system" fn(Unknown, u32) -> HRESULT,
    enum_connections: usize,
}

/// RdpBase's context. Only starting and ending it are called; the property methods before them
/// hold their places.
#[repr(C)]
struct PlatformMethods {
    unknown: UnknownMethods,
    properties: [usize; PLATFORM_PROPERTY_METHODS],
    initialize_instance: unsafe extern "system" fn(Unknown) -> HRESULT,
    terminate_instance: unsafe extern "system" fn(Unknown) -> HRESULT,
}

/// The context's methods for its properties: get one as a variant, a bool, a number and an
/// interface, set each of those, and set one only if it isn't there.
const PLATFORM_PROPERTY_METHODS: usize = 9;

#[repr(C)]
struct ConnectorMethods {
    unknown: UnknownMethods,
    initialize_instance: unsafe extern "system" fn(Unknown, *mut PipeCallbacks) -> HRESULT,
    terminate_instance: unsafe extern "system" fn(Unknown) -> HRESULT,
    start_connect: unsafe extern "system" fn(Unknown, *const u16) -> HRESULT,
}

#[repr(C)]
struct CallbackMethods {
    query_interface: unsafe extern "system" fn(*mut PipeCallbacks, *const GUID, *mut Unknown) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut PipeCallbacks) -> u32,
    release: unsafe extern "system" fn(*mut PipeCallbacks) -> u32,
    on_connection_completed: unsafe extern "system" fn(*mut PipeCallbacks, Unknown),
    on_connector_error: unsafe extern "system" fn(*mut PipeCallbacks, HRESULT),
}

/// The method table of the object `object` points to.
///
/// # Safety
/// `object` is a live COM object whose table starts with `T`'s methods.
unsafe fn methods<'a, T>(object: Unknown) -> &'a T {
    // SAFETY: a COM object's first field points to its table, as the caller promises.
    unsafe { &**(object as *const *const T) }
}

/// # Safety
/// `object` is null, or a live COM object that this code holds a reference on.
unsafe fn release(object: Unknown) {
    if !object.is_null() {
        // SAFETY: as the caller promises, and every COM table starts with IUnknown's.
        unsafe { (methods::<UnknownMethods>(object).release)(object) };
    }
}

/// # Safety
/// `object` is a live COM object.
unsafe fn query(object: Unknown, interface: &GUID) -> Result<Unknown, HRESULT> {
    let mut out = null_mut();
    // SAFETY: as the caller promises; `out` is written by the call.
    let hr = unsafe { (methods::<UnknownMethods>(object).query_interface)(object, interface, &mut out) };
    if hr < 0 || out.is_null() { Err(hr) } else { Ok(out) }
}

// ---- Calls by name on the control ----

/// # Safety
/// `object` is a live `IDispatch`.
unsafe fn dispatch_id(object: Unknown, name: &str) -> Result<i32, HRESULT> {
    let name = wide(name);
    let names = [name.as_ptr()];
    let mut id = 0;
    // SAFETY: as the caller promises; one name, ending in a null, and one ID to write.
    let hr = unsafe {
        (methods::<DispatchMethods>(object).get_ids_of_names)(
            object,
            &NO_GUID,
            names.as_ptr(),
            1,
            USER_LOCALE,
            &mut id,
        )
    };
    if hr < 0 { Err(hr) } else { Ok(id) }
}

/// # Safety
/// `object` is a live `IDispatch`.
unsafe fn invoke(
    object: Unknown,
    name: &str,
    flags: DISPATCH_FLAGS,
    params: &mut DISPPARAMS,
    result: *mut VARIANT,
) -> HRESULT {
    // SAFETY: as the caller promises. `params` points to what it describes for the call's length.
    unsafe {
        match dispatch_id(object, name) {
            Ok(id) => (methods::<DispatchMethods>(object).invoke)(
                object,
                id,
                &NO_GUID,
                USER_LOCALE,
                flags,
                params,
                result,
                null_mut(),
                null_mut(),
            ),
            Err(hr) => hr,
        }
    }
}

/// A property of `object`, which the caller clears.
///
/// # Safety
/// `object` is a live `IDispatch`.
unsafe fn get(object: Unknown, name: &str) -> Result<VARIANT, HRESULT> {
    let mut result = VARIANT::default();
    let mut params = DISPPARAMS::default();
    // SAFETY: as the caller promises; no arguments, and a result to fill.
    let hr = unsafe { invoke(object, name, DISPATCH_PROPERTYGET, &mut params, &mut result) };
    if hr < 0 { Err(hr) } else { Ok(result) }
}

/// # Safety
/// `object` is a live `IDispatch`, and `value` holds what the property takes.
unsafe fn put(object: Unknown, name: &str, value: &mut VARIANT) -> HRESULT {
    let mut named = DISPID_PROPERTYPUT;
    let mut params = DISPPARAMS { rgvarg: value, rgdispidNamedArgs: &mut named, cArgs: 1, cNamedArgs: 1 };
    // SAFETY: as the caller promises; one argument, named as the property's new value.
    unsafe { invoke(object, name, DISPATCH_PROPERTYPUT, &mut params, null_mut()) }
}

/// # Safety
/// `object` is a live `IDispatch`.
unsafe fn put_long(object: Unknown, name: &str, value: i32) -> HRESULT {
    let mut variant = VARIANT::default();
    variant.Anonymous.Anonymous.vt = VT_I4;
    variant.Anonymous.Anonymous.Anonymous.lVal = value;
    // SAFETY: as the caller promises.
    unsafe { put(object, name, &mut variant) }
}

/// # Safety
/// `object` is a live `IDispatch`.
unsafe fn call(object: Unknown, name: &str) -> HRESULT {
    let mut params = DISPPARAMS::default();
    // SAFETY: as the caller promises; no arguments and no result.
    unsafe { invoke(object, name, DISPATCH_METHOD, &mut params, null_mut()) }
}

/// A whole-number property, or -1 when it can't be read.
///
/// # Safety
/// `object` is a live `IDispatch`.
unsafe fn get_number(object: Unknown, name: &str) -> i32 {
    // SAFETY: as the caller promises; the variant is read by its type, and cleared.
    unsafe {
        let Ok(mut value) = get(object, name) else { return -1 };
        let inner = value.Anonymous.Anonymous;
        let number = match inner.vt {
            VT_I4 => inner.Anonymous.lVal,
            VT_I2 => i32::from(inner.Anonymous.iVal),
            _ => -1,
        };
        VariantClear(&mut value);
        number
    }
}

/// A text property, or `None` when it can't be read or is empty.
///
/// # Safety
/// `object` is a live `IDispatch`.
unsafe fn get_text(object: Unknown, name: &str) -> Option<String> {
    // SAFETY: as the caller promises; a BSTR is read up to its null, and the variant cleared.
    unsafe {
        let mut value = get(object, name).ok()?;
        let inner = value.Anonymous.Anonymous;
        let text = (inner.vt == VT_BSTR && !inner.Anonymous.bstrVal.is_null())
            .then(|| from_bstr(inner.Anonymous.bstrVal));
        VariantClear(&mut value);
        text.filter(|text| !text.is_empty())
    }
}

/// # Safety
/// `text` is a live BSTR.
unsafe fn from_bstr(text: BSTR) -> String {
    let mut length = 0;
    // SAFETY: a BSTR ends in a null.
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(text, length))
    }
}

// ---- RdpBase and ATL, loaded when first needed ----

type CreateInstance = unsafe extern "system" fn(Unknown, *const GUID, *const GUID, *mut Unknown) -> HRESULT;
/// An export as `GetProcAddress` gives it, before it's cast to its own signature.
type Export = unsafe extern "system" fn() -> isize;
type AxWinInit = unsafe extern "system" fn() -> i32;
type AxAttachControl = unsafe extern "system" fn(Unknown, HWND, *mut Unknown) -> HRESULT;

struct Libraries {
    create_instance: CreateInstance,
    attach_control: AxAttachControl,
}

/// RdpBase's maker of its objects, and ATL's host for a control in a window, which only atl.dll
/// gives without ATL's headers. Words for the person when they can't be loaded.
fn libraries() -> Result<&'static Libraries, String> {
    static LOADED: OnceLock<Result<Libraries, String>> = OnceLock::new();
    LOADED.get_or_init(load).as_ref().map_err(String::clone)
}

fn load() -> Result<Libraries, String> {
    // SAFETY: both are Windows' own libraries, loaded for the life of the process. Each export is
    // cast to the signature it has, as appsandbox declares it, and checked for null first.
    unsafe {
        let rdpbase = LoadLibraryW(wide("RdpBase.dll").as_ptr());
        if rdpbase.is_null() {
            return Err(format!("Windows' RdpBase.dll couldn't be loaded (error {}).", GetLastError()));
        }
        let create = GetProcAddress(rdpbase, c"RDPBASE_CreateInstance".as_ptr().cast())
            .ok_or("RdpBase.dll has no RDPBASE_CreateInstance, so this Windows can't show a console.")?;
        let atl = LoadLibraryW(wide("atl.dll").as_ptr());
        if atl.is_null() {
            return Err(format!("Windows' atl.dll couldn't be loaded (error {}).", GetLastError()));
        }
        let init = GetProcAddress(atl, c"AtlAxWinInit".as_ptr().cast());
        let attach = GetProcAddress(atl, c"AtlAxAttachControl".as_ptr().cast());
        let (Some(init), Some(attach)) = (init, attach) else {
            return Err("atl.dll has no AtlAxWinInit or AtlAxAttachControl.".to_owned());
        };
        let init = std::mem::transmute::<Export, AxWinInit>(init);
        init();
        Ok(Libraries {
            create_instance: std::mem::transmute::<Export, CreateInstance>(create),
            attach_control: std::mem::transmute::<Export, AxAttachControl>(attach),
        })
    }
}

// ---- The console's state, on its thread ----

/// Messages to the console's window from the app's thread.
const RECONNECT: u32 = WM_APP + 1;
const DISCONNECT: u32 = WM_APP + 2;
const FRONT: u32 = WM_APP + 3;

/// The timer that asks the control whether it's connected, and how often.
const POLL_TIMER: usize = 1;
const POLL_EVERY_MS: u32 = 1000;
/// How long the connector has to answer before the console says it hasn't.
const ANSWER_WITHIN: Duration = Duration::from_secs(10);
/// How often, and how many times, the pipe is looked for before connecting: the worker makes it a
/// moment after the machine starts.
const PIPE_TRIES: u32 = 50;
const PIPE_APART: Duration = Duration::from_millis(100);
/// The window's first size, in pixels.
const WIDTH: i32 = 1280;
const HEIGHT: i32 = 860;

/// Everything the console holds, owned by its thread. Each field is a `Cell`, as the control and the
/// connector call back into it from inside calls made from it.
struct Display {
    shared: Arc<Shared>,
    pipe: Vec<u16>,
    events: Box<dyn Fn(Event) + Send>,
    hwnd: Cell<HWND>,
    /// The child window the control is hosted in.
    host: Cell<HWND>,
    control: Cell<Unknown>,
    dispatch: Cell<Unknown>,
    container: Cell<Unknown>,
    platform: Cell<Unknown>,
    connector: Cell<Unknown>,
    callbacks: Cell<*mut PipeCallbacks>,
    sink: Cell<*mut EventSink>,
    cookie: Cell<u32>,
    last_connected: Cell<i32>,
    /// Whether the connector has called back since the last connect.
    answered: Cell<bool>,
    connect_started: Cell<Option<Instant>>,
    silence_told: Cell<bool>,
}

impl Display {
    fn tell(&self, event: Event) {
        (self.events)(event);
    }
}

/// The object the connector calls back on.
#[repr(C)]
struct PipeCallbacks {
    methods: *const CallbackMethods,
    references: AtomicU32,
    /// Null once the console has let go of it, in case RdpBase holds on to it longer.
    display: Cell<*const Display>,
}

static CALLBACK_METHODS: CallbackMethods = CallbackMethods {
    query_interface: callbacks_query,
    add_ref: callbacks_add_ref,
    release: callbacks_release,
    on_connection_completed,
    on_connector_error,
};

/// The object the control sends its events to.
#[repr(C)]
struct EventSink {
    methods: *const DispatchMethods,
    references: AtomicU32,
    display: Cell<*const Display>,
}

static SINK_METHODS: DispatchMethods = DispatchMethods {
    unknown: UnknownMethods { query_interface: sink_query, add_ref: sink_add_ref, release: sink_release },
    get_type_info_count: sink_type_info_count,
    get_type_info: sink_type_info,
    get_ids_of_names: sink_ids_of_names,
    invoke: sink_invoke,
};

unsafe extern "system" fn callbacks_query(
    this: *mut PipeCallbacks,
    iid: *const GUID,
    out: *mut Unknown,
) -> HRESULT {
    // SAFETY: COM passes a live object, an interface ID and somewhere to write.
    unsafe {
        if same(&*iid, &IUNKNOWN) || same(&*iid, &PIPE_CALLBACKS) {
            *out = this.cast();
            callbacks_add_ref(this);
            S_OK
        } else {
            *out = null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn callbacks_add_ref(this: *mut PipeCallbacks) -> u32 {
    // SAFETY: COM passes a live object.
    unsafe { (*this).references.fetch_add(1, Ordering::AcqRel) + 1 }
}

unsafe extern "system" fn callbacks_release(this: *mut PipeCallbacks) -> u32 {
    // SAFETY: COM passes a live object, made by `Box::into_raw`, freed when the last reference goes.
    unsafe {
        let left = (*this).references.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            drop(Box::from_raw(this));
        }
        left
    }
}

/// The connector has opened the pipe: hand its connection to the control, and connect.
unsafe extern "system" fn on_connection_completed(this: *mut PipeCallbacks, stream: Unknown) {
    // SAFETY: the connector passes this object, live, and its connection, which is put in the
    // control's settings without a reference of ours, as appsandbox does, and so isn't cleared.
    // The display is the thread's own, alive while its pointer is set.
    unsafe {
        let Some(display) = (*this).display.get().as_ref() else { return };
        display.answered.set(true);
        let dispatch = display.dispatch.get();
        if dispatch.is_null() {
            display.tell(Event::Problem("The pipe answered, but the console's control wasn't made.".into()));
            return;
        }
        display.tell(Event::Connecting);
        let settings = match get(dispatch, "AdvancedSettings9")
            .or_else(|_| get(dispatch, "AdvancedSettings8"))
        {
            Ok(settings) => settings,
            Err(hr) => {
                display
                    .tell(Event::Problem(format!("The control's settings couldn't be had ({}).", hex(hr))));
                return;
            }
        };
        let inner = settings.Anonymous.Anonymous;
        let advanced = match inner.vt {
            VT_DISPATCH if !inner.Anonymous.pdispVal.is_null() => inner.Anonymous.pdispVal,
            VT_UNKNOWN if !inner.Anonymous.punkVal.is_null() => {
                let found = query(inner.Anonymous.punkVal, &IDISPATCH);
                release(inner.Anonymous.punkVal);
                match found {
                    Ok(advanced) => advanced,
                    Err(hr) => {
                        display.tell(Event::Problem(format!(
                            "The control's settings can't be set ({}).",
                            hex(hr)
                        )));
                        return;
                    }
                }
            }
            other => {
                let mut settings = settings;
                VariantClear(&mut settings);
                display.tell(Event::Problem(format!("The control's settings came as type {other}.")));
                return;
            }
        };
        let mut endpoint = VARIANT::default();
        endpoint.Anonymous.Anonymous.vt = VT_UNKNOWN;
        endpoint.Anonymous.Anonymous.Anonymous.punkVal = stream;
        let hr = put(advanced, "ConnectWithEndpoint", &mut endpoint);
        if hr < 0 {
            release(advanced);
            display.tell(Event::Problem(format!(
                "The control didn't take the pipe's connection ({}).",
                hex(hr)
            )));
            return;
        }
        // Not in appsandbox: the guest's screen is scaled to the window, so it follows the window's
        // size rather than showing a corner of it with scroll bars.
        put_bool(advanced, "SmartSizing", true);
        release(advanced);
        let hr = call(dispatch, "Connect");
        if hr < 0 {
            display.tell(Event::Problem(format!("The control couldn't connect ({}).", hex(hr))));
        } else if let Some(status) = get_text(dispatch, "ConnectedStatusText") {
            display.tell(Event::Note(format!("The control says: {status}")));
        }
    }
}

/// The connector couldn't open the pipe, or its connection failed.
unsafe extern "system" fn on_connector_error(this: *mut PipeCallbacks, error: HRESULT) {
    // SAFETY: the connector passes this object, live. The display is the thread's own.
    unsafe {
        let Some(display) = (*this).display.get().as_ref() else { return };
        display.answered.set(true);
        if error == PIPE_MISSING {
            let words = "The machine's screen isn't being served: its console's pipe isn't there. It's served \
                         while the machine runs, so Reconnect tries again once it's running.";
            return display.tell(Event::Problem(words.to_owned()));
        }
        if error == PIPE_REFUSED {
            let words = "Windows refused the console's pipe to this user. It's served to the user signed in \
                         at the PC when the machine started, and to administrators.";
            return display.tell(Event::Problem(words.to_owned()));
        }
        let mut words = format!("The console's pipe failed ({}).", hex(error));
        let dispatch = display.dispatch.get();
        if !dispatch.is_null() {
            words.push_str(&format!(
                " The control's state is {}, and its extended reason {}.",
                get_number(dispatch, "Connected"),
                get_number(dispatch, "ExtendedDisconnectReason")
            ));
        }
        display.tell(Event::Problem(words));
    }
}

/// # Safety
/// `object` is a live `IDispatch`.
unsafe fn put_bool(object: Unknown, name: &str, value: bool) -> HRESULT {
    /// OLE's true and false, as a variant's bool holds them.
    const TRUE: i16 = -1;
    const FALSE: i16 = 0;
    let mut variant = VARIANT::default();
    variant.Anonymous.Anonymous.vt = windows_sys::Win32::System::Variant::VT_BOOL;
    variant.Anonymous.Anonymous.Anonymous.boolVal = if value { TRUE } else { FALSE };
    // SAFETY: as the caller promises.
    unsafe { put(object, name, &mut variant) }
}

unsafe extern "system" fn sink_query(this: Unknown, iid: *const GUID, out: *mut Unknown) -> HRESULT {
    // SAFETY: COM passes a live object, an interface ID and somewhere to write.
    unsafe {
        let iid = &*iid;
        if same(iid, &IUNKNOWN) || same(iid, &IDISPATCH) || same(iid, &CONTROL_EVENTS) {
            *out = this;
            sink_add_ref(this);
            S_OK
        } else {
            *out = null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn sink_add_ref(this: Unknown) -> u32 {
    // SAFETY: COM passes a live sink.
    unsafe { (*(this as *mut EventSink)).references.fetch_add(1, Ordering::AcqRel) + 1 }
}

unsafe extern "system" fn sink_release(this: Unknown) -> u32 {
    // SAFETY: COM passes a live sink, made by `Box::into_raw`, freed when the last reference goes.
    unsafe {
        let sink = this as *mut EventSink;
        let left = (*sink).references.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            drop(Box::from_raw(sink));
        }
        left
    }
}

unsafe extern "system" fn sink_type_info_count(_: Unknown, count: *mut u32) -> HRESULT {
    // SAFETY: COM passes somewhere to write.
    unsafe { *count = 0 };
    S_OK
}

unsafe extern "system" fn sink_type_info(_: Unknown, _: u32, _: u32, out: *mut Unknown) -> HRESULT {
    // SAFETY: COM passes somewhere to write.
    unsafe { *out = null_mut() };
    E_NOTIMPL
}

unsafe extern "system" fn sink_ids_of_names(
    _: Unknown,
    _: *const GUID,
    _: *const *const u16,
    _: u32,
    _: u32,
    _: *mut i32,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn sink_invoke(
    this: Unknown,
    id: i32,
    _: *const GUID,
    _: u32,
    _: DISPATCH_FLAGS,
    params: *mut DISPPARAMS,
    _: *mut VARIANT,
    _: *mut EXCEPINFO,
    _: *mut u32,
) -> HRESULT {
    // SAFETY: the control passes this sink, live, and its event's arguments, which are only read.
    // The display is the thread's own, alive while its pointer is set.
    unsafe {
        let Some(display) = (*(this as *mut EventSink)).display.get().as_ref() else { return S_OK };
        let first = || -> i32 {
            match params.as_ref() {
                Some(params) if params.cArgs >= 1 => {
                    let inner = (*params.rgvarg).Anonymous.Anonymous;
                    if inner.vt == VT_I4 { inner.Anonymous.lVal } else { 0 }
                }
                _ => 0,
            }
        };
        match id {
            ON_CONNECTING => display.tell(Event::Note("The control is connecting.".into())),
            ON_CONNECTED => {
                display.tell(Event::Connected);
                let dispatch = display.dispatch.get();
                if !dispatch.is_null() {
                    display.tell(Event::Note(format!(
                        "The guest's screen is {} by {}.",
                        get_number(dispatch, "DesktopWidth"),
                        get_number(dispatch, "DesktopHeight")
                    )));
                }
            }
            ON_DISCONNECTED => {
                let dispatch = display.dispatch.get();
                let reason = first();
                let (extended, words) = if dispatch.is_null() {
                    (-1, None)
                } else {
                    let extended = get_number(dispatch, "ExtendedDisconnectReason");
                    (extended, describe(dispatch, reason, extended))
                };
                display.tell(Event::Disconnected { reason, extended, words });
            }
            ON_LOGIN_COMPLETE => display.tell(Event::Note("The control's sign-in is complete.".into())),
            ON_LOGON_ERROR => {
                display.tell(Event::Note(format!("The control's sign-in failed ({}).", first())))
            }
            ON_FATAL_ERROR => {
                display.tell(Event::Problem(format!("The Remote Desktop control failed ({}).", first())))
            }
            _ => {}
        }
        S_OK
    }
}

/// The control's own words for why its session ended.
///
/// # Safety
/// `dispatch` is the control's live `IDispatch`.
unsafe fn describe(dispatch: Unknown, reason: i32, extended: i32) -> Option<String> {
    let number = |value: i32| {
        let mut variant = VARIANT::default();
        variant.Anonymous.Anonymous.vt = VT_I4;
        variant.Anonymous.Anonymous.Anonymous.lVal = value;
        variant
    };
    // A call's arguments go last first.
    let mut arguments = [number(extended), number(reason)];
    let mut params = DISPPARAMS {
        rgvarg: arguments.as_mut_ptr(),
        rgdispidNamedArgs: null_mut(),
        cArgs: arguments.len() as u32,
        cNamedArgs: 0,
    };
    let mut result = VARIANT::default();
    // SAFETY: as the caller promises; two whole numbers in, a string out, read and cleared.
    unsafe {
        if invoke(dispatch, "GetErrorDescription", DISPATCH_METHOD, &mut params, &mut result) < 0 {
            return None;
        }
        let inner = result.Anonymous.Anonymous;
        let words = (inner.vt == VT_BSTR && !inner.Anonymous.bstrVal.is_null())
            .then(|| from_bstr(inner.Anonymous.bstrVal));
        VariantClear(&mut result);
        words.filter(|words| !words.is_empty())
    }
}

fn same(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn hex(hr: HRESULT) -> String {
    format!("0x{:08X}", hr as u32)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

// ---- The thread ----

const CLASS: &str = "Guesthouse.Console";

fn run(shared: Arc<Shared>, pipe: Vec<u16>, title: Vec<u16>, quiet: bool, events: Box<dyn Fn(Event) + Send>) {
    // SAFETY: OLE is started for this thread, which the control needs, and ended when it's done.
    let ole = unsafe { OleInitialize(null()) };
    if ole < 0 && ole != RPC_E_CHANGED_MODE {
        events(Event::Problem(format!("OLE couldn't start for the console ({}).", hex(ole))));
        events(Event::Closed);
        return;
    }
    let display = Box::new(Display {
        shared,
        pipe,
        events,
        hwnd: Cell::new(null_mut()),
        host: Cell::new(null_mut()),
        control: Cell::new(null_mut()),
        dispatch: Cell::new(null_mut()),
        container: Cell::new(null_mut()),
        platform: Cell::new(null_mut()),
        connector: Cell::new(null_mut()),
        callbacks: Cell::new(null_mut()),
        sink: Cell::new(null_mut()),
        cookie: Cell::new(0),
        last_connected: Cell::new(NOT_CONNECTED),
        answered: Cell::new(false),
        connect_started: Cell::new(None),
        silence_told: Cell::new(false),
    });
    // SAFETY: the display outlives its window, whose messages all come on this thread, inside the
    // loop below, which ends once the window is destroyed.
    unsafe { show(&display, &title, quiet) };
    display.tell(Event::Closed);
    drop(display);
    if ole >= 0 {
        // SAFETY: matches the start above, on the same thread.
        unsafe { OleUninitialize() };
    }
}

/// Makes the window, connects, and runs its messages until it's gone.
///
/// # Safety
/// `display` outlives the window, and this runs on the thread that started OLE.
unsafe fn show(display: &Display, title: &[u16], quiet: bool) {
    let libraries = match libraries() {
        Ok(libraries) => libraries,
        Err(words) => return display.tell(Event::Problem(words)),
    };
    if !wait_for_pipe(display) {
        return;
    }
    // SAFETY: the class's procedure is `window_procedure`, and the window is given the display as
    // its creation parameter, which the procedure keeps for its later messages.
    unsafe {
        let instance = GetModuleHandleW(null());
        static REGISTERED: OnceLock<()> = OnceLock::new();
        let class = wide(CLASS);
        REGISTERED.get_or_init(|| {
            let window_class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_procedure),
                hInstance: instance,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: GetStockObject(BLACK_BRUSH) as HBRUSH,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            RegisterClassExW(&window_class);
        });
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WIDTH,
            HEIGHT,
            null_mut(),
            null_mut(),
            instance,
            (display as *const Display).cast(),
        );
        if hwnd.is_null() {
            return display.tell(Event::Problem(format!(
                "The console's window couldn't be made (error {}).",
                GetLastError()
            )));
        }
        display.hwnd.set(hwnd);
        ShowWindow(hwnd, if quiet { SW_SHOWNOACTIVATE } else { SW_SHOWNORMAL });
        display.shared.hwnd.store(hwnd as isize, Ordering::Release);
        // The app may have asked for it closed before there was a window to tell.
        if display.shared.closing.load(Ordering::Acquire) {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
        connect(display, libraries, false);
        SetTimer(hwnd, POLL_TIMER, POLL_EVERY_MS, None);
        let mut message = MSG::default();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        display.shared.hwnd.store(0, Ordering::Release);
    }
}

/// Looks for the pipe a few times before connecting, as the worker makes it a moment after the
/// machine starts. Returns `false` if the app asked for the console closed meanwhile.
fn wait_for_pipe(display: &Display) -> bool {
    for n in 0..PIPE_TRIES {
        if display.shared.closing.load(Ordering::Acquire) {
            return false;
        }
        // SAFETY: the name ends in a null. With no wait, the call only says whether the pipe is
        // there, without taking its connection.
        let (there, error) = unsafe { (WaitNamedPipeW(display.pipe.as_ptr(), 0) != 0, GetLastError()) };
        if there || (error != ERROR_FILE_NOT_FOUND && error != ERROR_SEM_TIMEOUT) {
            break;
        }
        if n == 0 {
            display.tell(Event::Waiting);
        }
        thread::sleep(PIPE_APART);
    }
    true
}

/// Makes the host window, the control, the connector's context and the connector, and starts
/// connecting. With `again`, everything from before is let go of first, as appsandbox's
/// `display_reconnect` does, since a control can't be used again.
///
/// # Safety
/// On the console's thread, with its window made.
unsafe fn connect(display: &Display, libraries: &Libraries, again: bool) {
    // SAFETY: every object made here is kept in the display and let go of in `tear_down` or
    // `close`; the callbacks and the sink point back to the display, which outlives them.
    unsafe {
        display.last_connected.set(NOT_CONNECTED);
        if again {
            unadvise(display);
            let dispatch = display.dispatch.get();
            if !dispatch.is_null() {
                call(dispatch, "Disconnect");
            }
            end_connector(display);
            release(display.dispatch.replace(null_mut()));
            release(display.container.replace(null_mut()));
            release(display.control.replace(null_mut()));
            end_platform(display);
            let host = display.host.replace(null_mut());
            if !host.is_null() {
                DestroyWindow(host);
            }
        }
        let hwnd = display.hwnd.get();
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);
        let host = CreateWindowExW(
            0,
            wide("Static").as_ptr(),
            wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            0,
            0,
            client.right,
            client.bottom,
            hwnd,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if host.is_null() {
            return display.tell(Event::Problem(format!(
                "The console's host window couldn't be made (error {}).",
                GetLastError()
            )));
        }
        display.host.set(host);

        let mut control = null_mut();
        let hr = CoCreateInstance(&CONTROL, null_mut(), CLSCTX_INPROC_SERVER, &IUNKNOWN, &mut control);
        if hr < 0 {
            return display.tell(Event::Problem(format!(
                "Windows' Remote Desktop control couldn't be made ({}).",
                hex(hr)
            )));
        }
        display.control.set(control);
        let mut container = null_mut();
        let hr = (libraries.attach_control)(control, host, &mut container);
        display.container.set(container);
        if hr < 0 {
            return display.tell(Event::Problem(format!(
                "The Remote Desktop control couldn't be put in the window ({}).",
                hex(hr)
            )));
        }
        let dispatch = match query(control, &IDISPATCH) {
            Ok(dispatch) => dispatch,
            Err(hr) => {
                return display.tell(Event::Problem(format!(
                    "The Remote Desktop control can't be called by name ({}).",
                    hex(hr)
                )));
            }
        };
        display.dispatch.set(dispatch);
        // The worker asks for no server authentication, as there's no server to check.
        let hr = put_long(dispatch, "AuthenticationLevel", 0);
        if hr < 0 {
            display.tell(Event::Note(format!(
                "The control didn't take its authentication level ({}).",
                hex(hr)
            )));
        }
        advise(display);

        let mut platform = null_mut();
        let hr = (libraries.create_instance)(null_mut(), &STA_CONTEXT, &PLATFORM_CONTEXT, &mut platform);
        if hr < 0 {
            return display
                .tell(Event::Problem(format!("RdpBase's context couldn't be made ({}).", hex(hr))));
        }
        display.platform.set(platform);
        let hr = (methods::<PlatformMethods>(platform).initialize_instance)(platform);
        if hr < 0 {
            return display.tell(Event::Problem(format!("RdpBase's context didn't start ({}).", hex(hr))));
        }

        let mut connector = null_mut();
        let hr =
            (libraries.create_instance)(platform, &PIPE_CONNECTOR, &PIPE_CONNECTOR_INTERFACE, &mut connector);
        if hr < 0 {
            return display
                .tell(Event::Problem(format!("RdpBase's pipe connector couldn't be made ({}).", hex(hr))));
        }
        display.connector.set(connector);
        let callbacks = Box::into_raw(Box::new(PipeCallbacks {
            methods: &CALLBACK_METHODS,
            references: AtomicU32::new(1),
            display: Cell::new(display as *const Display),
        }));
        display.callbacks.set(callbacks);
        let hr = (methods::<ConnectorMethods>(connector).initialize_instance)(connector, callbacks);
        if hr < 0 {
            return display
                .tell(Event::Problem(format!("RdpBase's pipe connector didn't start ({}).", hex(hr))));
        }

        if !again {
            // Only a look: with no wait it doesn't take the pipe's connection.
            WaitNamedPipeW(display.pipe.as_ptr(), 0);
        }
        display.answered.set(false);
        display.silence_told.set(false);
        display.connect_started.set(Some(Instant::now()));
        let hr = (methods::<ConnectorMethods>(connector).start_connect)(connector, display.pipe.as_ptr());
        if hr < 0 {
            display.tell(Event::Problem(format!("The console's pipe couldn't be opened ({}).", hex(hr))));
        } else {
            display.tell(Event::Note("Opening the console's pipe.".into()));
        }
    }
}

/// Has the control send its events to a sink of ours.
///
/// # Safety
/// On the console's thread, with the control made.
unsafe fn advise(display: &Display) {
    // SAFETY: the container and the point are released here once used; the sink is ours, made with
    // one reference, which the display keeps until `unadvise`.
    unsafe {
        let control = display.control.get();
        let container = match query(control, &ICONNECTION_POINT_CONTAINER) {
            Ok(container) => container,
            Err(hr) => {
                return display
                    .tell(Event::Note(format!("The control's events can't be had ({}).", hex(hr))));
            }
        };
        let mut point = null_mut();
        let hr = (methods::<ConnectionPointContainerMethods>(container).find_connection_point)(
            container,
            &CONTROL_EVENTS,
            &mut point,
        );
        release(container);
        if hr < 0 || point.is_null() {
            return display.tell(Event::Note(format!("The control's events can't be found ({}).", hex(hr))));
        }
        let sink = Box::into_raw(Box::new(EventSink {
            methods: &SINK_METHODS,
            references: AtomicU32::new(1),
            display: Cell::new(display as *const Display),
        }));
        let mut cookie = 0;
        let hr = (methods::<ConnectionPointMethods>(point).advise)(point, sink.cast(), &mut cookie);
        release(point);
        if hr < 0 {
            sink_release(sink.cast());
            return display.tell(Event::Note(format!("The control's events can't be heard ({}).", hex(hr))));
        }
        display.sink.set(sink);
        display.cookie.set(cookie);
    }
}

/// # Safety
/// On the console's thread.
unsafe fn unadvise(display: &Display) {
    // SAFETY: the cookie is the one `advise` was given, and the sink's reference is the display's.
    unsafe {
        let control = display.control.get();
        let cookie = display.cookie.replace(0);
        if !control.is_null()
            && cookie != 0
            && let Ok(container) = query(control, &ICONNECTION_POINT_CONTAINER)
        {
            let mut point = null_mut();
            let found = (methods::<ConnectionPointContainerMethods>(container).find_connection_point)(
                container,
                &CONTROL_EVENTS,
                &mut point,
            );
            if found >= 0 && !point.is_null() {
                (methods::<ConnectionPointMethods>(point).unadvise)(point, cookie);
                release(point);
            }
            release(container);
        }
        let sink = display.sink.replace(null_mut());
        if !sink.is_null() {
            (*sink).display.set(null());
            sink_release(sink.cast());
        }
    }
}

/// Ends the connector and lets go of it and its callbacks, which breaks the pipe, so the control
/// doesn't hang on a machine that's gone.
///
/// # Safety
/// On the console's thread.
unsafe fn end_connector(display: &Display) {
    // SAFETY: both are the display's own references.
    unsafe {
        let connector = display.connector.replace(null_mut());
        if !connector.is_null() {
            (methods::<ConnectorMethods>(connector).terminate_instance)(connector);
            release(connector);
        }
        let callbacks = display.callbacks.replace(null_mut());
        if !callbacks.is_null() {
            (*callbacks).display.set(null());
            callbacks_release(callbacks);
        }
    }
}

/// # Safety
/// On the console's thread.
unsafe fn end_platform(display: &Display) {
    // SAFETY: the display's own reference.
    unsafe {
        let platform = display.platform.replace(null_mut());
        if !platform.is_null() {
            (methods::<PlatformMethods>(platform).terminate_instance)(platform);
            release(platform);
        }
    }
}

/// Lets go of everything as the window closes. The control isn't told to disconnect, which can
/// hang: destroying its window ends it.
///
/// # Safety
/// On the console's thread.
unsafe fn close(display: &Display) {
    // SAFETY: each is the display's own, let go of once, in appsandbox's order.
    unsafe {
        unadvise(display);
        end_connector(display);
        let host = display.host.replace(null_mut());
        if !host.is_null() {
            DestroyWindow(host);
        }
        release(display.dispatch.replace(null_mut()));
        release(display.container.replace(null_mut()));
        release(display.control.replace(null_mut()));
        end_platform(display);
    }
}

/// Asks the control whether it's connected, says when that changes, and says if the connector
/// hasn't answered in time.
///
/// # Safety
/// On the console's thread.
unsafe fn poll(display: &Display) {
    let dispatch = display.dispatch.get();
    if dispatch.is_null() {
        return;
    }
    if !display.answered.get()
        && !display.silence_told.get()
        && display.connect_started.get().is_some_and(|started| started.elapsed() > ANSWER_WITHIN)
    {
        display.silence_told.set(true);
        // SAFETY: the name ends in a null, and with no wait the call only looks.
        let there = unsafe { WaitNamedPipeW(display.pipe.as_ptr(), 0) != 0 };
        display.tell(Event::Problem(if there {
            "The console's pipe is there, but hasn't answered.".to_owned()
        } else {
            "The console's pipe isn't there. Is the machine running?".to_owned()
        }));
    }
    // SAFETY: the display's own `IDispatch`.
    let state = unsafe { get_number(dispatch, "Connected") };
    let was = display.last_connected.replace(state);
    if state != was {
        display.tell(Event::Note(format!("The control's state went from {was} to {state}.")));
        if was > NOT_CONNECTED && state == NOT_CONNECTED {
            // SAFETY: as above.
            let extended = unsafe { get_number(dispatch, "ExtendedDisconnectReason") };
            display.tell(Event::Note(format!("Its extended reason is {extended}.")));
        }
    }
}

unsafe extern "system" fn window_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the display was given at creation and outlives the window; every message comes on the
    // console's thread.
    unsafe {
        if message == WM_CREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            return 0;
        }
        let Some(display) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Display).as_ref() else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        match message {
            WM_CLOSE => {
                close(display);
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                display.hwnd.set(null_mut());
                PostQuitMessage(0);
                0
            }
            WM_SIZE => {
                let host = display.host.get();
                if !host.is_null() {
                    let mut client = RECT::default();
                    GetClientRect(hwnd, &mut client);
                    MoveWindow(host, 0, 0, client.right, client.bottom, 1);
                }
                0
            }
            WM_TIMER if wparam == POLL_TIMER => {
                poll(display);
                0
            }
            RECONNECT => {
                if let Ok(libraries) = libraries() {
                    connect(display, libraries, true);
                }
                0
            }
            FRONT => {
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                }
                SetForegroundWindow(hwnd);
                0
            }
            DISCONNECT => {
                end_connector(display);
                let dispatch = display.dispatch.get();
                if !dispatch.is_null() {
                    call(dispatch, "Disconnect");
                }
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}
