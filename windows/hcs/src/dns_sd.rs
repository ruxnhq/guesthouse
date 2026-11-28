//! DNS-SD through Windows' own mDNS responder (dnsapi.dll's `DnsServiceRegister`, Windows 10 1809
//! and later), so a service is announced without a second responder on port 5353 and without this
//! process reading what the network sends.
//!
//! What the responder does, as tried on Windows 11 as an ordinary user, which is all it needs:
//!
//! - A registration answers about 0.8 s later, through its callback, once the responder has probed
//!   for the name. Taking one back answers within milliseconds, through the same callback and with
//!   the context of the request that took it back.
//! - The records point at the computer's own host name. The responder answers for no other: an
//!   instance whose host is a name of its own is listed, but that name never resolves, so a viewer
//!   can't reach it. The address given with the instance isn't sent either, as each network gets
//!   the computer's own address on it, so none is given.
//! - It answers on every network the computer is on, Hyper-V's virtual switches too, each with the
//!   computer's address there.
//! - A name already taken, by this process, another one, or the computer's own instance, is not
//!   refused: the registration succeeds as `NAME(1)`, and the callback says so. Taking back goes by
//!   name alone, so taking back `NAME` then removes the other holder's instance, and the `NAME(1)`
//!   one outlives this process. Such a registration is taken back at once under the name it got,
//!   and reported.
//! - A name with a dot goes out as two labels, and a backslash escape isn't understood, so dots
//!   become dashes.
//! - Cancelling a registration that hasn't answered yet calls back with `ERROR_CANCELLED` before
//!   the cancel returns.
//!
//! The rule for what happens next, [`Lifecycle`], is plain code and is tested anywhere. Only the
//! calls are Windows'.

use std::fmt;

/// The domain mDNS answers in.
const LOCAL: &str = "local";

/// One service instance to announce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// The name browsers show. [`instance_label`] makes it one label.
    pub instance: String,
    /// The DNS-SD type, such as `_remoteman._udp`.
    pub kind: String,
    pub port: u16,
    /// The TXT record's keys and values.
    pub txt: Vec<(String, String)>,
}

impl Service {
    /// `NAME._type._proto.local`, the name Windows registers and browsers resolve.
    pub fn full_name(&self) -> String {
        format!("{}.{}.{LOCAL}", instance_label(&self.instance), self.kind)
    }
}

/// The name as one DNS label as Windows sends it: a dot would split it in two, so it becomes a dash,
/// as does a backslash, which Windows would send as it is.
pub fn instance_label(name: &str) -> String {
    name.chars().map(|c| if matches!(c, '.' | '\\') { '-' } else { c }).collect()
}

/// Why nothing is announced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Windows wouldn't take the registration, with its error code.
    Refused(u32),
    /// Windows took it, then said it failed, with its status.
    Failed(u32),
    /// Something on the network, or on this PC, already has the name. Windows would have announced
    /// it as `given` instead, which nobody would recognise, so it was taken back.
    Taken { wanted: String, given: String },
    /// Windows didn't say how the registration went in time, so it was cancelled.
    NoAnswer,
    /// The computer's host name, which the records point at, couldn't be read.
    NoHostName(u32),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(code) => write!(f, "Windows refused to announce it (error {code})"),
            Self::Failed(code) => write!(f, "Windows couldn't announce it (error {code})"),
            Self::Taken { wanted, given } => write!(
                f,
                "something else on this network already announces \"{}\", so Windows wanted to call it \
                 \"{}\": give the machine another name to announce",
                short(wanted),
                short(given)
            ),
            Self::NoAnswer => f.write_str("Windows didn't say whether it announced it"),
            Self::NoHostName(code) => write!(f, "this PC's host name couldn't be read (error {code})"),
        }
    }
}

/// An instance's name without its type and domain.
fn short(full: &str) -> &str {
    // The label itself never holds a dot, so the first one ends it.
    full.split_once('.').map_or(full, |(label, _)| label)
}

/// Where one registration stands, between what this side asked and what Windows answered.
// Only Windows' calls use it; elsewhere it is built for its tests.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    /// Asked for, and Windows hasn't answered.
    Registering,
    /// Windows announces it, under this full name.
    Up(String),
    /// Withdrawn before Windows answered; the cancel's own answer is awaited.
    Cancelling,
    /// Being taken back; Windows' answer is awaited.
    Deregistering,
    /// Nothing is announced, and Windows will say nothing more.
    Over,
}

