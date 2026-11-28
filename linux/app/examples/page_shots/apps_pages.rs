//! The Apps page of a Windows machine in each of its states, for `PAGE_SHOTS_WALK=apps`. The machine is
//! a stand-in, its helper is the pretend one of `app_windows::stand_in` on this process's own loopback
//! port, the agent that would start the helper is a stub, and QEMU is a sleeping process with the words
//! of a machine that has the way in to its helper. Nothing here reaches a guest or a viewer: an Open is
//! only ever answered with silence or a refusal, so no viewer is started.

use super::*;
use guesthouse_app_core::agent::GuestAgent;
use guesthouse_app_core::app_windows::launcher::{self, Program};
use guesthouse_app_core::app_windows::open_windows::{self, OpenProgram};
use guesthouse_app_core::app_windows::stand_in::{FakeHelper, Welcome};
use guesthouse_app_core::app_windows::viewer_lock;
use guesthouse_app_core::app_windows::{
    Caption, Cell as WindowCell, Layout, LinkSettings, Point, Rect, Size, State as WindowState, Timing,
    answer_line, forward_word, host_port,
};
use guesthouse_app_core::tasks::{State, Status};
use guesthouse_app_core::text::base64_encode;

pub const TOKEN: &str = "page-shots-token";

/// An agent that has nobody logged in, which is what a guest is for a while after it starts.
struct Nobody;

impl GuestAgent for Nobody {
    fn ask(&self, _port: u16, _token: &str, _script: &str, _seconds: u64) -> String {
        "nobody is logged in".to_owned()
    }
}

thread_local! {
    static HELPER: std::cell::RefCell<Option<Rc<FakeHelper>>> = const { std::cell::RefCell::new(None) };
}

fn helper() -> Rc<FakeHelper> {
    HELPER.with(|helper| helper.borrow().clone().expect("the helper is started in main"))
}

/// Called before the context is shared: the pretend helper, the stub agent, quick timings.
pub fn prepare(context: &mut Context, folder: &Path) {
    let helper = FakeHelper::start(TOKEN);
    let timing = Timing {
        connect: Duration::from_millis(800),
        hello: Duration::from_millis(600),
        write: Duration::from_millis(800),
        reconnect_first: Duration::from_millis(50),
        reconnect_most: Duration::from_millis(200),
        start_cooldown: Duration::from_millis(300),
        // Long enough that an Open that is not answered shows busy for a look.
        request: Duration::from_secs(5),
        apps_request: Duration::from_secs(3),
    };
    context.app_windows = LinkSettings { port: Some(helper.port), timing, patience: Duration::from_secs(2) };
    context.agent = Arc::new(Nobody);
    let guest = PathBuf::from(env::var("GUESTHOUSE_PROGRAM").expect("the script sets it")).join("guest");
    fs::create_dir_all(&guest).unwrap();
    for name in ["app-windows.ps1", "app-windows.cs", "app-windows-start.ps1", "helper-stop.ps1"] {
        fs::write(guest.join(name), "script").unwrap();
    }
    let disk = folder.join("win.qcow2");
    fs::write(&disk, "").unwrap();
    let machine = Machine {
        kind: Kind::Windows,
        ..Machine::new(
            "win",
            "Windows 11",
            &disk.display().to_string(),
            &folder.join("vars").display().to_string(),
        )
    };
    fs::create_dir_all(machine.folder(context)).unwrap();
    machine.save(context).unwrap();
    fs::write(machine.token_file(context), format!("{TOKEN}\n")).unwrap();
    HELPER.with(|slot| *slot.borrow_mut() = Some(Rc::new(helper)));
}

fn cell(id: &str, exe: &str, popup: bool) -> WindowCell {
    let rect = Rect { x: 0, y: 0, w: 400, h: 300 };
    WindowCell {
        id: id.into(),
        pid: 7,
        exe: exe.into(),
        app: String::new(),
        name: String::new(),
        cls: String::new(),
        title: String::new(),
        cell: rect,
        frame: rect,
        popup,
        parent: String::new(),
        owner: String::new(),
        rel: Point::default(),
        state: WindowState::Shown,
        overflow: false,
        focus: false,
        caption: Caption::default(),
        resizable: true,
        corner: 0,
    }
}

