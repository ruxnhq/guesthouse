//! Shows the Remote View group of a machine's Network page in each state it can be in, from canned
//! values, and saves a picture of each, in light and in dark. It needs no machine and no guest: see
//! `remote-view-shots.sh`, which runs it on GTK's Broadway backend, where nothing appears on a desktop.
//!
//! Usage: `remote_view_shots FOLDER`.

use adw::prelude::*;
use gtk::{gdk, glib};
use guesthouse_app::settings::remote_view::RemoteView;
use guesthouse_app::settings::remote_view::fixture::Fixture;
use guesthouse_remote_view_core::share::{
    CloseReason, DeviceInfo, Pending, Permissions, ShareInfo, ShareState,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FINGERPRINT: &str = "9F2C41AB07D3E85C6B1A0F44D2E9C873A5B60E1D";
const SETTLE: Duration = Duration::from_millis(700);
const WIDTH: i32 = 640;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(0))
}

fn listening() -> ShareInfo {
    ShareInfo {
        machine: "fixture".to_owned(),
        state: ShareState::Listening,
        port: Some(7792),
        fingerprint: Some(FINGERPRINT.to_owned()),
        ..ShareInfo::default()
    }
}

fn device(name: &str, tail: &str, connected: bool, permissions: Permissions) -> DeviceInfo {
    DeviceInfo {
        fingerprint: format!("{tail}{}", &FINGERPRINT[tail.len()..]),
        name: name.to_owned(),
        connected,
        permissions,
    }
}

/// Each state: its file name, whether the switch is on, what the core says.
fn states() -> Vec<(&'static str, bool, ShareInfo)> {
    let mut open = listening();
    open.pairing = true;
    open.pairing_until_ms = Some(now_ms() + 95_000);
    let mut waiting = listening();
    waiting.pending = Some(Pending {
        name: "Pixel 9".to_owned(),
        fingerprint: "A1B2C3D4".to_owned(),
        code: "482 917".to_owned(),
        seconds: 52,
    });
    let mut listed = listening();
    listed.viewer = Some("Studio laptop".to_owned());
    listed.devices = vec![
        device("Studio laptop", "1A2B", true, Permissions::default()),
        device(
            "Pixel 9",
            "7C8D",
            false,
            Permissions { files: false, mic: false, camera: false, ..Permissions::default() },
        ),
    ];
    let closed = |reason, paired: Option<&str>| {
        let mut info = listening();
        info.closed_reason = Some(reason);
        info.paired = paired.map(str::to_owned);
        info
    };
    let mut failed = ShareInfo::off("fixture");
    failed.state = ShareState::Failed;
    failed.reason = Some("Port 7792 is already in use by another program.".to_owned());
    vec![
        ("1-off", false, ShareInfo::off("fixture")),
        ("2-listening", true, listening()),
        ("3-pairing-open", true, open),
        ("4-device-waiting", true, waiting),
        ("5-devices", true, listed),
        ("6-closed-timeout", true, closed(CloseReason::Timeout, None)),
        ("6b-closed-declined", true, closed(CloseReason::Declined, None)),
        ("6c-closed-paired", true, closed(CloseReason::Paired, Some("Pixel 9"))),
        ("7-failed", true, failed),
    ]
}

/// The page's groups as one widget: its scrolled area's content, which is as tall as they are whatever
/// the window shows of it. Broadway's screen is short, so a tall page is drawn from here, not from the
/// window.
fn content_of(widget: &gtk::Widget) -> Option<gtk::Widget> {
    if let Some(area) = widget.downcast_ref::<gtk::ScrolledWindow>() {
        return area.child().and_downcast::<gtk::Viewport>().and_then(|viewport| viewport.child());
    }
    let mut child = widget.first_child();
    while let Some(next) = child {
        if let Some(found) = content_of(&next) {
            return Some(found);
        }
        child = next.next_sibling();
    }
    None
}

/// The window's own background, which the content has none of.
fn background() -> gdk::RGBA {
    if adw::StyleManager::default().is_dark() {
        gdk::RGBA::new(0.14, 0.14, 0.16, 1.0)
    } else {
        gdk::RGBA::new(0.98, 0.98, 0.98, 1.0)
    }
}

fn save(content: &gtk::Widget, path: &Path) -> bool {
    let (width, height) = (f64::from(content.width()), f64::from(content.height()));
    let snapshot = gtk::Snapshot::new();
    snapshot.append_color(&background(), &gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32));
    gtk::WidgetPaintable::new(Some(content)).snapshot(&snapshot, width, height);
    let Some(node) = snapshot.to_node() else { return false };
    let Some(renderer) = content.native().and_then(|native| native.renderer()) else { return false };
    renderer.render_texture(node, None).save_to_png(path).is_ok()
}

/// Takes the picture right after a paint, as only then is nothing waiting for layout.
async fn shot(widget: &gtk::Widget, content: &gtk::Widget, path: PathBuf) -> bool {
    let Some(clock) = widget.frame_clock() else { return false };
    let (done, wait) = async_channel::bounded::<bool>(1);
    let (target, path) = (content.clone(), path);
    let handler = clock.connect_after_paint(move |_| {
        if !done.is_full() {
            let _ = done.try_send(save(&target, &path));
        }
    });
    widget.queue_draw();
    let saved = wait.recv().await.unwrap_or(false);
    clock.disconnect(handler);
    saved
}

async fn walk(folder: PathBuf) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_enable_animations(false);
    }
    let mut failed = false;
    for (scheme, name) in [(adw::ColorScheme::ForceLight, "light"), (adw::ColorScheme::ForceDark, "dark")] {
        adw::StyleManager::default().set_color_scheme(scheme);
        for (file, allowed, info) in states() {
            let page = adw::PreferencesPage::new();
            let backend = Arc::new(Fixture::new(allowed, info));
            let view = RemoteView::new(&page, backend, |text| println!("problem: {text}"));
            view.set_running(true);
            let window =
                adw::Window::builder().default_width(WIDTH).default_height(600).content(&page).build();
            window.present();
            glib::timeout_future(SETTLE).await;
            let Some(content) = content_of(page.upcast_ref()) else { panic!("the page has no content") };
            let path = folder.join(format!("{file}-{name}.png"));
            let saved = shot(window.upcast_ref(), &content, path.clone()).await;
            println!("{} {}", if saved { "saved" } else { "FAILED" }, path.display());
            failed |= !saved;
            window.close();
        }
    }
    std::process::exit(i32::from(failed));
}

fn main() -> glib::ExitCode {
    let folder = PathBuf::from(std::env::args().nth(1).expect("a folder for the pictures"));
    std::fs::create_dir_all(&folder).expect("the folder");
    let app = adw::Application::builder().application_id("com.ruxn.guesthouse.RemoteViewShots").build();
    let _hold = app.hold();
    app.connect_activate(move |_| {
        glib::spawn_future_local(walk(folder.clone()));
    });
    app.run_with_args::<&str>(&[])
}