/// What the owner of a registration has to ask Windows next.
// Only Windows' calls use it; elsewhere it is built for its tests.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum Next {
    Nothing,
    /// Cancel the registration that hasn't answered.
    Cancel,
    /// Take back the instance of this full name.
    Deregister(String),
}

/// One registration's state. Windows' answers come in on its threads through [`Lifecycle::answered`],
/// and the owner asks what to do in [`Lifecycle::settled`] and [`Lifecycle::withdraw`].
// Only Windows' calls use it; elsewhere it is built for its tests.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug)]
struct Lifecycle {
    /// The full name asked for.
    wanted: String,
    phase: Phase,
    /// Windows' failing status, once one came.
    failed: Option<u32>,
}

// Only Windows' calls use it; elsewhere it is built for its tests.
#[cfg_attr(not(windows), allow(dead_code))]
impl Lifecycle {
    fn new(wanted: impl Into<String>) -> Self {
        Self { wanted: wanted.into(), phase: Phase::Registering, failed: None }
    }

    /// Windows' answer to what was last asked of it, with the full name of the instance it answered
    /// about when it gave one.
    fn answered(&mut self, status: u32, name: Option<String>) {
        self.phase = match (&self.phase, status) {
            // A cancel that came too late: Windows registered it first, so it has to be taken back.
            (Phase::Registering | Phase::Cancelling, 0) => {
                Phase::Up(name.unwrap_or_else(|| self.wanted.clone()))
            }
            (Phase::Registering, _) => {
                self.failed = Some(status);
                Phase::Over
            }
            (Phase::Cancelling | Phase::Deregistering, _) => Phase::Over,
            // Windows answers once to each request; anything more changes nothing.
            (Phase::Up(_) | Phase::Over, _) => return,
        };
    }

    /// How the registration went, once Windows has said: announced under the name asked for, or why
    /// not.
    fn settled(&self) -> Option<Result<(), Failure>> {
        match &self.phase {
            Phase::Registering => None,
            Phase::Up(name) if *name == self.wanted => Some(Ok(())),
            Phase::Up(name) => Some(Err(Failure::Taken { wanted: self.wanted.clone(), given: name.clone() })),
            Phase::Over => Some(Err(Failure::Failed(self.failed.unwrap_or_default()))),
            Phase::Cancelling | Phase::Deregistering => Some(Err(Failure::NoAnswer)),
        }
    }

    /// What to ask Windows so that nothing is announced. Taking back goes by the name Windows gave,
    /// never the one asked for, which may be someone else's.
    fn withdraw(&mut self) -> Next {
        match std::mem::replace(&mut self.phase, Phase::Over) {
            Phase::Registering => {
                self.phase = Phase::Cancelling;
                Next::Cancel
            }
            Phase::Up(name) => {
                self.phase = Phase::Deregistering;
                Next::Deregister(name)
            }
            waiting @ (Phase::Cancelling | Phase::Deregistering) => {
                self.phase = waiting;
                Next::Nothing
            }
            Phase::Over => Next::Nothing,
        }
    }

    /// Whether an answer from Windows is still awaited.
    fn waiting(&self) -> bool {
        matches!(self.phase, Phase::Registering | Phase::Cancelling | Phase::Deregistering)
    }
}

#[cfg(windows)]
pub use windows::{Found, Registration, browse, host_name, resolve};

#[cfg(windows)]
mod windows {
    use super::{Failure, Lifecycle, Next, Service};
    use crate::text::{read, wide};
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::net::Ipv4Addr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::DNS_REQUEST_PENDING;
    use windows_sys::Win32::NetworkManagement::Dns::{
        DNS_QUERY_REQUEST_VERSION1, DNS_RECORDW, DNS_SERVICE_BROWSE_REQUEST, DNS_SERVICE_CANCEL,
        DNS_SERVICE_INSTANCE, DNS_SERVICE_REGISTER_REQUEST, DNS_SERVICE_RESOLVE_REQUEST, DNS_TYPE_PTR,
        DnsFree, DnsFreeRecordList, DnsServiceBrowse, DnsServiceBrowseCancel, DnsServiceConstructInstance,
        DnsServiceDeRegister, DnsServiceFreeInstance, DnsServiceRegister, DnsServiceRegisterCancel,
        DnsServiceResolve, DnsServiceResolveCancel,
    };
    use windows_sys::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
    use windows_sys::core::PCWSTR;