/// A square PNG of one colour, as a helper's icon.
fn icon(shade: u8) -> String {
    let pixels: Vec<u8> = (0..48 * 48).flat_map(|_| [shade, 90, 255 - shade, 255]).collect();
    let texture = gtk::gdk::MemoryTexture::new(
        48,
        48,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(pixels),
        48 * 4,
    );
    base64_encode(texture.save_to_png_bytes().as_ref())
}

/// What the helper lists: a few programs a person uses, and `minor` uninstallers and help files.
fn programs(minor: usize, many: usize) -> String {
    let mut apps: Vec<String> = Vec::new();
    let mut add = |id: &str, name: &str, target: &str, minor: bool, shade: u8| {
        apps.push(format!(
            "{{\"id\":{id:?},\"name\":{name:?},\"target\":{},\"icon\":{:?},\"minor\":{minor}}}",
            serde_json::to_string(target).unwrap(),
            icon(shade)
        ));
    };
    add("Microsoft Edge", "Microsoft Edge", r"C:\Program Files\Edge\msedge.exe", false, 30);
    add("Notepad", "Notepad", r"C:\Windows\notepad.exe", false, 90);
    add("Paint", "Paint", r"C:\Windows\System32\mspaint.exe", false, 150);
    add("Calculator", "Calculator", r"C:\Windows\System32\calc.exe", false, 210);
    add("Visual Studio Code", "Visual Studio Code", r"C:\Code\Code.exe", false, 60);
    add("Task Manager", "Task Manager", r"C:\Windows\System32\Taskmgr.exe", false, 120);
    for n in 0..minor {
        add(
            &format!("Tools/Uninstall {n}"),
            &format!("Uninstall Tool {n}"),
            &format!(r"C:\Tools\unins{n}.exe"),
            true,
            10,
        );
    }
    for n in 0..many {
        add(
            &format!("Many/App {n}"),
            &format!("Program number {n:03}"),
            &format!(r"C:\Many\app{n}.exe"),
            false,
            (n % 250) as u8,
        );
    }
    format!("[{}]", apps.join(","))
}

fn lists(apps: String) {
    helper().respond_with(move |request, _| {
        let n = request["n"].as_u64().unwrap_or(0);
        match request["t"].as_str() {
            Some("apps") => vec![format!("{{\"t\":\"answer\",\"re\":{n},\"ok\":true,\"apps\":{apps}}}")],
            // An Open is not answered: the button stays busy until the wait is up, and nothing ends in
            // a viewer.
            Some("launch") => Vec::new(),
            _ => vec![answer_line(n, true, "")],
        }
    });
}

fn refuses(why: &'static str) {
    helper().respond_with(move |request, _| {
        let n = request["n"].as_u64().unwrap_or(0);
        vec![answer_line(n, false, why)]
    });
}

/// A process that passes for the machine's QEMU, with the forward or without it.
fn vm_with(context: &Context, machine: &Machine, forward: bool) -> Child {
    let socket = format!("unix:{}/qmp.sock,server=on", context.run_folder(None).display());
    let drive = format!("file={},format=qcow2", machine.disk);
    let mut command = Command::new(env::current_exe().unwrap());
    command.arg0(context.paths.qemu.as_os_str()).args(["-qmp", &socket, "-drive", &drive]);
    if forward {
        command.args(["-nic", &format!("user,{}", forward_word(host_port(context, None)))]);
    }
    command.spawn().unwrap()
}

fn stop(vm: &mut Child) {
    let _ = vm.kill();
    let _ = vm.wait();
}

fn switch(window: &Window, on: bool) {
    let rows = all_of::<adw::SwitchRow>(window);
    if let Some(row) = rows.iter().find(|row| row.title() == "Show Apps as Their Own Windows") {
        row.set_active(on);
    }
}

async fn settle_on(window: &Window, text: &str) -> bool {
    shows(window, text).await
}

fn programs_in_launcher(context: &Context, machine: &Machine) {
    for (name, exe, id, shade) in [
        ("Notepad", r"C:\Windows\notepad.exe", "Notepad", 90),
        ("Paint", r"C:\Windows\System32\mspaint.exe", "Paint", 150),
    ] {
        let png = guesthouse_app_core::text::base64_decode(&icon(shade));
        let program = Program {
            name: name.into(),
            exe: exe.into(),
            launch: guesthouse_app_core::app_windows::Launch::App(id.into()),
            icon: png,
        };
        launcher::add(context, machine, &program).unwrap();
    }
}

