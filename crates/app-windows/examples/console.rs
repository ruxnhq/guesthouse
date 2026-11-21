//! Opens a machine's console in a window of its own, without the app, and prints what it says, to
//! try the console against a running machine:
//!
//! ```text
//! cargo run -p guesthouse-app-windows --example console -- win 30
//! cargo run -p guesthouse-app-windows --example console -- \\.\pipe\guesthouse-win.BasicSession 30 who
//! ```
//!
//! A pipe's path is taken as it is. With `who`, the pipe is opened once first to say which process
//! serves it, which takes one of its connections.
//!
//! The window shows without taking the keyboard, and closes after the given seconds (20 unless
//! said). If the session ends, it connects once more, as the app's Reconnect does. It only shows
//! the guest's screen: nothing is asked of the service.

// The app uses all of it, and this only some.
#[cfg(windows)]
#[allow(dead_code)]
#[path = "../src/console.rs"]
mod console;

#[cfg(not(windows))]
fn main() {
    eprintln!("The console is Windows' Remote Desktop control, so this runs only on Windows.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "win".to_owned());
    let seconds: u64 = args.next().and_then(|text| text.parse().ok()).unwrap_or(20);
    // A path is taken as it is, so a pipe that isn't there can be tried too.
    let pipe = if name.starts_with(r"\\") { name.clone() } else { guesthouse_control::console_pipe(&name) };
    if args.next().as_deref() == Some("who") {
        // Takes one of the pipe's connections, so only when asked.
        say_who_serves(&pipe);
    }
    let started = Instant::now();
    let (sender, events) = mpsc::channel();
    let console = console::Console::open(&pipe, &format!("{name} — Console"), true, move |event| {
        let _ = sender.send(event);
    })
    .expect("the console's thread didn't start");
    println!("{:6.2}s opening {pipe}", 0.0);
    let mut told_hwnd = false;
    let mut reconnected = false;
    let until = started + Duration::from_secs(seconds);
    while Instant::now() < until {
        if !told_hwnd && console.hwnd() != 0 {
            told_hwnd = true;
            println!("{:6.2}s window {:#x}", started.elapsed().as_secs_f64(), console.hwnd());
        }
        match events.recv_timeout(Duration::from_millis(100)) {
            Ok(console::Event::Closed) => {
                println!("{:6.2}s closed", started.elapsed().as_secs_f64());
                return;
            }
            Ok(event) => {
                println!("{:6.2}s {event:?}", started.elapsed().as_secs_f64());
                // Once, as the app's Reconnect does: a control can't connect twice, so it's made anew.
                if matches!(event, console::Event::Disconnected { .. }) && !reconnected {
                    reconnected = true;
                    console.reconnect();
                }
            }
            Err(_) => {}
        }
    }
    // As the app does when a machine stops: end the session, then close.
    console.disconnect();
    console.close();
    for event in events.iter() {
        println!("{:6.2}s {event:?}", started.elapsed().as_secs_f64());
        if event == console::Event::Closed {
            break;
        }
    }
}

/// Opens the pipe once, says which process serves it, and closes it again.
#[cfg(windows)]
fn say_who_serves(pipe: &str) {
    use std::os::windows::io::AsRawHandle;
    match std::fs::OpenOptions::new().read(true).write(true).open(pipe) {
        Ok(file) => {
            let mut process = 0u32;
            // SAFETY: the handle is the open pipe's, for the call's length, and `process` is written.
            let known = unsafe {
                windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId(
                    file.as_raw_handle(),
                    &mut process,
                )
            };
            if known != 0 {
                println!("{pipe} is served by process {process}");
            } else {
                println!("{pipe} opened, but its server isn't known: {}", std::io::Error::last_os_error());
            }
        }
        Err(error) => println!("{pipe} didn't open: {error}"),
    }
}