    /// How long a registration may take to answer. The responder probes for the name first, which
    /// took 0.8 s each time it was tried.
    const REGISTER_WAIT: Duration = Duration::from_secs(5);
    /// How long taking one back may take to answer, which took milliseconds when tried.
    const WITHDRAW_WAIT: Duration = Duration::from_secs(2);
    /// A cancel that came too late is followed by a take-back, and nothing comes after that.
    const MOST_WITHDRAW_STEPS: usize = 2;
    /// `DnsServiceRegister`, `DnsServiceDeRegister` and the rest return this once the request is
    /// on its way and an answer will come through the callback.
    const PENDING: u32 = DNS_REQUEST_PENDING as u32;
    /// The longest host name GetComputerNameExW gives, with its zero.
    const LONGEST_HOST_NAME: usize = 256;

    /// One registration's state, shared with the callback that Windows calls on its own thread.
    struct Shared {
        life: Mutex<Lifecycle>,
        changed: Condvar,
    }

    impl Shared {
        fn life(&self) -> MutexGuard<'_, Lifecycle> {
            // A panic while it was held can't have left it half changed: every change is one
            // assignment.
            self.life.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    /// The registrations Windows may still call back about, by the number given to it as the
    /// request's context. The context is a number, never a pointer, so an answer that comes late, or
    /// twice, finds nothing rather than freed memory.
    static LIVE: Mutex<Option<HashMap<usize, Arc<Shared>>>> = Mutex::new(None);
    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

    fn live() -> MutexGuard<'static, Option<HashMap<usize, Arc<Shared>>>> {
        LIVE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What Windows calls when a registration, its cancel or its take-back is done. It only records
    /// the answer and wakes the owner, which does whatever comes next on its own thread.
    unsafe extern "system" fn answered(
        status: u32,
        context: *const c_void,
        instance: *const DNS_SERVICE_INSTANCE,
    ) {
        let name = if instance.is_null() {
            None
        } else {
            // SAFETY: Windows hands over an instance it made, whose name is null or ends in a zero.
            unsafe { read((*instance).pszInstanceName) }
        };
        if !instance.is_null() {
            // SAFETY: the answer's instance is the callee's to free, and nothing reads it after this.
            unsafe { DnsServiceFreeInstance(instance) };
        }
        let shared = live().as_ref().and_then(|live| live.get(&(context as usize)).cloned());
        if let Some(shared) = shared {
            shared.life().answered(status, name);
            shared.changed.notify_all();
        }
    }

    /// A request as Windows reads it, and the instance it names, which stay where they are until
    /// Windows is done with them.
    struct Request {
        instance: *mut DNS_SERVICE_INSTANCE,
        raw: DNS_SERVICE_REGISTER_REQUEST,
    }

    impl Request {
        fn new(full_name: &str, host: &str, service: &Service, context: usize) -> Result<Box<Self>, Failure> {
            let name = wide(full_name);
            let host = wide(host);
            let keys: Vec<Vec<u16>> = service.txt.iter().map(|(key, _)| wide(key)).collect();
            let values: Vec<Vec<u16>> = service.txt.iter().map(|(_, value)| wide(value)).collect();
            let keys: Vec<PCWSTR> = keys.iter().map(|key| key.as_ptr()).collect();
            let values: Vec<PCWSTR> = values.iter().map(|value| value.as_ptr()).collect();
            // SAFETY: every string ends in a zero, the arrays hold `txt.len()` of them each, and all
            // of them live until the call returns, as Windows copies them into the instance it makes.
            // No address is given: the responder answers with the computer's own.
            let instance = unsafe {
                DnsServiceConstructInstance(
                    name.as_ptr(),
                    host.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    service.port,
                    0,
                    0,
                    keys.len() as u32,
                    keys.as_ptr(),
                    values.as_ptr(),
                )
            };
            if instance.is_null() {
                return Err(Failure::Refused(0));
            }
            Ok(Box::new(Self {
                instance,
                raw: DNS_SERVICE_REGISTER_REQUEST {
                    Version: DNS_QUERY_REQUEST_VERSION1,
                    // Every network the computer is on.
                    InterfaceIndex: 0,
                    pServiceInstance: instance,
                    pRegisterCompletionCallback: Some(answered),
                    pQueryContext: context as *mut c_void,
                    ..Default::default()
                },
            }))
        }
    }

