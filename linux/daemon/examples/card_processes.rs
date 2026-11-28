//! `card_processes ADDRESS [--jailed] [ROUNDS]`: reads each process's part of an NVIDIA card a few
//! times, a second apart, through the same library calls and table the card's sensors publisher uses,
//! and prints the table as the app would read it. It only reads the card, and writes no shared file.
//!
//! With `--jailed` it first puts itself in the daemon's jail, Landlock and the call filter both, with
//! a scratch folder in place of a run folder, so a call the jail refuses shows as a line on stderr
//! and as a table with nothing read. The card's address is its PCI address, as `0000:01:00.0`.

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use guesthouse_backend_nvidia::sensors::{ProcessSource, process_source, table};
    use guesthouse_daemon::jail::{Needs, host_gpu_devices, lock_calls, lock_files};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const BYTES_PER_MIB: u64 = 1 << 20;

    let words: Vec<String> = std::env::args().skip(1).collect();
    let card = words.iter().find(|word| word.contains(':')).ok_or("name the card by its address")?;
    let jailed = words.iter().any(|word| word == "--jailed");
    let rounds: u32 = words.iter().find_map(|word| word.parse().ok()).unwrap_or(3);
    if jailed {
        let scratch = std::env::temp_dir().join(format!("gh-card-processes-{}", std::process::id()));
        let needs = Needs {
            run_folder: scratch.join("run"),
            shared_files: vec![scratch.join("table")],
            devices: host_gpu_devices(),
            driver_folder: None,
            read_files: Vec::new(),
        };
        std::fs::create_dir_all(&scratch)?;
        needs.prepare()?;
        let locked = lock_files(&needs)?;
        lock_calls(locked)?;
        println!("jailed as the daemon is, in {}", scratch.display());
    }
    let mut source = process_source(card).ok_or("the management library can't be loaded for that card")?;
    for round in 0..rounds {
        if round > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        let started = Instant::now();
        let samples = source.samples_since(0);
        let processes = source.processes();
        let took = started.elapsed();
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64;
        let read = table(&samples, processes.as_deref(), now);
        println!(
            "look {round}: {took:.1?}, samples {}, process lists {}",
            if read.samples_read { "read" } else { "not read" },
            if read.processes_read { "read" } else { "not read" }
        );
        for process in &read.processes {
            let used = match (process.used, read.samples_read) {
                (Some(used), _) => format!("sm {}% enc {}% dec {}%", used.sm, used.encoder, used.decoder),
                (None, true) => "no use in its last second".to_owned(),
                (None, false) => "use not read".to_owned(),
            };
            let memory = process
                .bytes
                .map_or("memory not read".to_owned(), |bytes| format!("{} MiB", bytes / BYTES_PER_MIB));
            println!("  pid {:>8}  {used:<28} {memory}", process.pid);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("card_processes runs on Linux only.");
    std::process::exit(1);
}