fn open_program(name: &str, key: &str, windows: u32, shade: Option<u8>, folder: &Path) -> OpenProgram {
    let icon = shade.map(|shade| {
        let file = folder.join(format!("open-{}.png", name.replace(' ', "-")));
        fs::write(&file, guesthouse_app_core::text::base64_decode(&icon(shade)).unwrap()).unwrap();
        file
    });
    OpenProgram { key: key.into(), name: name.into(), windows, icon }
}

/// The machine's card in the Library with app windows open and without, as the viewer's file says.
async fn card_states(
    window: &Window,
    context: &Arc<Context>,
    machine: &MachineHandle,
    folder: &Path,
    failures: &Rc<Cell<u32>>,
) {
    let shots = |name: &str| format!("card-{name}");
    let run = context.run_folder(None);
    fs::create_dir_all(&run).unwrap();
    let mut vm = vm_with(context, &machine.borrow(), false);
    glib::timeout_future(LONG).await;
    glib::timeout_future(LONG).await;
    check(failures, shows(window, "Running.").await, "the stand-in machine runs");
    check(failures, !text_of(window).contains("app window"), "no viewer, nothing said of app windows");
    check(
        failures,
        !all_of::<gtk::Button>(window).iter().any(|b| b.label().as_deref() == Some("Open Another App")),
        "no chips and no button with nothing open",
    );
    shot_after_paint(window, folder, &shots("1-none")).await;

    // A stale file with no viewer holding the lock counts for nothing.
    open_windows::write_to(&run, &[open_program("Notepad", "notepad.exe", 1, None, folder)]).unwrap();
    glib::timeout_future(LONG).await;
    check(
        failures,
        !text_of(window).contains("app window"),
        "a file left by a viewer that is gone is not shown",
    );
    open_windows::remove_from(&run).unwrap();

    let lock = viewer_lock::take_in(&run).unwrap().expect("no viewer holds the lock here");
    let edge = r"C:\Program Files\Edge\msedge.exe";
    let say = |programs: Vec<OpenProgram>| open_windows::write_to(&run, &programs).unwrap();
    say(vec![open_program("Notepad", "notepad.exe", 1, Some(90), folder)]);
    check(failures, shows(window, "1 app window open.").await, "one window is said in the singular");
    shot_after_paint(window, folder, &shots("2-one")).await;
    shot_after_paint_of(&card_of(window), folder, &shots("2-one-card")).await;

    say(vec![
        open_program("Microsoft Edge", edge, 2, Some(30), folder),
        open_program("Notepad", "notepad.exe", 1, Some(90), folder),
        open_program("Paint", "mspaint.exe", 3, None, folder),
    ]);
    check(failures, shows(window, "6 app windows open.").await, "windows are counted over the programs");
    shot_after_paint(window, folder, &shots("3-three")).await;
    shot_after_paint_of(&card_of(window), folder, &shots("3-three-card")).await;

    let many: Vec<OpenProgram> = (0..14u8)
        .map(|n| {
            let name = format!("Program number {n}");
            open_program(&name, &format!("p{n}.exe"), u32::from(n % 3) + 1, Some(n * 17), folder)
        })
        .collect();
    say(many);
    glib::timeout_future(LONG).await;
    glib::timeout_future(LONG).await;
    shot_after_paint(window, folder, &shots("4-long-row")).await;
    shot_after_paint_of(&card_of(window), folder, &shots("4-long-row-card")).await;

    window.widget().set_default_size(420, 760);
    glib::timeout_future(LONG).await;
    shot_after_paint(window, folder, &shots("5-narrow")).await;
    shot_after_paint_of(&card_of(window), folder, &shots("5-narrow-card")).await;
    window.widget().set_default_size(1100, 760);
    glib::timeout_future(LONG).await;

    // The viewer ends: the file goes with it, and so do the chips.
    open_windows::remove_from(&run).unwrap();
    drop(lock);
    glib::timeout_future(LONG).await;
    check(failures, !text_of(window).contains("app window"), "the chips go when the viewer ends");

    let lock = viewer_lock::take_in(&run).unwrap().unwrap();
    say(vec![open_program("Notepad", "notepad.exe", 1, Some(90), folder)]);
    check(failures, shows(window, "1 app window open.").await, "a viewer that comes again is shown");
    check(failures, click(window, "Open Another App"), "Open Another App can be pressed");
    check(
        failures,
        shows(window, "Show Apps as Their Own Windows").await,
        "Open Another App opens the Apps page of the machine",
    );
    close_dialogs(window).await;
    open_windows::remove_from(&run).unwrap();
    drop(lock);
    stop(&mut vm);
    glib::timeout_future(LONG).await;
    glib::timeout_future(LONG).await;
}