    impl Drop for Request {
        fn drop(&mut self) {
            // SAFETY: the instance came from DnsServiceConstructInstance, and a request is only
            // dropped once Windows has answered it, so nothing reads it any more.
            unsafe { DnsServiceFreeInstance(self.instance) };
        }
    }

    /// One service announced through Windows' responder, until it is withdrawn or dropped.
    pub struct Registration {
        id: usize,
        shared: Arc<Shared>,
        service: Service,
        host: String,
        /// Every request Windows may still read, kept until it has answered all of them.
        /// Boxed, so each stays where Windows was given it as the list grows.
        #[allow(clippy::vec_box)]
        requests: Vec<Box<Request>>,
        /// Where Windows keeps what it needs to cancel the registration.
        cancel: Box<DNS_SERVICE_CANCEL>,
    }

    // SAFETY: the requests' pointers are to memory this owns, which nothing else reaches but
    // Windows' own calls, and the state Windows' threads change is behind a mutex.
    unsafe impl Send for Registration {}

    impl Registration {
        /// Announces `service`, pointing at this computer's own host name, and waits for Windows to
        /// say how it went. Nothing stays announced when this fails.
        pub fn register(service: &Service) -> Result<Self, Failure> {
            let registration = Self::start(service)?;
            registration.wait(REGISTER_WAIT, |life| life.settled().is_some());
            let settled = registration.shared.life().settled();
            match settled {
                Some(Ok(())) => Ok(registration),
                // Dropping it takes back what Windows announced, under the name Windows gave.
                Some(Err(failure)) => Err(failure),
                None => Err(Failure::NoAnswer),
            }
        }

        /// Asks Windows to announce `service`, and returns before it answers.
        fn start(service: &Service) -> Result<Self, Failure> {
            let host = host_name()?;
            let full_name = service.full_name();
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let shared =
                Arc::new(Shared { life: Mutex::new(Lifecycle::new(&full_name)), changed: Condvar::new() });
            let request = Request::new(&full_name, &host, service, id)?;
            // In the list before the call: Windows may answer before the call returns.
            live().get_or_insert_with(HashMap::new).insert(id, shared.clone());
            let mut registration = Self {
                id,
                shared,
                service: service.clone(),
                host,
                requests: Vec::new(),
                cancel: Box::default(),
            };
            // SAFETY: the request and the cancel handle are boxed in `registration`, which keeps
            // them until Windows has answered.
            let status = unsafe { DnsServiceRegister(&request.raw, &mut *registration.cancel) };
            registration.requests.push(request);
            if status != PENDING {
                // Not taken, so no answer will come.
                registration.shared.life().answered(status, None);
                registration.forget();
                return Err(Failure::Refused(status));
            }
            Ok(registration)
        }

        /// The instance's full name, as Windows announces it.
        pub fn full_name(&self) -> String {
            self.service.full_name()
        }

        /// Takes the announcement back, so browsers drop it now rather than when its records run
        /// out, and waits a little for Windows to say so. Doing it again does nothing.
        pub fn withdraw(&mut self) {
            for _ in 0..MOST_WITHDRAW_STEPS {
                // Asked of the state first, and of Windows after the lock is let go: a cancel's
                // answer comes before the cancel returns.
                let next = self.shared.life().withdraw();
                match next {
                    Next::Nothing => {}
                    Next::Cancel => {
                        // SAFETY: the handle Windows filled for this registration, which it hasn't
                        // answered yet.
                        unsafe { DnsServiceRegisterCancel(&*self.cancel) };
                    }
                    Next::Deregister(name) => {
                        let Ok(request) = Request::new(&name, &self.host, &self.service, self.id) else {
                            break;
                        };
                        // SAFETY: the request is kept in `requests` until Windows has answered.
                        let status = unsafe { DnsServiceDeRegister(&request.raw, std::ptr::null_mut()) };
                        self.requests.push(request);
                        if status != PENDING {
                            self.shared.life().answered(status, None);
                        }
                    }
                }
                if !self.wait(WITHDRAW_WAIT, |life| !life.waiting()) {
                    break;
                }
            }
            if self.shared.life().waiting() {
                // Windows may still read the requests, so they are left to it rather than freed.
                std::mem::forget(std::mem::take(&mut self.requests));
            } else {
                self.forget();
            }
        }

