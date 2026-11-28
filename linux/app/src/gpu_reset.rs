//! What the app does when the GPU is reset under it.
//!
//! GTK draws the window through Vulkan, and a reset loses that device for good. GTK logs
//! VK_ERROR_DEVICE_LOST and does nothing more, so the window never draws again. The mouse goes dead
//! over it too, as GTK hands on motion only when it draws, while keys still reach the guest. GTK
//! keeps the device for as long as its display connection is open, as it holds a renderer of its own
//! for reading dmabufs, and Mesa doesn't bring back a lost GL context either. So only a new process
//! draws again. Every VM runs on its own and the app can close at any moment, so it starts itself
//! again in place, and the new one opens the screen that was open.
//!
//! Only a lost Vulkan device is noticed. GTK says nothing when a GL context is lost.

use crate::application::APP_ID;
use crate::login::AT_LOGIN;
use gtk::prelude::*;
use gtk::{gio, glib};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::log::write_log;
use std::cell::RefCell;
use std::env;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The note the old process leaves the new one.
pub const VARIABLE: &str = "GUESTHOUSE_RESTARTED";
/// A process that loses the GPU this soon after it was started over stays as it is: the card may be
/// failing for good, and starting over and over helps nobody.
const SOONEST_AGAIN: Duration = Duration::from_secs(60);
/// How long the new process waits for the old one's name to leave the session bus. If it were still
/// there, the new one would take itself for a second copy and hand over to one that's gone.
const NAME_WAIT: Duration = Duration::from_secs(3);
const NAME_LOOK_EVERY: Duration = Duration::from_millis(50);
/// GTK's log domains, where the loss shows.
const GTK_DOMAINS: [&str; 2] = ["Gdk", "Gsk"];
const DEVICE_LOST: &str = "VK_ERROR_DEVICE_LOST";
/// The process itself, which still runs if its file was replaced by an update.
const THIS_PROGRAM: &str = "/proc/self/exe";

/// What the old process hands the new one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Restart {
    /// Seconds since 1970 when the old process gave up.
    pub at: u64,
    /// The machine whose screen was open.
    pub screen: Option<String>,
    pub fullscreen: bool,
}

impl Restart {
    pub fn value(&self) -> String {
        format!("{} {} {}", self.at, u8::from(self.fullscreen), self.screen.as_deref().unwrap_or(""))
    }

