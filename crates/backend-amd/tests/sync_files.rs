//! A run on one open of the card waiting for a run on another, which is how the daemon's copy to the
//! screen waits on the card for a guest's frame. Its own file, so it doesn't run beside the timing
//! checks in igpu.rs on the same engine. Without an AMD card it returns early.

#![cfg(target_os = "linux")]

use guesthouse_backend_amd::{
    Context, CpuAccess, DOMAIN_GTT, Device, HW_IP_DMA, Ib, MapFlags, MemoryType, Syncobj,
};

const PAGE: u64 = 4096;

fn card() -> Option<Device> {
    Device::open(None).ok()
}

/// A run on one open of the card held until a run on another has retired: the second waits on a
/// sync file of the first's sync object, as the daemon's copy to the screen waits on a guest's run.
#[test]
fn a_run_waits_on_the_card_for_a_run_on_another_open() {
    const FIRST: u32 = 0x1111_2222;
    const SECOND: u32 = 0x3333_4444;
    let Some(device) = card() else {
        eprintln!("no AMD render node; skipping the cross-open wait");
        return;
    };
    let info = device.info().clone();
    let other = device.open_again().unwrap();
    let align = info.va_alignment.max(PAGE);
    let rwx =
        MapFlags { readable: true, writable: true, executable: true, memory_type: MemoryType::Uncached };
    let make = |file: &std::fs::File| {
        let ib = guesthouse_backend_amd::create_buffer(file, PAGE, align, DOMAIN_GTT, CpuAccess::Required);
        let out = guesthouse_backend_amd::create_buffer(file, PAGE, align, DOMAIN_GTT, CpuAccess::Required);
        (ib.unwrap(), out.unwrap())
    };
    let first_file = device.open_again().unwrap();
    let (ib_va, out_va) = (info.va_start + 0x700_000, info.va_start + 0x800_000);
    let mut runs = Vec::new();
    for (file, word) in [(&first_file, FIRST), (&other, SECOND)] {
        let (ib, out) = make(file);
        guesthouse_backend_amd::map_buffer(file, ib, ib_va, 0, PAGE, rwx).unwrap();
        guesthouse_backend_amd::map_buffer(file, out, out_va, 0, PAGE, rwx).unwrap();
        let mut ib_cpu = guesthouse_backend_amd::Mapping::of(file, ib, PAGE as usize).unwrap();
        let mut out_cpu = guesthouse_backend_amd::Mapping::of(file, out, PAGE as usize).unwrap();
        // Many words, so the first run isn't over at once.
        let packet = guesthouse_backend_amd::sdma_write_ib(out_va, &[word; 900]);
        ib_cpu.write_dwords(&packet);
        out_cpu.write_dwords(&[0; 900]);
        let ib = Ib { address: ib_va, bytes: (packet.len() * 4) as u32 };
        runs.push((ib, out_cpu));
    }
    let first_context = Context::create(&first_file).unwrap();
    let first_done = Syncobj::create(&first_file).unwrap();
    guesthouse_backend_amd::submit_all(
        &first_file,
        &first_context,
        HW_IP_DMA,
        &[runs[0].0],
        &[],
        &first_done,
    )
    .unwrap();
    let sync_file = first_done.export_sync_file(&first_file).unwrap();
    let wait = Syncobj::from_sync_file(&other, &sync_file).unwrap();
    let context = Context::create(&other).unwrap();
    let done = Syncobj::create(&other).unwrap();
    guesthouse_backend_amd::submit_after(&other, &context, HW_IP_DMA, &[runs[1].0], &[], &[wait], &done)
        .unwrap();
    let event = done.arm_eventfd(&other).unwrap();
    assert!(guesthouse_backend_amd::wait_eventfd(&event, 2000).unwrap(), "did not complete");
    assert!(first_done.signalled(&first_file).unwrap(), "the run it waited for retired first");
    assert_eq!(runs[0].1.read_u32(899 * 4), FIRST);
    assert_eq!(runs[1].1.read_u32(899 * 4), SECOND);
}
