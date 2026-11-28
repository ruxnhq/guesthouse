// SPDX-License-Identifier: GPL-2.0-or-later
//! Plays a guest in memory of this process's own up to a picture on the display, and reads the
//! picture back: `cargo run -p guesthouse-pvg-sys --example present -- 1920 1080 picture.ppm`.
//! The size and the file may be left out.

#[cfg(target_os = "macos")]
#[path = "../tests/guest/mod.rs"]
#[allow(dead_code)]
mod guest;

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let width = arguments.first().map_or(Ok(1920), |text| text.parse())?;
    let height = arguments.get(1).map_or(Ok(1080), |text| text.parse())?;

    guesthouse_pvg_sys::watch_reasons(|reason| println!("the framework refused: {reason}"));
    let mut guest = guest::Guest::new();
    let frame = guest::present(&mut guest, width, height).ok_or("the display has no picture")?;

    let mut colours = std::collections::BTreeMap::new();
    for pixel in frame.pixels.as_chunks::<4>().0 {
        *colours.entry(u32::from_le_bytes(*pixel)).or_insert(0u32) += 1;
    }
    println!("a picture of {} by {}, {} interrupts", frame.width, frame.height, guest.interrupts());
    for (colour, pixels) in &colours {
        println!("   {colour:#010x} on {pixels} pixels");
    }
    for refusal in guest.refusals() {
        println!("the guard refused: {refusal}");
    }
    if let Some(path) = arguments.get(2) {
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        write!(file, "P6\n{} {}\n255\n", frame.width, frame.height)?;
        for [blue, green, red, _] in frame.pixels.as_chunks::<4>().0 {
            file.write_all(&[*red, *green, *blue])?;
        }
        println!("written to {path}");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("this example calls Apple's framework, and runs on a Mac only");
}