    pub fn from_value(value: &str) -> Option<Self> {
        let mut parts = value.splitn(3, ' ');
        let at = parts.next()?.parse().ok()?;
        let fullscreen = match parts.next()? {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        let screen = parts.next().filter(|id| !id.is_empty()).map(str::to_owned);
        Some(Self { at, screen, fullscreen })
    }

    /// Whether losing the GPU at `now` is too soon after this start to start over again.
    fn too_recent_for(&self, now: u64) -> bool {
        now.saturating_sub(self.at) < SOONEST_AGAIN.as_secs()
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// Whether a line of the log is GTK saying its GPU device is gone.
pub fn names_a_lost_device(domain: Option<&str>, message: &str) -> bool {
    domain.is_some_and(|domain| GTK_DOMAINS.contains(&domain)) && message.contains(DEVICE_LOST)
}

fn is_loss(level: glib::LogLevel, fields: &[glib::LogField<'_>]) -> bool {
    if !matches!(level, glib::LogLevel::Warning | glib::LogLevel::Critical) {
        return false;
    }
    let field =
        |key: &str| fields.iter().find(|field| field.key() == key).and_then(|field| field.value_str());
    field("MESSAGE").is_some_and(|message| names_a_lost_device(field("GLIB_DOMAIN"), message))
}

/// Seen once, as GTK goes on logging the loss at every frame it tries.
static LOST: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The window's part, which runs on GTK's thread.
    static WHEN_LOST: RefCell<Option<Box<dyn Fn()>>> = RefCell::default();
}

/// Reads every log line on its way out, for GTK losing its device. Once per process, before GTK
/// starts. Every line still goes where GLib would put it.
pub fn watch() {
    glib::log_set_writer_func(|level, fields| {
        if is_loss(level, fields) && !LOST.swap(true, Ordering::Relaxed) {
            // GTK is in the middle of a frame, and may be on another thread.
            glib::idle_add_once(|| {
                WHEN_LOST.with(|action| {
                    if let Some(action) = action.borrow().as_ref() {
                        action();
                    }
                })
            });
        }
        glib::log_writer_default(level, fields)
    });
}

/// What the window does when GTK loses the GPU. The last one set is the one done.
pub fn when_lost(action: impl Fn() + 'static) {
    WHEN_LOST.with(|slot| *slot.borrow_mut() = Some(Box::new(action)));
}

/// Starts the app again in this process, with `note` for the new one. `earlier` is the note this
/// process was started with, if it was. Returns only if it didn't start over.
pub fn start_over(context: &Context, note: &Restart, earlier: Option<&Restart>) {
    if earlier.is_some_and(|earlier| earlier.too_recent_for(note.at)) {
        let _ = write_log(
            context,
            "app: GTK lost the GPU again just after starting over, so the app stays as it is",
        );
        return;
    }
    let _ = write_log(context, "app: GTK lost the GPU, as after a GPU reset, so the app starts over");
    let error = command(note, env::args_os()).exec();
    let _ = write_log(context, &format!("app: couldn't start over: {error}"));
}

/// The app again, with the arguments it had. A start at login isn't repeated: the machines it starts
/// are running, and the window was in use.
fn command(note: &Restart, arguments: impl Iterator<Item = OsString>) -> Command {
    let mut arguments = arguments;
    let mut command = Command::new(THIS_PROGRAM);
    if let Some(name) = arguments.next() {
        command.arg0(name);
    }
    command.args(arguments.filter(|argument| argument != AT_LOGIN)).env(VARIABLE, note.value());
    command
}

/// Waits, a little, for the old process's name to leave the session bus.
pub fn wait_for_the_name() {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else { return };
    let started = Instant::now();
    while started.elapsed() < NAME_WAIT {
        let reply = bus.call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&(APP_ID,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            NAME_WAIT.as_millis() as i32,
            gio::Cancellable::NONE,
        );
        if !reply.ok().and_then(|reply| reply.get::<(bool,)>()).is_some_and(|(owned,)| owned) {
            return;
        }
        thread::sleep(NAME_LOOK_EVERY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lines from the NixOS tester's terminal, after the reset on 2026-11-20.
    #[test]
    fn gtks_own_lines_after_a_reset_are_a_loss() {
        let submit =
            "vkQueueSubmit(): The logical or physical device has been lost. (VK_ERROR_DEVICE_LOST) (-4)";
        let fence =
            "vkGetFenceStatus(): The logical or physical device has been lost. (VK_ERROR_DEVICE_LOST) (-4)";
        let failed =
            "Vulkan: ../src/amd/vulkan/radv_queue.c:1931: vkQueueSubmit() failed (VK_ERROR_DEVICE_LOST)";
        assert!(names_a_lost_device(Some("Gsk"), submit));
        assert!(names_a_lost_device(Some("Gsk"), fence));
        assert!(names_a_lost_device(Some("Gdk"), failed));
    }

    #[test]
    fn a_swapchain_to_remake_or_another_domain_is_not() {
        let suboptimal = "vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, \
                          but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR) (1000001003)";
        assert!(!names_a_lost_device(Some("Gdk"), suboptimal));
        assert!(!names_a_lost_device(Some("guesthouse"), "VK_ERROR_DEVICE_LOST"));
        assert!(!names_a_lost_device(None, "VK_ERROR_DEVICE_LOST"));
    }

    #[test]
    fn the_note_goes_across_whole() {
        let note = Restart { at: 1_795_000_000, screen: Some("Windows 11 Pro".into()), fullscreen: true };
        assert_eq!(Restart::from_value(&note.value()), Some(note));
        let bare = Restart { at: 7, screen: None, fullscreen: false };
        assert_eq!(bare.value(), "7 0 ");
        assert_eq!(Restart::from_value(&bare.value()), Some(bare));
        assert_eq!(Restart::from_value("soon 1 a"), None);
        assert_eq!(Restart::from_value("7 yes a"), None);
        assert_eq!(Restart::from_value(""), None);
    }

    #[test]
    fn a_second_loss_within_a_minute_stays() {
        let started_over = Restart { at: 1000, ..Restart::default() };
        assert!(started_over.too_recent_for(1059));
        assert!(!started_over.too_recent_for(1060));
    }

    #[test]
    fn the_new_process_is_this_one_with_its_arguments_but_not_the_login_one() {
        let note = Restart { at: 5, screen: Some("vm".into()), fullscreen: false };
        let arguments = ["guesthouse", AT_LOGIN, "--other"].map(OsString::from);
        let command = command(&note, arguments.into_iter());
        assert_eq!(command.get_program(), THIS_PROGRAM);
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--other"]);
        let set: Vec<_> = command.get_envs().collect();
        assert_eq!(set, [(std::ffi::OsStr::new(VARIABLE), Some(std::ffi::OsStr::new("5 0 vm")))]);
    }
}