        /// Waits until `done` holds, or `most` has passed. Says whether it held.
        fn wait(&self, most: Duration, done: impl Fn(&Lifecycle) -> bool) -> bool {
            let until = Instant::now() + most;
            let mut life = self.shared.life();
            while !done(&life) {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                life = self.shared.changed.wait_timeout(life, left).unwrap_or_else(PoisonError::into_inner).0;
            }
            true
        }

        /// Windows has answered everything: it calls back about this registration no more.
        fn forget(&mut self) {
            if let Some(live) = live().as_mut() {
                live.remove(&self.id);
            }
            self.requests.clear();
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            self.withdraw();
        }
    }

    /// Withdrawing before Windows has answered, on this PC's own responder: the registration is
    /// cancelled, and neither its name nor a `NAME(1)` stays announced.
    /// `cargo test -p guesthouse-hcs --lib withdrawn_before -- --ignored --nocapture`
    #[test]
    #[ignore = "announces on this PC's networks for a few seconds"]
    fn withdrawn_before_windows_answered_nothing_stays() {
        const LOOK: Duration = Duration::from_secs(3);
        let service = Service {
            instance: "Guesthouse cancel test".into(),
            kind: "_remoteman._udp".into(),
            port: 65103,
            txt: vec![("v".into(), "0".into())],
        };
        let mut registration = Registration::start(&service).expect("Windows takes the registration");
        assert!(registration.shared.life().waiting(), "Windows has not answered yet");
        registration.withdraw();
        assert!(!registration.shared.life().waiting(), "the cancel was answered");
        println!("after the cancel: {:?}", registration.shared.life());
        registration.withdraw();
        drop(registration);
        let seen = browse("_remoteman._udp", LOOK).expect("a browse");
        println!("browse: {seen:?}");
        assert!(!seen.iter().any(|name| name.starts_with("Guesthouse cancel test")), "{seen:?}");
        let found = resolve(&service.full_name(), LOOK).expect("a resolve");
        assert_eq!(found, None);
    }

    /// `HOST.local`, this computer's own host name, which its responder answers for.
    pub fn host_name() -> Result<String, Failure> {
        let mut buffer = [0u16; LONGEST_HOST_NAME];
        let mut length = buffer.len() as u32;
        // SAFETY: the buffer holds `length` units, and Windows writes no more than that.
        if unsafe { GetComputerNameExW(ComputerNameDnsHostname, buffer.as_mut_ptr(), &mut length) } == 0 {
            // SAFETY: only reads this thread's last error.
            return Err(Failure::NoHostName(unsafe { windows_sys::Win32::Foundation::GetLastError() }));
        }
        Ok(format!("{}.{}", crate::text::from_wide(&buffer), super::LOCAL))
    }

    /// What a resolved instance says of itself.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Found {
        pub name: String,
        pub host: String,
        pub port: u16,
        pub address: Option<Ipv4Addr>,
        pub txt: Vec<(String, String)>,
    }

    /// What a browse or a resolve has heard, by the number given as its context.
    static HEARD: Mutex<Option<HashMap<usize, Heard>>> = Mutex::new(None);

    enum Heard {
        Names(Vec<String>),
        Instance(Option<Found>),
    }