fn card_of(window: &Window) -> gtk::Widget {
    let boxes = all_of::<gtk::Box>(window);
    boxes.iter().find(|b| b.has_css_class("machine-card")).expect("the card").clone().upcast()
}

pub async fn walk(window: Window, context: Arc<Context>, folder: PathBuf) {
    let failures = Rc::new(Cell::new(0));
    glib::timeout_future(LONG).await;
    close_dialogs(&window).await;
    let machine: MachineHandle = {
        let found = guesthouse_app_core::machine::load_all(&context)
            .0
            .into_iter()
            .find(|machine| machine.id == "win");
        window.add_machine(found.expect("the stand-in machine was saved"))
    };
    glib::timeout_future(LONG).await;
    let shots = |name: &str| format!("apps-{name}");
    card_states(&window, &context, &machine, &folder, &failures).await;
    window.show_settings(machine.clone());
    glib::timeout_future(LONG).await;
    check(&failures, text_of(&window).contains("Apps"), "a Windows machine has an Apps page");
    show_page(&window, "apps");
    glib::timeout_future(SETTLE).await;
    check(&failures, settle_on(&window, "Show Apps as Their Own Windows").await, "the switch is on the page");
    check(
        &failures,
        text_of(&window).contains("App windows is off for this machine"),
        "the switch is off at first",
    );
    shot_after_paint(&window, &folder, &shots("01-switch-off")).await;

    // On, with the machine off and nothing in the launcher.
    switch(&window, true);
    check(
        &failures,
        settle_on(&window, "and no program of it is in the launcher yet").await,
        "off, no entries says so",
    );
    shot_after_paint(&window, &folder, &shots("02-machine-off-empty")).await;

    // The launcher's programs while the machine is off.
    programs_in_launcher(&context, &machine.borrow());
    window.open_settings().unwrap().set_status(&Status::default());
    close_dialogs(&window).await;
    window.show_settings(machine.clone());
    glib::timeout_future(LONG).await;
    show_page(&window, "apps");
    check(
        &failures,
        settle_on(&window, "The full list shows when the machine runs.").await,
        "off, the full list waits for a run",
    );
    check(&failures, text_of(&window).contains("2 in the Linux launcher"), "the launcher's count is shown");
    shot_after_paint(&window, &folder, &shots("03-machine-off-launcher")).await;

    // Starting, as the dialog is told by the window.
    let settings = window.open_settings().unwrap();
    settings.set_status(&Status { state: State::Starting, ..Status::default() });
    // The window's own refresh puts the real, off status back, so this says it again until the shot.
    let holding = settings.clone();
    let hold = glib::timeout_add_local(Duration::from_millis(50), move || {
        holding.set_status(&Status { state: State::Starting, ..Status::default() });
        glib::ControlFlow::Continue
    });
    check(&failures, settle_on(&window, "is starting").await, "a machine that starts says so");
    shot_after_paint(&window, &folder, &shots("04-starting")).await;
    hold.remove();
    settings.set_status(&Status { state: State::Installing, ..Status::default() });
    check(&failures, settle_on(&window, "still being installed").await, "an install says so");
    settings.set_status(&Status::default());

    // Running with the way in, and a list.
    lists(programs(31, 0));
    helper().with(|inner| {
        inner.layout = Layout {
            seq: 3,
            screen: Size { w: 1920, h: 1080 },
            cells: vec![
                cell("1", r"C:\Program Files\Edge\msedge.exe", false),
                cell("2", r"C:\Program Files\Edge\msedge.exe", false),
                cell("3", r"C:\Program Files\Edge\msedge.exe", true),
                cell("4", r"C:\Windows\notepad.exe", false),
            ],
        }
    });
    let mut vm = vm_with(&context, &machine.borrow(), true);
    let listed = settle_on(&window, "Open now, 2 windows").await;
    check(&failures, listed, "a running machine lists its programs with their open windows");
    let text = text_of(&window);
    check(
        &failures,
        text.contains("Open now, 1 window") && text.contains("Not running"),
        "windows are counted",
    );
    check(
        &failures,
        text.contains("31 more, such as uninstallers and help files"),
        "minor programs are folded",
    );
    check(&failures, !text.contains("Uninstall Tool 3"), "an uninstaller is not shown at first");
    shot_after_paint(&window, &folder, &shots("05-list")).await;

    // Show all.
    check(&failures, click(&window, "Show All"), "Show all can be pressed");
    glib::timeout_future(SETTLE).await;
    check(&failures, text_of(&window).contains("Uninstall Tool 3"), "Show all shows the minor ones");
    scroll_to(&window, "Programs Found in Windows");
    shot_after_paint(&window, &folder, &shots("06-list-all")).await;

    // An Open in flight, which the helper never answers.
    check(&failures, click_first(&window, "Open"), "Open can be pressed");
    glib::timeout_future(Duration::from_millis(1200)).await;
    let busy = all_of::<gtk::Button>(&window)
        .iter()
        .any(|button| !button.is_sensitive() && button.is_mapped() && button.label().is_none());
    check(&failures, busy, "the button of an Open in flight is busy");
    shot_after_paint(&window, &folder, &shots("07-open-busy")).await;
    check(
        &failures,
        shows(&window, "did not answer in time").await,
        "an Open that gets no answer fails and says so",
    );
    shot_after_paint(&window, &folder, &shots("08-open-failed")).await;

    // A machine that was started with the switch off, and then has it turned on.
    stop(&mut vm);
    glib::timeout_future(LONG).await;
    glib::timeout_future(LONG).await;
    vm = vm_with(&context, &machine.borrow(), false);
    check(
        &failures,
        settle_on(&window, "started with app windows off").await,
        "a late switch says it counts from the next start",
    );
    shot_after_paint(&window, &folder, &shots("09-next-start")).await;
    stop(&mut vm);
    glib::timeout_future(LONG).await;
    glib::timeout_future(LONG).await;

    // The request failing, with a retry.
    refuses("the Start menu is locked");
    vm = vm_with(&context, &machine.borrow(), true);
    check(
        &failures,
        settle_on(&window, "the Start menu is locked").await,
        "a refusal is shown with the helper's words",
    );
    check(
        &failures,
        text_of(&window).contains("Try Again")
            || all_of::<gtk::Button>(&window).iter().any(|b| b.label().as_deref() == Some("Try Again")),
        "a failure has Try again",
    );
    shot_after_paint(&window, &folder, &shots("10-failed")).await;
    lists("[]".to_owned());
    launcher::remove_all(&context, &machine.borrow());
    check(&failures, click(&window, "Try Again"), "Try again can be pressed");
    check(&failures, settle_on(&window, "has no programs in its Start menu").await, "an empty list says so");
    shot_after_paint(&window, &folder, &shots("11-empty")).await;

    // Nobody logged in.
    helper().with(|inner| inner.welcome = Welcome::Close);
    click(&window, "Try Again");
    switch(&window, false);
    glib::timeout_future(SETTLE).await;
    switch(&window, true);
    check(
        &failures,
        settle_on(&window, "Nobody is logged in").await,
        "a guest with nobody logged in says so and waits",
    );
    shot_after_paint(&window, &folder, &shots("12-not-yet")).await;
    helper().with(|inner| inner.welcome = Welcome::Hello);

    // A long list.
    lists(programs(5, 600));
    let began = std::time::Instant::now();
    switch(&window, false);
    glib::timeout_future(SETTLE).await;
    switch(&window, true);
    let long = settle_on(&window, "Program number 000").await;
    println!("long list drawn in about {:?}", began.elapsed());
    check(&failures, long, "a list of hundreds is shown");
    shot_after_paint(&window, &folder, &shots("13-long")).await;

    // The same states in a narrow window.
    window.widget().set_default_size(420, 760);
    glib::timeout_future(LONG).await;
    let rows = all_of::<gtk::ListBoxRow>(&window);
    if let Some(row) = rows.iter().find(|row| row.widget_name() == "apps") {
        row.activate();
    }
    glib::timeout_future(LONG).await;
    shot_after_paint(&window, &folder, &shots("14-narrow-list")).await;
    stop(&mut vm);
    glib::timeout_future(LONG).await;
    glib::timeout_future(LONG).await;
    shot_after_paint(&window, &folder, &shots("15-narrow-machine-off")).await;
    switch(&window, false);
    glib::timeout_future(SETTLE).await;
    shot_after_paint(&window, &folder, &shots("16-narrow-switch-off")).await;
    println!("failures: {}", failures.get());
    std::process::exit(i32::try_from(failures.get()).unwrap_or(1));
}