    fn heard() -> MutexGuard<'static, Option<HashMap<usize, Heard>>> {
        HEARD.lock().unwrap_or_else(PoisonError::into_inner)
    }

    unsafe extern "system" fn browsed(_: u32, context: *const c_void, records: *const DNS_RECORDW) {
        let mut names = Vec::new();
        let mut record = records;
        while !record.is_null() {
            // SAFETY: a list Windows made, each record whole until the list is freed below.
            let this = unsafe { &*record };
            if this.wType == DNS_TYPE_PTR {
                // SAFETY: a PTR record's data is its PTR member, whose name ends in a zero.
                names.extend(unsafe { read(this.Data.PTR.pNameHost) });
            }
            record = this.pNext;
        }
        if !records.is_null() {
            // SAFETY: the list is the callee's to free, and nothing reads it after this.
            unsafe { DnsFree(records.cast(), DnsFreeRecordList) };
        }
        if let Some(Heard::Names(all)) = heard().as_mut().and_then(|heard| heard.get_mut(&(context as usize)))
        {
            all.extend(names);
        }
    }

    unsafe extern "system" fn resolved(
        status: u32,
        context: *const c_void,
        instance: *const DNS_SERVICE_INSTANCE,
    ) {
        if instance.is_null() {
            return;
        }
        // SAFETY: an instance Windows made, whose strings end in a zero and whose arrays hold
        // `dwPropertyCount` of them, whole until it is freed below.
        let found = unsafe {
            let instance = &*instance;
            let text = |at: *mut *mut u16, n: usize| if at.is_null() { None } else { read(*at.add(n)) };
            Found {
                name: read(instance.pszInstanceName).unwrap_or_default(),
                host: read(instance.pszHostName).unwrap_or_default(),
                port: instance.wPort,
                address: (!instance.ip4Address.is_null())
                    .then(|| Ipv4Addr::from((*instance.ip4Address).to_ne_bytes())),
                txt: (0..instance.dwPropertyCount as usize)
                    .map(|n| {
                        (
                            text(instance.keys, n).unwrap_or_default(),
                            text(instance.values, n).unwrap_or_default(),
                        )
                    })
                    .collect(),
            }
        };
        // SAFETY: the instance is the callee's to free, and nothing reads it after this.
        unsafe { DnsServiceFreeInstance(instance) };
        if status == 0
            && let Some(Heard::Instance(slot)) =
                heard().as_mut().and_then(|heard| heard.get_mut(&(context as usize)))
        {
            *slot = Some(found);
        }
    }

    /// Every instance of `kind` (`_remoteman._udp`) the network names within `wait`, this
    /// computer's own included, as full names. For checking an announcement.
    pub fn browse(kind: &str, wait: Duration) -> Result<Vec<String>, Failure> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        heard().get_or_insert_with(HashMap::new).insert(id, Heard::Names(Vec::new()));
        let query = wide(format!("{kind}.{}", super::LOCAL));
        let mut request = DNS_SERVICE_BROWSE_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1,
            InterfaceIndex: 0,
            QueryName: query.as_ptr(),
            pQueryContext: id as *mut c_void,
            ..Default::default()
        };
        request.Anonymous.pBrowseCallback = Some(browsed);
        let mut cancel = DNS_SERVICE_CANCEL::default();
        // SAFETY: the request's name lives until the browse is cancelled below, after which Windows
        // reads nothing of it, and its context is a number.
        let status = unsafe { DnsServiceBrowse(&request, &mut cancel) };
        if status as u32 == PENDING {
            std::thread::sleep(wait);
            // SAFETY: the handle Windows filled for this browse.
            unsafe { DnsServiceBrowseCancel(&cancel) };
        }
        let names = match heard().as_mut().and_then(|heard| heard.remove(&id)) {
            Some(Heard::Names(mut names)) => {
                names.sort();
                names.dedup();
                names
            }
            _ => Vec::new(),
        };
        if status as u32 == PENDING { Ok(names) } else { Err(Failure::Refused(status as u32)) }
    }

    /// Where the instance `full_name` points and what it says, or `None` if it doesn't answer
    /// within `wait`. For checking an announcement.
    pub fn resolve(full_name: &str, wait: Duration) -> Result<Option<Found>, Failure> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        heard().get_or_insert_with(HashMap::new).insert(id, Heard::Instance(None));
        let mut query = wide(full_name);
        let request = DNS_SERVICE_RESOLVE_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1,
            InterfaceIndex: 0,
            QueryName: query.as_mut_ptr(),
            pResolveCompletionCallback: Some(resolved),
            pQueryContext: id as *mut c_void,
        };
        let mut cancel = DNS_SERVICE_CANCEL::default();
        // SAFETY: as in `browse`: the name lives until the resolve is cancelled below.
        let status = unsafe { DnsServiceResolve(&request, &mut cancel) };
        if status as u32 == PENDING {
            let until = Instant::now() + wait;
            while Instant::now() < until
                && matches!(heard().as_ref().and_then(|heard| heard.get(&id)), Some(Heard::Instance(None)))
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            // SAFETY: the handle Windows filled for this resolve; cancelling one that has answered
            // does nothing.
            unsafe { DnsServiceResolveCancel(&cancel) };
        }
        let found = match heard().as_mut().and_then(|heard| heard.remove(&id)) {
            Some(Heard::Instance(found)) => found,
            _ => None,
        };
        if status as u32 == PENDING { Ok(found) } else { Err(Failure::Refused(status as u32)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WANTED: &str = "Windows 11._remoteman._udp.local";
    /// What Windows answers a cancel and a take-back of nothing with.
    const CANCELLED: u32 = 1223;
    const NO_SUCH_RECORD: u32 = 9701;

    #[test]
    fn the_full_name_is_one_label_then_the_type_under_local() {
        let service = Service {
            instance: "Windows 11".into(),
            kind: "_remoteman._udp".into(),
            port: 7792,
            txt: vec![],
        };
        assert_eq!(service.full_name(), WANTED);
        let dotted = Service { instance: r"v1.2\b".into(), ..service };
        assert_eq!(dotted.full_name(), "v1-2-b._remoteman._udp.local");
    }

    #[test]
    fn an_answer_under_the_name_asked_for_is_announced_and_taken_back_under_it() {
        let mut life = Lifecycle::new(WANTED);
        assert_eq!(life.settled(), None);
        life.answered(0, Some(WANTED.into()));
        assert_eq!(life.settled(), Some(Ok(())));
        assert_eq!(life.withdraw(), Next::Deregister(WANTED.into()));
        assert!(life.waiting());
        life.answered(0, Some(WANTED.into()));
        assert!(!life.waiting());
        assert_eq!(life.withdraw(), Next::Nothing, "withdrawing again does nothing");
    }

    #[test]
    fn a_name_windows_changed_is_a_conflict_and_is_taken_back_under_the_name_it_got() {
        let mut life = Lifecycle::new(WANTED);
        life.answered(0, Some("Windows 11(1)._remoteman._udp.local".into()));
        assert_eq!(
            life.settled(),
            Some(Err(Failure::Taken {
                wanted: WANTED.into(),
                given: "Windows 11(1)._remoteman._udp.local".into()
            }))
        );
        assert_eq!(
            life.withdraw(),
            Next::Deregister("Windows 11(1)._remoteman._udp.local".into()),
            "never the name asked for, which is someone else's"
        );
        let message = life.settled().map(|settled| settled.unwrap_err().to_string());
        assert_eq!(message, Some(Failure::NoAnswer.to_string()), "while the take-back is awaited");
        let taken =
            Failure::Taken { wanted: WANTED.into(), given: "Windows 11(1)._remoteman._udp.local".into() };
        assert!(taken.to_string().contains("\"Windows 11\""), "{taken}");
        assert!(taken.to_string().contains("\"Windows 11(1)\""), "{taken}");
    }

    #[test]
    fn a_failed_registration_has_nothing_to_take_back() {
        let mut life = Lifecycle::new(WANTED);
        life.answered(87, None);
        assert_eq!(life.settled(), Some(Err(Failure::Failed(87))));
        assert_eq!(life.withdraw(), Next::Nothing);
        assert!(!life.waiting());
    }

    #[test]
    fn withdrawing_before_windows_answered_cancels_and_the_cancel_ends_it() {
        let mut life = Lifecycle::new(WANTED);
        assert_eq!(life.withdraw(), Next::Cancel);
        assert_eq!(life.withdraw(), Next::Nothing, "the cancel is asked for once");
        life.answered(CANCELLED, None);
        assert!(!life.waiting());
        assert_eq!(life.withdraw(), Next::Nothing);
    }

    #[test]
    fn a_registration_that_answered_before_its_cancel_is_taken_back() {
        let mut life = Lifecycle::new(WANTED);
        assert_eq!(life.withdraw(), Next::Cancel);
        life.answered(0, Some(WANTED.into()));
        assert!(!life.waiting());
        assert_eq!(life.withdraw(), Next::Deregister(WANTED.into()));
        life.answered(NO_SUCH_RECORD, None);
        assert!(!life.waiting());
    }

    #[test]
    fn an_answer_nobody_waits_for_changes_nothing() {
        let mut life = Lifecycle::new(WANTED);
        life.answered(0, Some(WANTED.into()));
        life.answered(87, None);
        assert_eq!(life.settled(), Some(Ok(())));
        life.withdraw();
        life.answered(0, None);
        life.answered(0, Some("other._remoteman._udp.local".into()));
        assert!(!life.waiting());
        assert_eq!(life.withdraw(), Next::Nothing);
    }
}
