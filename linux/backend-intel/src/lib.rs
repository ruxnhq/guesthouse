//! The Intel backend: the host's open xe driver, and the private data Intel's Windows driver passes
//! to its kernel driver.
//!
//! It reads the card through sysfs and xe's read-only queries, builds the adapter record the
//! guest's user-mode drivers ask for, and answers the guest so that they can make devices, contexts,
//! queues and allocations and submit. Every exchange that is Intel's own gets a line in the log, in
//! [`traffic`].
//!
//! What becomes of the guest's work is the [`stage`]'s word. While listening, nothing reaches the
//! card: a submission comes back finished, unrun, and video memory is only a name. Past that,
//! [`work`] makes real buffers, address spaces and queues through [`xe`] and runs the guest's
//! batches. That has run against a stand-in for xe only, never on a card, so the backend tests the
//! card itself at each start and stays listening if any of it fails.
//!
//! The guest-protocol side, in [`protocol`], is plain byte work tested without a guest. The host
//! side is [`card`] and the modules under it, and `intel-probe` prints what it reads.
//!
//! Where the trait does not fit Intel:
//! - A submission's batch address and length are in the submission message, not in Intel's private
//!   data. [`Work`] carries them for a hardware queue's submission, and not yet for a context's.
//! - Escapes have no status of their own, so one this backend's rules refuse can only be logged.
//! - [`Backend::finish_queue_data`] sees the queue's data with the message's padding after it, so
//!   its true size can't be checked.
//! - The core describes every node but the first as a copy engine, whatever gmmlib's numbering
//!   says the node is.

// Linux only: it talks to xe through ioctl. Other systems get an empty crate.
#![cfg(target_os = "linux")]

// The shared test helpers name this crate as a test outside it does.
#[cfg(test)]
extern crate self as guesthouse_backend_intel;

mod books;
pub mod card;
pub mod generation;
pub mod hwconfig;
pub mod ioctl;
#[cfg(feature = "lab-fake-card")]
mod lab;
pub mod protocol;
pub mod recording;
mod sensors;
pub mod stage;
pub mod sysfs;
mod traffic;
mod watch;
pub mod work;
pub mod xe;
pub mod xe_query;

use books::Books;
use card::CardFacts;
use generation::Generation;
use guesthouse_backend::guest_text::QUERY_USER_DRIVER_NAME;
use guesthouse_backend::{
    AdapterFacts, AllocationShape, Backend, BackendError, COPY_PIXEL_BYTES, CardLabel, Doorbell, DriverFound,
    DriverSurvey, GpuRange, GuestBooks, HostServices, MapRequest, MemoryId, MemoryKind, PciCard, Refusal,
    RegistryName, RegistryValue, Setup, SlotCopy, SpaceId, StagingCopy, StagingStart, Status, SurfaceShape,
    UnmapRequest, VendorQuery, Work, WorkSource, WorkStart,
};
use hwconfig::Hwconfig;
pub use ioctl::{INTEL_VENDOR, intel_render_node};
use protocol::allocation::Resource;
use protocol::command_header::Header;
use protocol::context::Context;
use protocol::escape::Escape;
use protocol::media_escape::{MediaEngines, MediaEscapes};
use protocol::record::{
    EngineMaxima, GtSystem, IpWord, Memory, Platform, RecordFacts, StageThreads, Urb, UrbEntries,
};
use protocol::{gaps, guest_driver, record, shape};
use stage::{Desktop, Fallback, Stage};
use std::os::fd::OwnedFd;
use std::time::Instant;
use traffic::{QueryOutcome, Traffic};
use watch::PREFIX;
use work::CardWork;
use work::blits::Blit;
use work::copies::{Area, Rows, colours_turn};
use xe::abi::Keying;
use xe::device::Xe;
use xe::plan::BufferUse;
use xe_query::{Engine, Gt};

/// How this backend names the host driver's branch: the Linux kernel's xe.
const HOST_BRANCH: &str = "xe";
/// The part of the system's memory offered as shared memory, as Windows gives a card half of the
/// memory as shared. An integrated card has no other, so for it this is the video memory too.
const SHARED_MEMORY_PART: u64 = 2;

/// See the crate documentation.
#[derive(Debug)]
pub struct IntelBackend {
    card: CardFacts,
    generation: &'static Generation,
    facts: AdapterFacts,
    record: Vec<u8>,
    /// How the record's parts that come from data were built, for the log.
    telling: record::Telling,
    books: Books,
    traffic: Traffic,
    started: Instant,
    /// See [`IntelBackend::set_desktop`].
    desktop: Desktop,
    /// Work on the card, at every stage past listening. `None` is the listening stage: memory and
    /// spaces are [`Books`]' names, and every submission is finished unrun.
    work: Option<CardWork>,
    /// Why the guest is left listening, when it is.
    listening: Option<Fallback>,
    /// What the media driver's escapes keep for the guest: its reset count, and the engines its
    /// video streams hold.
    media: MediaEscapes,
    /// The card's video engines a video stream can be given, by node.
    media_engines: MediaEngines,
}

impl IntelBackend {
    /// The backend for a card whose facts are already read, which is how a test makes one. Refuses
    /// a card whose graphics has no profile.
    pub fn from_card(card: CardFacts) -> Result<Self, Refusal> {
        let ip = card.main_gt().map(|gt| gt.ip).unwrap_or_default();
        let generation = generation::for_card(ip, card.pci.device).ok_or_else(|| {
            Refusal(format!(
                "This Intel card, device {:04x} with graphics IP {ip}, is of a generation this build has no \
                 profile for. Run `intel-probe --record card.txt` and send card.txt with the probe's output, so \
                 one can be made.",
                card.pci.device
            ))
        })?;
        // compute-runtime works out threads per EU by dividing the thread count by the EU count, so a
        // record with none would stop it at the start, or worse.
        if card.eu_count() == 0 {
            return Err(Refusal(
                "The xe driver reported no EUs for this Intel card, so the guest can't be told its shape."
                    .into(),
            ));
        }
        let shared = card.system_memory() / SHARED_MEMORY_PART;
        // No queue exists yet, so the render cache is keyed as xe keys it until work starts.
        let (record, telling) =
            record::build_telling(&record_facts(&card, generation, shared, Keying::BindingIndex));
        // A card with memory of its own offers that; one without offers its share of the system's.
        let own = card.card_memory();
        let video_memory = if own > 0 { own } else { shared };
        let reachable = card.reachable_card_memory();
        let bar_size = if reachable > 0 { reachable } else { video_memory };
        let facts = AdapterFacts {
            card: PciCard { vendor: INTEL_VENDOR, device: card.pci.device, revision: card.pci.revision },
            name: adapter_name(&card),
            video_memory,
            gtt_memory: 0,
            bar_size,
            node_count: protocol::nodes::COUNT,
            engines: card.engine_families(),
            guest_driver_version: 0,
        };
        let media_engines = MediaEngines::of(|node| work::engines::for_node(&card.engines, node).is_some());
        let backend = Self {
            card,
            generation,
            facts,
            record,
            telling,
            books: Books::default(),
            traffic: Traffic::default(),
            started: Instant::now(),
            desktop: Desktop::Hardware,
            work: None,
            listening: None,
            media: MediaEscapes::default(),
            media_engines,
        };
        backend.log_start();
        Ok(backend)
    }

    /// Where the guest's desktop is drawn, which decides the drivers each program is given: see
    /// [`stage`]. The daemon sets the stage's own unless a person asks otherwise.
    /// A hardware desktop asked for at a stage that doesn't run everything is given as the software
    /// desktop: see [`Desktop::at_stage`]. Returns the note for the app ([`Desktop::note`]): why,
    /// when that happens, or that nothing runs on a software desktop asked for while the stage
    /// listens. Set it after the stage.
    pub fn set_desktop(&mut self, desktop: Desktop) -> Option<String> {
        let lab_kept = stage::lab_keeps_hardware_desktop(
            std::env::var_os(stage::LAB_HARDWARE_DESKTOP_VARIABLE).as_deref(),
            recording::recorded_card_file().is_some(),
        );
        let (given, why) = desktop.at_stage(self.stage(), self.listening.as_ref(), lab_kept);
        self.desktop = given;
        match (given, &why) {
            (Desktop::Software, Some(why)) => log::warn!(
                "{PREFIX} software-desktop on, though the hardware desktop was asked: {why}. Nothing the \
                 compositor sends would run, so it could never draw"
            ),
            (Desktop::Software, None) => log::info!(
                "{PREFIX} software-desktop on: every Direct3D driver refused, 9 to 12, and every driver to the compositor"
            ),
            (Desktop::Hardware, _) if lab_kept && self.stage() != Stage::Everything => log::warn!(
                "{PREFIX} software-desktop off on a recorded card, as {} asks: a lab run to see the hardware \
                 desktop fail, as nothing it draws runs",
                stage::LAB_HARDWARE_DESKTOP_VARIABLE
            ),
            (Desktop::Hardware, _) => log::info!("{PREFIX} software-desktop off: every driver given"),
        }
        Desktop::note(given, why, self.stage(), self.listening.as_ref())
    }

    pub fn card(&self) -> &CardFacts {
        &self.card
    }

    pub fn generation(&self) -> &'static Generation {
        self.generation
    }

    /// Blocks of memory still held, for a test to see that frees reach the backend.
    pub fn live_memory_blocks(&self) -> usize {
        self.work.as_ref().map_or_else(|| self.books.live_memory(), CardWork::live_blocks)
    }

    /// The adapter record the guest is given, before its build stamp is put in.
    pub fn record(&self) -> &[u8] {
        &self.record
    }

    /// How every queue on a render engine has its colour cache keyed: decided once when work
    /// starts, from what xe offers and what the self-test proved, and followed by both the queues
    /// and the record. Keyed by index alone, as xe keys it, while nothing runs on the card.
    pub fn render_keying(&self) -> Keying {
        self.work.as_ref().map_or(Keying::BindingIndex, CardWork::render_keying)
    }

    /// Builds the record again for the render keying work now has, so the two can't disagree.
    fn follow_render_keying(&mut self) {
        let shared = self.card.system_memory() / SHARED_MEMORY_PART;
        let facts = record_facts(&self.card, self.generation, shared, self.render_keying());
        (self.record, self.telling) = record::build_telling(&facts);
    }

    fn log_start(&self) {
        let card = &self.card;
        log::info!(
            "{PREFIX} card pci={:04x}:{:04x} rev={:#04x} subsystem={:04x}:{:04x} address={} driver={} kernel={} \
             profile=\"{}\" render_ip={} eus={} system_memory={:#x} card_memory={:#x}",
            card.pci.vendor,
            card.pci.device,
            card.pci.revision,
            card.pci.subsystem_vendor,
            card.pci.subsystem_device,
            card.address.as_deref().unwrap_or("-"),
            card.driver,
            card.kernel.as_deref().unwrap_or("-"),
            self.generation.name,
            card.main_gt().map(|gt| gt.ip).unwrap_or_default(),
            card.eu_count(),
            card.system_memory(),
            card.card_memory()
        );
        for (what, error) in &card.unanswered {
            log::warn!("{PREFIX} card query=\"{what}\" unanswered=\"{error}\"");
        }
        for guess in self.generation.guesses {
            log::info!("{PREFIX} profile-guess \"{guess}\"");
        }
        log::info!("{PREFIX} record {}", record::summary(&self.record));
        log::info!("{PREFIX} media-engines {}", self.media_engines);
        let urb = record::urb(&self.record);
        log::info!("{PREFIX} record-urb {urb}");
        if urb.vertex.max == 0 {
            log::warn!("{PREFIX} record-urb-unknown \"the card's hwconfig gives no URB entry limits\"");
        }
        match &self.telling.card_data {
            record::CardData::Built(built) => log::info!("{PREFIX} record-features {built}"),
            record::CardData::Uncovered { device, revision, feature_bits } => log::warn!(
                "{PREFIX} record-features-uncovered device={device:#06x} pci_rev={revision:#04x} bits={feature_bits} \
                 \"the kernel driver's data has no feature table for this card, so the feature table, platform \
                 words and hwconfig fields are this backend's own\""
            ),
        }
        match &self.telling.workarounds {
            protocol::workarounds::Outcome::Built(built) => {
                log::info!("{PREFIX} record-workarounds {built}");
                for part in &built.uncovered {
                    log::warn!(
                        "{PREFIX} record-workarounds-partial part=\"{part}\" \"the workaround table has no data for \
                         this part, so its entries are left out and the rest are set\""
                    );
                }
            }
            protocol::workarounds::Outcome::Uncovered(ip) => log::warn!(
                "{PREFIX} record-workarounds-uncovered ip=\"{ip}\" \"the workaround table has no data for the \
                 render IP, so only the entries this backend sets are set\""
            ),
        }
        log::info!("{PREFIX} record-partition [{}]", protocol::partition::Text(protocol::partition::ours()));
        let from_data = matches!(self.telling.card_data, record::CardData::Built(_));
        for gap in gaps::unset(shape::filled(&self.record), from_data) {
            log::info!("{PREFIX} record-gap \"{gap}\"");
        }
        if self.card.config.has_vram() {
            log::info!("{PREFIX} record-gap \"{}\"", gaps::LOCAL_MEMORY);
        }
    }

    /// How much of the guest's work runs on the card.
    pub fn stage(&self) -> Stage {
        self.work.as_ref().map_or(Stage::Listen, CardWork::stage)
    }

    /// Starts work on the card at the stage asked for, or stays listening and says in the log which
    /// layer refused. Nothing here fails the guest's start: a card that can't run work is still
    /// one whose guest can be listened to. `open` opens the card for work, and `sysfs` is the
    /// card's device folder.
    pub fn start_work_on_card(
        &mut self,
        asked: (Stage, Option<Fallback>),
        open: impl FnOnce() -> std::io::Result<Box<dyn Xe + Send>>,
        sysfs: Option<&std::path::Path>,
    ) {
        match self.work_at(asked, open, sysfs) {
            Ok(work) => {
                self.listening = None;
                log::info!(
                    "{PREFIX} stage={:?}: the guest's work runs on the card, with at most {} MiB of video \
                     memory; {}=0 turns that off",
                    work.stage(),
                    work.cap() >> 20,
                    stage::RUN_VARIABLE
                );
                self.work = Some(work);
                self.follow_render_keying();
                self.log_render_keying();
            }
            Err(fallback) => {
                self.work = None;
                self.follow_render_keying();
                log::warn!(
                    "{PREFIX} stage=Listen {fallback}. Nothing reaches the card: the guest's work is \
                     finished unrun"
                );
                self.listening = Some(fallback);
            }
        }
    }

    fn log_render_keying(&self) {
        let entry = record::RENDER_CACHE_KEYING_WORKAROUND;
        match self.render_keying() {
            Keying::BindingTableAndIndex => log::info!(
                "{PREFIX} render-cache keyed=table: every render queue's colour cache is keyed by binding \
                 table and index, as Intel's kernel driver keys it, and the record keeps workaround entry \
                 {entry} as the card's table gives it"
            ),
            Keying::BindingIndex => {
                let why = if self.card.config.offers_cache_keying() {
                    "the self-test didn't prove a render queue keyed by binding table on this card"
                } else {
                    "this kernel's xe doesn't offer it (Linux v7.1 and later do)"
                };
                log::info!(
                    "{PREFIX} render-cache keyed=index: {why}, so render queues are keyed by binding index \
                     alone, as xe makes them, and the record keeps workaround entry {entry} clear: the \
                     guest's driver flushes on each binding change itself"
                );
            }
        }
    }

    fn work_at(
        &mut self,
        (asked, why): (Stage, Option<Fallback>),
        open: impl FnOnce() -> std::io::Result<Box<dyn Xe + Send>>,
        sysfs: Option<&std::path::Path>,
    ) -> Result<CardWork, Fallback> {
        if !asked.is_real() {
            return Err(why.unwrap_or(Fallback::AskedFor));
        }
        if self.card.driver != ioctl::XE_DRIVER {
            return Err(Fallback::NotXe(self.card.driver.clone()));
        }
        let mut xe = open().map_err(|error| Fallback::NodeNotOpened(error.to_string()))?;
        // The same test `intel-probe --run` runs, without its two break steps: if this card and
        // kernel don't do what work on the card needs, no guest finds out the hard way.
        let mut said = Vec::new();
        let summary = xe::selftest::run(
            &mut *xe,
            &self.card,
            sysfs,
            xe::selftest::Options::default(),
            &mut std::io::empty(),
            &mut said,
        );
        for line in String::from_utf8_lossy(&said).lines() {
            if line.starts_with("FAIL") {
                log::warn!("{PREFIX} self-test {line}");
            } else {
                log::info!("{PREFIX} self-test {line}");
            }
        }
        self_test_verdict(&summary)?;
        let shared = self.card.system_memory() / SHARED_MEMORY_PART;
        let cap = work::cap::video_cap(&self.card.regions, shared);
        if cap < work::cap::LEAST_FOR_A_GUEST {
            return Err(Fallback::TooLittleMemory(cap));
        }
        let keying = render_keying(&self.card, &summary);
        CardWork::new(xe, &self.card, asked, cap, keying).ok_or(Fallback::NoMemoryRegion)
    }
}

/// How render queues are to be keyed: by binding table and index when xe says it can and the
/// self-test made and ran such a queue on this card, and by index alone otherwise.
fn render_keying(card: &CardFacts, summary: &xe::selftest::Summary) -> Keying {
    if card.config.offers_cache_keying() && summary.cache_keyed {
        Keying::BindingTableAndIndex
    } else {
        Keying::BindingIndex
    }
}

/// Whether the self-test lets the guest's work on the card: every step work needs passed. A failed
/// step work does without, such as a video engine, the clock or the cleanup, is a warning only.
fn self_test_verdict(summary: &xe::selftest::Summary) -> Result<(), Fallback> {
    let needed = summary.failed_that_work_needs();
    if !needed.is_empty() {
        return Err(Fallback::SelfTestFailed(needed));
    }
    let spared = summary.failed_that_work_does_without();
    if !spared.is_empty() {
        log::warn!(
            "{PREFIX} self-test: {} failed, which the guest's work doesn't need, so it runs on the card all the same",
            spared.join(", ")
        );
    }
    Ok(())
}

/// The facts of a card recorded elsewhere, for a lab run on a PC without it. The guest must have
/// been given the recorded card's address, so a machine made for another card can't start on it
/// unnoticed. The log says loudly that nothing here is a card.
fn recorded_card(file: &std::path::Path, address: Option<&str>) -> Result<CardFacts, Refusal> {
    let refused = |why: String| Refusal(format!("The recorded Intel card in {} {why}", file.display()));
    let text =
        std::fs::read_to_string(file).map_err(|error| refused(format!("could not be read: {error}")))?;
    let card = recording::read(&text).map_err(|bad| refused(format!("is damaged: {bad}")))?;
    let recorded = card.address.as_deref().unwrap_or("-");
    if address.is_some_and(|wanted| wanted != recorded) {
        return Err(refused(format!(
            "is of the card at {recorded}, and this guest was given the one at {}.",
            address.unwrap_or_default()
        )));
    }
    log::warn!(
        "{PREFIX} RECORDED CARD: no Intel card is opened. The facts are a recording, from {} ({}), named in \
         {}. A lab run only: nothing the guest sends reaches a card.",
        file.display(),
        recorded,
        recording::RECORDED_CARD_VARIABLE
    );
    Ok(card)
}

/// What a recorded card can be asked for: listening, whatever a person asked, as there is no card
/// to run on. A person's own request to listen keeps its own reason.
fn on_a_recorded_card((asked, why): (Stage, Option<Fallback>)) -> (Stage, Option<Fallback>) {
    (Stage::Listen, if asked.is_real() { Some(Fallback::RecordedCard) } else { why })
}

/// The opener for a card that isn't there.
/// The lab card's switch, read only in a build that has the lab card.
fn lab_switch() -> Option<std::ffi::OsString> {
    #[cfg(feature = "lab-fake-card")]
    return std::env::var_os(lab::FAKE_CARD_VARIABLE);
    #[cfg(not(feature = "lab-fake-card"))]
    None
}

impl IntelBackend {
    /// A build without the lab card leaves a recorded card listening, whatever is asked.
    #[cfg(not(feature = "lab-fake-card"))]
    fn start_on_lab_card(
        &mut self,
        _asked: (Stage, Option<Fallback>),
        _switch: Option<&std::ffi::OsStr>,
    ) -> bool {
        false
    }
}

fn no_card() -> std::io::Result<Box<dyn Xe + Send>> {
    Err(std::io::Error::other("there is no card"))
}

/// Starts a copy by the copy engine wherever the engine can be given it, as the CPU reads the
/// card's memory slowly and the guest waits while it copies. A copy in rows at both ends (`rows`)
/// that the engine can't be given is done by the CPU at once instead; one in tiles fails, which the
/// log is told. An empty copy is done at once. `turn` names rows of the target whose red and blue
/// change over once the copy is done.
fn start_copy(
    work: &mut CardWork,
    host: &mut dyn HostServices,
    space: SpaceId,
    blit: Result<Blit, String>,
    rows: Option<Rows>,
    turn: Option<Area>,
) -> StagingStart {
    let started = match blit {
        Ok(blit) if blit.is_empty() => return StagingStart::Finished(Status::SUCCESS),
        Ok(blit) => work.start_blit(host, space, &blit),
        Err(why) => Err(BackendError::new(format!("the copy engine isn't given this copy: {why}"))),
    };
    let error = match started {
        Ok(()) => {
            work.turn_after_blit(space, turn);
            return StagingStart::Running;
        }
        Err(error) => error,
    };
    let Some(rows) = rows else {
        work.note_blit_failure(&error);
        return StagingStart::Finished(Status::UNSUCCESSFUL);
    };
    work.note_cpu_instead(&error);
    // A large copy goes in steps, which the copy's polls take: see `work::cpu_copy`.
    work.start_cpu_copy(space, Some(rows), turn)
        .map_or(StagingStart::Running, |ended| StagingStart::Finished(copied(ended)))
}

/// A copy's result as the guest is told it, with the reason in the log.
fn copied(result: Result<(), BackendError>) -> Status {
    match result {
        Ok(()) => Status::SUCCESS,
        Err(error) => {
            log::warn!("{PREFIX} a frame wasn't copied: {error}");
            Status::UNSUCCESSFUL
        }
    }
}

/// The adapter's name, as the guest's programs see it in the adapter's description and the record.
fn adapter_name(card: &CardFacts) -> String {
    format!("Intel GPU {:#06x}", card.pci.device)
}

/// What the record is built from: the card where it can say, and the generation where it can't.
fn record_facts(
    card: &CardFacts,
    generation: &Generation,
    shared: u64,
    render_keying: Keying,
) -> RecordFacts {
    let ip = |gt: Option<&Gt>| {
        gt.map_or(IpWord::default(), |gt| IpWord::new(gt.ip.major, gt.ip.minor, gt.ip.revision))
    };
    let config = |key| card.hwconfig.value(key).unwrap_or(0);
    let threads_per_eu =
        card.hwconfig.value(hwconfig::NUM_THREADS_PER_EU).unwrap_or(generation.threads_per_eu);
    let most_dual_subslices = card.hwconfig.most_cores().unwrap_or(0);
    // The counts are of the cores the slice arrays can hold, so the arrays and the counts agree.
    let cores = shape::representable(&card.cores(), config(hwconfig::MAX_SLICES_SUPPORTED));
    let (eus, dual_subslices, slices) = if cores.is_empty() {
        (card.eu_count(), card.dual_subslices(), card.slices().unwrap_or(0))
    } else {
        let eus = cores.iter().fold(0u32, |sum, core| sum.saturating_add(core.eu_count));
        (eus, cores.len() as u32, shape::slices_in(&cores))
    };
    RecordFacts {
        platform: Platform {
            product_family: generation.product_family,
            pch_family: generation.pch_family,
            display_core: generation.display_core,
            render_core: generation.render_core,
            platform_type: generation.platform_type,
            device_id: card.pci.device,
            revision: card.pci.revision.into(),
            gt_type: generation.gt_type,
            render_ip: ip(card.main_gt()),
        },
        gt: GtSystem {
            eu_count: eus,
            thread_count: eus.saturating_mul(threads_per_eu),
            slice_count: slices,
            subslice_count: dual_subslices,
            dual_subslice_count: dual_subslices,
            max_eu_per_subslice: card.hwconfig.most_eus_per_core().unwrap_or(0),
            max_slices: config(hwconfig::MAX_SLICES_SUPPORTED),
            max_subslices: most_dual_subslices,
            max_dual_subslices: most_dual_subslices,
            threads_per_eu,
            stage_threads: StageThreads {
                vertex: config(hwconfig::TOTAL_VS_THREADS),
                hull: config(hwconfig::TOTAL_HS_THREADS),
                domain: config(hwconfig::TOTAL_DS_THREADS),
                geometry: config(hwconfig::TOTAL_GS_THREADS),
                pixel: config(hwconfig::TOTAL_PS_THREADS),
                vertex_pocs: config(hwconfig::TOTAL_VS_THREADS_POCS),
            },
            // The hwconfig's most, or what the xe driver lists where the table doesn't say.
            engine_maxima: {
                let most = |key, class| card.hwconfig.value(key).unwrap_or_else(|| card.engines_of(class));
                EngineMaxima {
                    render: most(hwconfig::MAX_RCS, Engine::RENDER),
                    compute: most(hwconfig::MAX_CCS, Engine::COMPUTE),
                    video: most(hwconfig::MAX_VCS, Engine::VIDEO_DECODE),
                    video_enhance: most(hwconfig::MAX_VECS, Engine::VIDEO_ENHANCE),
                    copy: most(hwconfig::MAX_COPY_CS, Engine::COPY),
                }
            },
        },
        timestamp_hz: card.main_gt().map_or(0, |gt| gt.reference_clock),
        adapter_name: adapter_name(card),
        render_mhz: (card.frequencies.lowest.unwrap_or(0), card.frequencies.highest.unwrap_or(0)),
        memory: Memory { dedicated: card.card_memory(), system_shared: shared, system_video: 0 },
        local_memory: card.config.has_vram(),
        urb: urb(&card.hwconfig, generation, slices),
        cores,
        compute_engines: card.engines_of(Engine::COMPUTE),
        copy_engine: card.engines_of(Engine::COPY) > 0,
        video_engines: card.instance_mask(Engine::VIDEO_DECODE),
        video_enhance_engines: card.instance_mask(Engine::VIDEO_ENHANCE),
        address_bits: card.config.va_bits,
        xe2_tiling: generation.xe2_tiling,
        media_ip: card.media_gt().map(|gt| ip(Some(gt))),
        subsystem: (card.pci.subsystem_vendor, card.pci.subsystem_device),
        pci_class: card.pci.class,
        hwconfig: card
            .hwconfig
            .entries
            .iter()
            .filter_map(|entry| Some((entry.key, *entry.values.first()?)))
            .collect(),
        local_memory_small_pages: card.small_pages_on_card(),
        render_cache_keyed: render_keying == Keying::BindingTableAndIndex,
    }
}

/// The card's URB from its hwconfig. The size is per slice there and for the whole GPU in the
/// record; a table without it gets the generation's. A stage whose limits the table lacks is left at
/// zero, which gives that stage no entries.
fn urb(config: &Hwconfig, generation: &Generation, slices: u32) -> Urb {
    let value = |key| config.value(key).unwrap_or(0);
    let entries = |min, max| UrbEntries { min: value(min), max: value(max) };
    let per_slice = config.value(hwconfig::URB_SIZE_PER_SLICE_IN_KB).unwrap_or(generation.urb_kb_per_slice);
    Urb {
        size_kb: per_slice.saturating_mul(slices),
        vertex: entries(hwconfig::MIN_VS_URB_ENTRIES, hwconfig::MAX_VS_URB_ENTRIES),
        hull: entries(hwconfig::MIN_HS_URB_ENTRIES, hwconfig::MAX_HS_URB_ENTRIES),
        geometry: entries(hwconfig::MIN_GS_URB_ENTRIES, hwconfig::MAX_GS_URB_ENTRIES),
        domain: entries(hwconfig::MIN_DS_URB_ENTRIES, hwconfig::MAX_DS_URB_ENTRIES),
        task: entries(hwconfig::MIN_TASK_URB_ENTRIES, hwconfig::MAX_TASK_URB_ENTRIES),
        mesh: entries(hwconfig::MIN_MESH_URB_ENTRIES, hwconfig::MAX_MESH_URB_ENTRIES),
        max_starting_address: value(hwconfig::MAX_URB_STARTING_ADDRESS),
    }
}

impl Backend for IntelBackend {
    fn supports(card: &PciCard) -> bool {
        card.vendor == INTEL_VENDOR
    }

    /// The host is the Linux kernel's xe and the guest one release of Intel's driver. xe has no
    /// branches of its own: the host's side of that release's data is this backend, written for it,
    /// so the host is said to be on the guest release's branch. That is all that lets the pair
    /// through, and only for the release this backend knows. Nothing has been tested yet, so a start
    /// always warns.
    fn survey(setup: &Setup) -> Result<DriverSurvey, Refusal> {
        let version = setup.guest_driver_version.to_string();
        let known = guest_driver::GUEST_RELEASES.contains(&version.as_str());
        let kernel = sysfs::kernel_release().unwrap_or_else(|| "kernel".into());
        let host = format!("{HOST_BRANCH} {kernel}");
        let branch = if known { setup.guest_branch.clone() } else { HOST_BRANCH.into() };
        Ok(DriverSurvey {
            host: DriverFound { version: host, branch },
            guest: DriverFound { version, branch: setup.guest_branch.clone() },
            host_branch_known: true,
            guest_branch_known: known,
            host_version_tested: false,
            mixed_branches_tested: false,
            card: CardLabel::Experimental,
        })
    }

    fn open(setup: &Setup, _survey: &DriverSurvey) -> Result<Self, Refusal> {
        let address = setup.card_address.map(|address| address.to_string());
        let asked = Stage::from_environment();
        let mut backend = match recording::recorded_card_file() {
            Some(file) => {
                let mut backend = Self::from_card(recorded_card(&file, address.as_deref())?)?;
                let lab_switch = lab_switch();
                if !backend.start_on_lab_card(asked.clone(), lab_switch.as_deref()) {
                    backend.start_work_on_card(on_a_recorded_card(asked), no_card, None);
                }
                backend
            }
            None => {
                let node = intel_render_node(address.as_deref()).ok_or_else(|| {
                    Refusal(
                        "No Intel card on the xe driver was found at the address this guest was given."
                            .into(),
                    )
                })?;
                let card = CardFacts::read(&node)
                    .map_err(|error| Refusal(format!("Opening the Intel card failed: {error}")))?;
                let mut backend = Self::from_card(card)?;
                let open = || xe::device::XeCard::open(&node.device_file).map(|card| Box::new(card) as _);
                backend.start_work_on_card(asked, open, Some(&node.sysfs));
                if let Err(error) =
                    sensors::start(&node.sysfs, &node.device_file, setup.card_address.as_ref())
                {
                    log::info!("{PREFIX} no host sensors: {error}");
                }
                backend
            }
        };
        backend.facts.guest_driver_version = setup.guest_driver_version.packed();
        Ok(backend)
    }

    fn facts(&self) -> &AdapterFacts {
        &self.facts
    }

    fn wake_descriptor(&self) -> Option<std::os::fd::RawFd> {
        self.work.as_ref().and_then(CardWork::wake_descriptor)
    }

    fn allocate_video_memory(
        &mut self,
        size: u64,
        _kind: MemoryKind,
    ) -> Result<(MemoryId, u64), BackendError> {
        // Intel's Windows UMD maps GPU virtual addresses in 64 KiB pages even when xe's VRAM
        // min_page_size is 4 KiB; pad discrete-card allocations so MapGpuAddresses does not reach
        // past the end after LockForCpu fixes the allocation's guest_address.
        const VRAM_MAP_PAGE: u64 = 0x1_0000;
        let size = if self.card.config.has_vram() { size.next_multiple_of(VRAM_MAP_PAGE) } else { size };
        match &mut self.work {
            Some(work) => work.allocate(size, BufferUse::Card),
            None => Ok((self.books.new_memory(), size)),
        }
    }

    fn adopt_host_pages(&mut self, host_address: u64, size: u64) -> Result<MemoryId, BackendError> {
        match &mut self.work {
            Some(work) => work.adopt(host_address, size),
            None => Ok(self.books.new_memory()),
        }
    }

    fn release_memory(&mut self, memory: MemoryId) {
        match &mut self.work {
            Some(work) => work.release(memory),
            None => self.books.release(memory),
        }
    }

    /// By the copy engine as the kernel moves a buffer, from Alchemist on, and by the CPU before
    /// (the module `work::moves`). While listening nothing has written the video memory, so there is nothing
    /// to move.
    fn preserve_contents(
        &mut self,
        host: &mut dyn HostServices,
        own_space: SpaceId,
        from: MemoryId,
        to: MemoryId,
        size: u64,
    ) -> Result<(), BackendError> {
        match &mut self.work {
            Some(work) => work.preserve(host, own_space, from, to, size),
            None => Ok(()),
        }
    }

    /// xe clears every buffer it makes, and each allocation here gets a buffer of its own.
    fn new_video_memory_is_zero(&self) -> bool {
        true
    }

    fn read_memory_start(
        &mut self,
        _host: &mut dyn HostServices,
        _own_space: SpaceId,
        memory: MemoryId,
        out: &mut [u8],
    ) -> Result<(), BackendError> {
        match &mut self.work {
            Some(work) => work.read_start(memory, out),
            None => Err(BackendError::new("nothing has written video memory at this stage")),
        }
    }

    /// A buffer of the card's, as a file a viewer on the same card can import.
    fn export_memory(&mut self, memory: MemoryId, _size: u64) -> Result<OwnedFd, BackendError> {
        match &mut self.work {
            Some(work) => work.export(memory),
            None => Err(BackendError::new("nothing is on the card to export at this stage")),
        }
    }

    /// Intel's drivers have the CPU lock nearly every allocation, the display driver's slots among
    /// them, so once work runs the slots are looked for in the guest's pages too.
    fn slots_may_be_guest_pages(&self) -> bool {
        self.work.is_some()
    }

    /// Once work runs on the card, what it can spare beside the host's own desktop.
    fn video_memory_cap(&self) -> Option<u64> {
        self.work.as_ref().map(CardWork::cap)
    }

    fn create_address_space(&mut self, ranges: &[GpuRange]) -> Result<SpaceId, BackendError> {
        match &mut self.work {
            Some(work) => work.create_space(ranges),
            None => Ok(self.books.create_space(ranges)),
        }
    }

    fn destroy_address_space(&mut self, space: SpaceId) {
        match &mut self.work {
            Some(work) => work.destroy_space(space),
            None => self.books.destroy_space(space),
        }
    }

    fn map(&mut self, request: &MapRequest) -> Result<(), BackendError> {
        match &mut self.work {
            Some(work) => work.map(request),
            None => self.books.map(request),
        }
    }

    fn unmap(&mut self, unmap: &UnmapRequest) -> Result<(), BackendError> {
        match &mut self.work {
            Some(work) => work.unmap(unmap),
            None => self.books.unmap(unmap),
        }
    }

    /// Data this backend refuses gets a page, as the call fails anyway once
    /// [`Backend::allocation_data_valid`] says so.
    fn describe_allocation(&mut self, private: &[u8]) -> AllocationShape {
        match Resource::parse(private) {
            Ok(resource) => {
                self.traffic.allocation(private, &resource);
                if !resource.size_is_sane() {
                    self.traffic.refused(
                        "allocation-size",
                        "past every GPU address, given one page",
                        private,
                    );
                }
                resource.shape()
            }
            Err(_) => Resource::default().shape(),
        }
    }

    fn allocation_data_valid(&mut self, private: &[u8]) -> bool {
        let refused = Resource::parse(private).err();
        if let Some(refused) = refused {
            self.traffic.refused("allocation", refused.0, private);
        }
        refused.is_none()
    }

    /// Nothing is written back into an allocation's data.
    fn finish_allocation_data(&mut self, _private: &mut [u8], _size: u64) {}

    fn guest_placed_gpu_ranges(&self) -> &'static [GpuRange] {
        protocol::partition::guest_placed(self.card.config.va_bits)
    }

    fn finish_queue_data(&mut self, queue: u32, private: &mut [u8], serial: u32) -> Result<(), BackendError> {
        self.traffic.queue(queue, serial, private);
        Ok(())
    }

    fn adapter_opened(&mut self, guest_adapter_id: u64) {
        log::info!("{PREFIX} adapter-opened guest_id={guest_adapter_id:#x}");
    }

    fn guest_restarted(&mut self) {
        log::info!("{PREFIX} guest-restarted");
        self.media.guest_restarted();
    }

    /// A partition escape asking for one gets the record's, and the media driver's questions about
    /// its engines and its reset count are answered as the kernel driver answers them. Every other
    /// is answered by leaving the guest's buffer as it came.
    fn escape(&mut self, process: u32, data: &mut [u8], _books: &GuestBooks<'_>) {
        let escape = Escape::parse(data);
        let partition = protocol::escape::answer_partition(data, &protocol::partition::ours());
        let media = self.media.answer(process, data, &self.media_engines);
        self.traffic.escape(process, data, &escape, partition, media);
        if let Some(answered) = media {
            self.traffic.media(process, &answered);
        }
    }

    fn adapter_answer(&mut self, query: &VendorQuery<'_>, out: &mut Vec<u8>) -> Option<Status> {
        let outcome = match query.kind {
            record::QUERY_TYPE => {
                let (status, asked) = record::answer(&self.record, query, out);
                QueryOutcome::Record(status, asked)
            }
            guest_driver::QUERY_OPENGL => {
                let driver = guest_driver::write_opengl_info(query, &self.generation.drivers, out);
                QueryOutcome::OpenGl(Status::SUCCESS, driver)
            }
            QUERY_USER_DRIVER_NAME => {
                let (status, name) = guest_driver::write_name(query, self.desktop, out);
                if let guest_driver::NameAnswer::Withheld(why) = name {
                    self.traffic.program_withheld(query.program, why);
                }
                QueryOutcome::DriverName(status, name)
            }
            protocol::update_status::QUERY_DRIVER_UPDATE_STATUS => {
                match protocol::update_status::answer(query, self.desktop, out) {
                    Some(status) => QueryOutcome::UpdateInProgress(status),
                    None => QueryOutcome::LeftToTheCore,
                }
            }
            protocol::node_metadata::QUERY_NODE_METADATA => match protocol::node_metadata::answer(query, out)
            {
                Some(status) => QueryOutcome::Caps(status),
                None => QueryOutcome::LeftToTheCore,
            },
            _ => match protocol::wddm_caps::answer(query, out) {
                Some(status) => QueryOutcome::Caps(status),
                None => QueryOutcome::LeftToTheCore,
            },
        };
        self.traffic.query(query, outcome);
        match outcome {
            QueryOutcome::Record(status, _)
            | QueryOutcome::DriverName(status, _)
            | QueryOutcome::OpenGl(status, _)
            | QueryOutcome::Caps(status)
            | QueryOutcome::UpdateInProgress(status) => Some(status),
            QueryOutcome::LeftToTheCore => None,
        }
    }

    fn registry_value(&mut self, name: &RegistryName, _asked_type: u32) -> Option<RegistryValue> {
        let value = guest_driver::registry(name, &self.generation.drivers);
        self.traffic.registry(name, value.is_some());
        value
    }

    /// An engine's cycle counter once work runs on the card. While listening, or if xe won't say,
    /// the host's clock stands in for it, in nanoseconds since the backend started.
    fn gpu_clock(&mut self) -> Result<u64, BackendError> {
        let card = self.work.as_mut().and_then(CardWork::clock);
        Ok(card.unwrap_or_else(|| self.started.elapsed().as_nanos() as u64))
    }

    fn context_allowed(&mut self, node: u32, client_hint: u32) -> bool {
        let allowed = protocol::nodes::offered(node);
        if !allowed {
            self.traffic.context_refused(node, client_hint);
        }
        allowed
    }

    fn context_data_valid(&mut self, process: u32, node: u32, private: &[u8]) -> bool {
        match Context::parse(private) {
            Ok(context) => {
                self.traffic.context(process, node, private, &context);
                true
            }
            Err(refused) => {
                self.traffic.refused("context", refused.0, private);
                false
            }
        }
    }

    fn context_gone(&mut self, process: u32, node: u32) {
        log::debug!("{PREFIX} context-gone process={process:#x} node={node}");
    }

    fn process_gone(&mut self, process: u32) {
        log::debug!("{PREFIX} process-gone process={process:#x}");
        self.media.process_gone(process);
    }

    /// Each submission is logged with what its header says, as far as it can be read. While
    /// listening nothing runs, so none fails: each is finished unrun.
    /// A context's submission names its commands in the message, and its header after them.
    fn runs_context_commands(&self) -> bool {
        true
    }

    fn start_work(&mut self, host: &mut dyn HostServices, work: &Work<'_>) -> WorkStart {
        let header = Header::read(work.private);
        let start = match &mut self.work {
            Some(card) => card.start(host, work, &header),
            None => {
                if work.source == WorkSource::Context {
                    work::marks::write_marks_without_card(host, work, &header);
                }
                WorkStart::Finished
            }
        };
        let sends = |doorbell| self.work.as_ref().is_some_and(|work| work.sends(doorbell));
        let answer = match start {
            WorkStart::Started { doorbell, .. } if !sends(doorbell) => "unrun-after-earlier-runs",
            WorkStart::Started { .. } => "started",
            WorkStart::QueueFailed => "queue-failed",
            _ => "finished-unrun",
        };
        self.traffic.submission(work, &header, answer);
        start
    }

    fn ring_doorbell(&mut self, _queue: u32, doorbell: u64) -> Result<Doorbell, BackendError> {
        match &mut self.work {
            Some(work) => work.ring_doorbell(doorbell),
            None => Ok(Doorbell::Rung),
        }
    }

    fn poll_work(&mut self, host: &mut dyn HostServices, now: Instant) {
        if let Some(work) = &mut self.work {
            work.poll(host, now);
        }
    }

    /// Runs whose end the host finishes wake the daemon through an eventfd: see `work::wake`.
    fn gpu_woke(&mut self, host: &mut dyn HostServices, now: Instant) {
        if let Some(work) = &mut self.work {
            work.woke(host, now);
        }
    }

    fn needs_polling(&self) -> bool {
        self.work.as_ref().is_some_and(CardWork::needs_polling)
    }

    fn work_sent(&self) -> u64 {
        self.work.as_ref().map_or(0, CardWork::runs_sent)
    }

    /// From xe's timelines, which only the kernel signals. While listening nothing reaches the card.
    fn work_ended(&mut self) -> u64 {
        self.work.as_mut().map_or(0, CardWork::runs_ended)
    }

    fn close_queue(&mut self, queue: u32) {
        self.traffic.queue_closed(queue);
        if let Some(work) = &mut self.work {
            work.close_queue(queue);
        }
    }

    /// By the copy engine, which [`Backend::poll_staging_copy`] waits for, or by the CPU at once
    /// for a copy in rows the engine can't be given: see [`start_copy`]. While listening, what would
    /// be copied was never drawn, so the copy is done by leaving the target as it is.
    fn start_staging_copy(&mut self, host: &mut dyn HostServices, copy: &StagingCopy) -> StagingStart {
        self.traffic.copy(if copy.is_screen { "screen" } else { "staging" });
        let Some(work) = &mut self.work else { return StagingStart::Finished(Status::SUCCESS) };
        let turn = colours_turn(&copy.source, &copy.target)
            .then(|| Area::written_by(copy, COPY_PIXEL_BYTES))
            .flatten();
        let blit = Blit::of_staging(copy).map_err(|why| format!("{why}: {copy:?}"));
        start_copy(work, host, copy.own_space, blit, Rows::of_staging(copy, COPY_PIXEL_BYTES), turn)
    }

    /// A whole frame into a viewer's slot, by the copy engine: see [`start_copy`].
    fn start_slot_copy(&mut self, host: &mut dyn HostServices, copy: &SlotCopy) -> StagingStart {
        let Some(work) = &mut self.work else { return StagingStart::Finished(Status::UNSUCCESSFUL) };
        let blit = Blit::of_slot(copy).map_err(|why| format!("{why}: {copy:?}"));
        start_copy(work, host, copy.own_space, blit, Rows::of_slot(copy, COPY_PIXEL_BYTES), None)
    }

    /// A copy by the copy engine, or a large one by the CPU, which takes a step at each poll.
    fn poll_staging_copy(&mut self, _host: &mut dyn HostServices, now: Instant) -> Option<Status> {
        let Some(work) = &mut self.work else { return Some(Status::SUCCESS) };
        match work.poll_blit(now)? {
            Ok(()) => Some(Status::SUCCESS),
            Err(error) => {
                work.note_blit_failure(&error);
                Some(Status::UNSUCCESSFUL)
            }
        }
    }

    /// As the copy engine lays the surface out, for the tile modes it is given here: see
    /// [`work::blits::tiled_bytes`]. Any other tile mode isn't measured, so the core refuses the copy.
    fn tiled_bytes(&self, surface: &SurfaceShape, x: u64, y: u64) -> Option<u64> {
        work::blits::tiled_bytes(surface, x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend::{DriverVersion, StartDecision, decide_start};

    fn summary_with(failed: &[&str]) -> xe::selftest::Summary {
        xe::selftest::Summary {
            passed: 10,
            failed: failed.len() as u32,
            skipped: 0,
            failed_steps: failed.iter().map(|step| (*step).to_owned()).collect(),
            cache_keyed: false,
        }
    }

    #[test]
    fn a_failed_step_work_needs_keeps_work_off_the_card_and_is_named() {
        for step in ["vm", "buffers", "userptr", "binds", "engine rcs0", "engine ccs0", "engine bcs0"] {
            let verdict = self_test_verdict(&summary_with(&["clock", step]));
            assert_eq!(verdict, Err(Fallback::SelfTestFailed(vec![step.to_owned()])), "{step}");
        }
        let both = self_test_verdict(&summary_with(&["binds", "engine rcs0"])).unwrap_err();
        assert!(both.to_string().contains("its steps binds, engine rcs0 failed"), "{both}");
    }

    /// Starts work on a stand-in for xe at the stage asked, and says how render queues are keyed
    /// and what the record's render cache entry is.
    fn started_keyed(
        card: CardFacts,
        xe: &xe::fake::FakeXe,
        stage: Stage,
    ) -> (IntelBackend, Keying, Option<u8>) {
        let mut backend = IntelBackend::from_card(card).unwrap();
        assert_eq!(
            protocol::features::workaround(backend.record(), record::RENDER_CACHE_KEYING_WORKAROUND),
            Some(0)
        );
        let given = xe.clone();
        backend.start_work_on_card((stage, None), move || Ok(Box::new(given) as _), None);
        let entry = protocol::features::workaround(backend.record(), record::RENDER_CACHE_KEYING_WORKAROUND);
        let keying = backend.render_keying();
        (backend, keying, entry)
    }

    #[test]
    fn the_record_and_the_render_queues_follow_one_keying_decided_at_the_start() {
        // xe offers it and the self-test made and ran such a queue: the B580's table entry stands.
        let xe = xe::fake::FakeXe::default();
        let (mut backend, keying, entry) = started_keyed(card::tests::battlemage(), &xe, Stage::Everything);
        assert_eq!((keying, entry), (Keying::BindingTableAndIndex, Some(1)));
        // Staying to listen afterwards makes no queue, and the entry is cleared again.
        backend.start_work_on_card((Stage::Listen, None), no_card, None);
        assert_eq!(backend.render_keying(), Keying::BindingIndex);
        assert_eq!(
            protocol::features::workaround(backend.record(), record::RENDER_CACHE_KEYING_WORKAROUND),
            Some(0)
        );

        // A kernel that doesn't offer it: Lunar Lake's table sets the entry too, and it is cleared.
        let card = card::tests::lunar_lake();
        assert!(!card.config.offers_cache_keying());
        let (_, keying, entry) = started_keyed(card, &xe::fake::FakeXe::default(), Stage::Everything);
        assert_eq!((keying, entry), (Keying::BindingIndex, Some(0)));

        // xe says it can, and then turns the property down in the self-test.
        let xe = xe::fake::FakeXe::default();
        xe.without_keying();
        let (_, keying, entry) = started_keyed(card::tests::battlemage(), &xe, Stage::Everything);
        assert_eq!((keying, entry), (Keying::BindingIndex, Some(0)));

        // A stage that runs nothing on the card.
        let (_, keying, entry) =
            started_keyed(card::tests::battlemage(), &xe::fake::FakeXe::default(), Stage::Listen);
        assert_eq!((keying, entry), (Keying::BindingIndex, Some(0)));
    }

    #[test]
    fn the_lunar_lake_table_sets_the_render_cache_entry_for_a_keyed_start() {
        let card = card::tests::lunar_lake();
        let card = CardFacts {
            config: xe_query::Config { flags: xe_query::Config::HAS_CACHE_KEYING, ..card.config },
            ..card
        };
        let (_, keying, entry) = started_keyed(card, &xe::fake::FakeXe::default(), Stage::Everything);
        assert_eq!((keying, entry), (Keying::BindingTableAndIndex, Some(1)), "the table's value");
    }

    #[test]
    fn a_failed_video_engine_copy_clock_or_cleanup_lets_work_run() {
        for step in ["engine vcs0", "engine vecs0", "copy", "clock", "cleanup", "render cache keying"] {
            assert_eq!(self_test_verdict(&summary_with(&[step])), Ok(()), "{step}");
        }
        assert_eq!(
            self_test_verdict(&summary_with(&["engine vcs0", "engine vecs0", "copy", "clock", "cleanup"])),
            Ok(())
        );
        assert_eq!(self_test_verdict(&summary_with(&[])), Ok(()));
    }

    /// A recording of the test card in a file of its own.
    fn recorded(name: &str, text: &str) -> std::path::PathBuf {
        let file =
            std::env::temp_dir().join(format!("guesthouse-recorded-{}-{name}.card", std::process::id()));
        std::fs::write(&file, text).unwrap();
        file
    }

    #[test]
    #[cfg(not(feature = "lab-fake-card"))]
    fn a_recorded_card_listens_whatever_the_lab_switch_says_in_a_build_without_the_lab_card() {
        let mut backend = IntelBackend::from_card(card::tests::lunar_lake()).unwrap();
        let asked = Stage::asked(None);
        assert!(!backend.start_on_lab_card(asked.clone(), Some(std::ffi::OsStr::new("1"))));
        backend.start_work_on_card(on_a_recorded_card(asked), no_card, None);
        assert_eq!(backend.stage(), Stage::Listen);
        assert!(matches!(backend.listening, Some(Fallback::RecordedCard)));
        let (given, why) = Desktop::Hardware.at_stage(backend.stage(), backend.listening.as_ref(), false);
        assert_eq!(given, Desktop::Software);
        assert!(why.is_some());
    }

    #[test]
    fn a_recorded_card_stands_in_only_for_the_card_at_its_address() {
        let file = recorded("address", &recording::write(&card::tests::lunar_lake()));
        let card = recorded_card(&file, Some("0000:00:02.0")).unwrap();
        assert_eq!(card.pci.device, 0x64A0);
        assert!(recorded_card(&file, None).is_ok(), "a guest given no address takes it too");
        let other = recorded_card(&file, Some("0000:03:00.0")).unwrap_err();
        assert!(other.0.contains("is of the card at 0000:00:02.0"), "{}", other.0);
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn a_damaged_or_missing_recording_is_refused_and_says_where_it_is() {
        let file = recorded("damaged", "guesthouse-intel-card 1\nconfg 1\n");
        assert!(recorded_card(&file, None).unwrap_err().0.contains("is damaged: line 2"));
        std::fs::remove_file(&file).unwrap();
        assert!(recorded_card(&file, None).unwrap_err().0.contains("could not be read"));
    }

    fn setup_for(version: &str) -> Setup {
        Setup {
            guest_driver_version: DriverVersion::parse(version).unwrap(),
            guest_branch: version.into(),
            driver_folder: Default::default(),
            card: None,
            card_address: None,
        }
    }

    #[test]
    fn the_known_guest_release_starts_with_a_warning_and_another_is_refused() {
        let survey = IntelBackend::survey(&setup_for(guest_driver::GUEST_RELEASES[0])).unwrap();
        assert!(matches!(decide_start(&survey), StartDecision::RunWithWarning(_)));
        assert_eq!(survey.card, CardLabel::Experimental);
        assert!(!survey.mixed_branches_tested && !survey.host_version_tested, "nothing is claimed tested");
        let survey = IntelBackend::survey(&setup_for("31.0.101.5000")).unwrap();
        assert!(matches!(decide_start(&survey), StartDecision::Refuse(_)));
    }

    #[test]
    fn the_facts_come_from_the_card() {
        let backend = IntelBackend::from_card(card::tests::lunar_lake()).unwrap();
        let facts = backend.facts();
        assert_eq!((facts.card.vendor, facts.card.device, facts.card.revision), (INTEL_VENDOR, 0x64A0, 4));
        assert_eq!(facts.video_memory, 16 << 30, "half the system memory");
        assert!(facts.engines.graphics && facts.engines.compute);
        assert_eq!(facts.node_count, 15);
    }

    #[test]
    fn the_record_is_built_from_the_card_and_its_generation() {
        let backend = IntelBackend::from_card(card::tests::lunar_lake()).unwrap();
        let summary = record::summary(backend.record());
        assert_eq!((summary.product_family, summary.render_core, summary.device_id), (1275, 0xC09, 0x64A0));
        assert_eq!(summary.render_ip, IpWord::new(20, 4, 4).0);
        assert_eq!((summary.eu_count, summary.virtual_machine, summary.compression), (64, 1, false));
    }

    /// A test card given another generation's device id and render IP.
    fn card_as(mut card: CardFacts, device: u16, ip: xe_query::IpVersion) -> CardFacts {
        card.pci.device = device;
        card.config.device_id = device;
        card.gts.iter_mut().filter(|gt| gt.kind == Gt::MAIN).for_each(|gt| gt.ip = ip);
        card
    }

    fn ip(major: u16, minor: u16) -> xe_query::IpVersion {
        xe_query::IpVersion { major, minor, revision: 0 }
    }

    #[test]
    fn a_panther_lake_card_gets_xe3s_family_core_and_threads() {
        let card = card_as(card::tests::lunar_lake(), 0xB080, ip(30, 0));
        let backend = IntelBackend::from_card(card.clone()).unwrap();
        let summary = record::summary(backend.record());
        assert_eq!((summary.product_family, summary.render_core, summary.device_id), (1300, 0x1E00, 0xB080));
        assert!(protocol::features::is_set(backend.record(), protocol::features::XE2_TILING));
        let mut bare = card;
        bare.hwconfig = hwconfig::Hwconfig::default();
        let facts = record_facts(&bare, &generation::XE3_LPG, 0, Keying::BindingIndex);
        assert_eq!(facts.gt.thread_count, 64 * 10, "ten threads to an EU where hwconfig doesn't say");
    }

    #[test]
    fn an_alchemist_card_with_no_ip_gets_dg2s_family_and_no_xe2_tiling() {
        let backend = IntelBackend::from_card(card_as(card::tests::battlemage(), 0x56A0, ip(0, 0))).unwrap();
        let summary = record::summary(backend.record());
        assert_eq!((summary.product_family, summary.render_core, summary.device_id), (1270, 0xC07, 0x56A0));
        assert_eq!(summary.render_ip, 0, "xe gives no IP for it, so the record says none");
        assert!(!protocol::features::is_set(backend.record(), protocol::features::XE2_TILING));
        assert!(protocol::features::is_set(backend.record(), protocol::features::LOCAL_MEMORY));
    }

    #[test]
    fn meteor_lake_and_arrow_lake_cards_get_their_own_family_by_id() {
        let meteor = IntelBackend::from_card(card_as(card::tests::lunar_lake(), 0x7D55, ip(12, 71))).unwrap();
        let arrow = IntelBackend::from_card(card_as(card::tests::lunar_lake(), 0x7D67, ip(12, 70))).unwrap();
        assert_eq!(record::summary(meteor.record()).product_family, 1272);
        assert_eq!(record::summary(arrow.record()).product_family, 1273);
        assert!(!protocol::features::is_set(arrow.record(), protocol::features::XE2_TILING));
        let unplaced = IntelBackend::from_card(card_as(card::tests::lunar_lake(), 0x7DFF, ip(12, 70)));
        assert!(unplaced.unwrap_err().0.contains("intel-probe --record"));
    }

    #[test]
    fn a_card_with_memory_of_its_own_offers_that() {
        let backend = IntelBackend::from_card(card::tests::battlemage()).unwrap();
        let facts = backend.facts();
        assert_eq!((facts.video_memory, facts.bar_size), (12216 << 20, 12216 << 20));
        let summary = record::summary(backend.record());
        assert_eq!((summary.product_family, summary.render_core, summary.device_id), (1274, 0xC09, 0xE20B));
        assert_eq!(summary.eu_count, 160);
        let built = record_facts(backend.card(), &generation::XE2_HPG, 7, Keying::BindingIndex);
        assert_eq!(built.memory, Memory { dedicated: 12216 << 20, system_shared: 7, system_video: 0 });
        assert!(built.local_memory);
        assert_eq!(
            (built.gt.max_slices, built.gt.max_dual_subslices, built.gt.max_eu_per_subslice),
            (5, 20, 8)
        );
        assert_eq!(
            (built.gt.slice_count, built.gt.dual_subslice_count, built.gt.thread_count),
            (5, 20, 1280)
        );
    }

    #[test]
    fn a_card_whose_hwconfig_lacks_the_urb_gets_its_generations_size_and_no_limits() {
        let backend = IntelBackend::from_card(card::tests::lunar_lake()).unwrap();
        let urb = record::urb(backend.record());
        assert_eq!(urb.size_kb, 600, "one slice of Lunar Lake's");
        assert_eq!(urb.vertex, UrbEntries::default());
        assert_eq!(urb.max_starting_address, 0);
    }

    #[test]
    fn the_urb_size_is_the_hwconfigs_per_slice_figure_times_the_slices() {
        let config = Hwconfig::parse(&hwconfig::tests::table(&[(hwconfig::URB_SIZE_PER_SLICE_IN_KB, 768)]));
        assert_eq!(urb(&config, &generation::XE2_HPG, 5).size_kb, 3840);
        assert_eq!(urb(&config, &generation::XE2_HPG, 0).size_kb, 0, "nothing to divide by");
        assert_eq!(urb(&Hwconfig::default(), &generation::XE_HPG, 2).size_kb, 960);
    }

    #[test]
    fn a_card_with_no_eus_is_refused_in_plain_words() {
        let mut card = card::tests::lunar_lake();
        card.topology.clear();
        let refusal = IntelBackend::from_card(card).unwrap_err();
        assert!(refusal.0.contains("no EUs"), "{}", refusal.0);
    }

    #[test]
    fn a_card_with_no_profile_is_refused_in_plain_words() {
        let mut card = card::tests::lunar_lake();
        card.gts[0].ip.minor = 3;
        let refusal = IntelBackend::from_card(card).unwrap_err();
        assert!(refusal.0.contains("no profile"), "{}", refusal.0);
        assert!(refusal.0.contains("intel-probe --record"), "it says what to send: {}", refusal.0);
    }

    #[test]
    fn the_thread_count_falls_back_to_the_generations_when_hwconfig_has_none() {
        let mut card = card::tests::lunar_lake();
        card.hwconfig = hwconfig::Hwconfig::default();
        let facts = record_facts(&card, &generation::XE2_LPG, 0, Keying::BindingIndex);
        assert_eq!(facts.gt.thread_count, 64 * generation::XE2_LPG.threads_per_eu);
        assert_eq!(facts.gt.max_slices, 0, "not guessed");
    }
    /// A backend on the test card, with a stand-in for xe that `open` hands out when asked to.
    fn started(asked: (Stage, Option<Fallback>), xe: &xe::fake::FakeXe) -> (IntelBackend, bool) {
        let mut backend = IntelBackend::from_card(card::tests::battlemage()).unwrap();
        let mut opened = false;
        let open = || {
            opened = true;
            Ok(Box::new(xe.clone()) as Box<dyn Xe + Send>)
        };
        backend.start_work_on_card(asked, open, None);
        (backend, opened)
    }

    #[test]
    fn with_nothing_asked_a_card_that_passes_its_test_runs_everything() {
        let xe = xe::fake::FakeXe::default();
        let (backend, opened) = started(Stage::asked(None), &xe);
        assert!(opened);
        assert_eq!(backend.stage(), stage::FURTHEST_STAGE);
        assert_eq!(xe.live(), 0, "the test at the start left nothing on the card");
        assert!(!xe.calls_named("exec").is_empty(), "and it did run");
    }

    #[test]
    fn a_recorded_card_listens_whatever_is_asked_and_nothing_is_opened() {
        for asked in [Stage::asked(None), (Stage::Compute, None), (Stage::Everything, None)] {
            let on_recording = on_a_recorded_card(asked);
            assert_eq!(on_recording, (Stage::Listen, Some(Fallback::RecordedCard)));
            let xe = xe::fake::FakeXe::default();
            let (backend, opened) = started(on_recording, &xe);
            assert_eq!(backend.stage(), Stage::Listen);
            assert!(!opened && xe.calls().is_empty());
        }
        let asked_to_listen = (Stage::Listen, Some(Fallback::AskedFor));
        assert_eq!(on_a_recorded_card(asked_to_listen.clone()), asked_to_listen, "its own reason is kept");
        assert!(no_card().is_err());
    }

    #[test]
    fn a_hardware_desktop_asked_of_a_listening_guest_is_given_as_the_software_desktop() {
        let xe = xe::fake::FakeXe::default();
        let (mut backend, _) = started(on_a_recorded_card(Stage::asked(None)), &xe);
        let why = backend.set_desktop(Desktop::Hardware).expect("a reason");
        assert!(why.contains("layer=card"), "{why}");
        assert_eq!(backend.desktop, Desktop::Software);
        let (mut running, _) = started(Stage::asked(None), &xe::fake::FakeXe::default());
        assert_eq!(running.set_desktop(Desktop::Hardware), None);
        assert_eq!(running.desktop, Desktop::Hardware);
    }

    #[test]
    fn the_off_switch_and_an_unknown_word_keep_everything_off_the_card() {
        for value in ["0", "listen", "maybe"] {
            let xe = xe::fake::FakeXe::default();
            let asked = Stage::asked(Some(std::ffi::OsStr::new(value)));
            let (mut backend, opened) = started(asked, &xe);
            assert_eq!(backend.stage(), Stage::Listen, "{value}");
            assert!(!opened, "{value}: the card isn't even opened for work");
            // What a guest does at this stage is names in books, as before.
            let (memory, _) = backend.allocate_video_memory(0x1000, MemoryKind::PLAIN).unwrap();
            let space = backend.create_address_space(&guesthouse_backend::GPU_RANGES).unwrap();
            backend.map(&work::testing::map_request(space, memory, 0x400_0000, 0, 0x1000)).unwrap();
            let mut host = work::testing::FakeHost::default();
            let submission = work::testing::Rig::new(Stage::Everything).submission(1, 1);
            assert_eq!(backend.start_work(&mut host, &submission.work()), WorkStart::Finished);
            assert_eq!(backend.ring_doorbell(1, 1).unwrap(), Doorbell::Rung);
            backend.poll_work(&mut host, Instant::now());
            backend.release_memory(memory);
            assert!(xe.calls().is_empty(), "{value}: nothing reached xe");
        }
    }

    #[test]
    fn a_card_that_fails_its_test_at_the_start_listens_and_the_guest_still_starts() {
        let xe = xe::fake::FakeXe::default();
        xe.fail("exec", libc::EOPNOTSUPP);
        let (mut backend, opened) = started(Stage::asked(None), &xe);
        assert!(opened);
        assert_eq!(backend.stage(), Stage::Listen);
        assert_eq!(xe.live(), 0, "what the test made is gone");
        xe.heal();
        let calls = xe.calls().len();
        backend.allocate_video_memory(0x1000, MemoryKind::PLAIN).unwrap();
        assert_eq!(xe.calls().len(), calls, "and the guest's memory is names again");
    }

    #[test]
    fn a_card_that_cannot_be_opened_for_work_or_is_not_on_xe_listens() {
        let mut backend = IntelBackend::from_card(card::tests::battlemage()).unwrap();
        let denied = || Err(std::io::Error::from_raw_os_error(libc::EACCES));
        backend.start_work_on_card(Stage::asked(None), denied, None);
        assert_eq!(backend.stage(), Stage::Listen);
        let mut on_i915 = card::tests::battlemage();
        on_i915.driver = "i915".into();
        let mut backend = IntelBackend::from_card(on_i915).unwrap();
        let never =
            || -> std::io::Result<Box<dyn Xe + Send>> { panic!("a card not on xe is never opened for work") };
        backend.start_work_on_card(Stage::asked(None), never, None);
        assert_eq!(backend.stage(), Stage::Listen);
    }

    #[test]
    fn past_listening_the_trait_makes_real_things_and_runs_a_batch() {
        let xe = xe::fake::FakeXe::default();
        let (mut backend, _) = started((Stage::Everything, None), &xe);
        let before = xe.calls().len();
        let (batch, size) = backend.allocate_video_memory(0x1000, MemoryKind::PLAIN).unwrap();
        assert_eq!(size, 0x1_0000);
        let pages = work::testing::Pages::new(0x1000);
        let fence = backend.adopt_host_pages(pages.address(), 0x1000).unwrap();
        let space = backend.create_address_space(&guesthouse_backend::GPU_RANGES).unwrap();
        backend.map(&work::testing::map_request(space, batch, 0x1_4000_0000, 0, 0x1000)).unwrap();
        backend.map(&work::testing::map_request(space, fence, 0x500_0000, 0, 0x1000)).unwrap();
        assert_eq!(backend.live_memory_blocks(), 2);
        let view = backend.work.as_mut().unwrap();
        view.write_for_test(batch, &xe::batch::bytes(&[xe::batch::BATCH_END]));
        let mut host = work::testing::FakeHost::default();
        host.fence_at(0x7000_0000, pages.address());
        let mut submission = work::testing::Rig::new(Stage::Everything).submission(7, 3);
        submission.space = space;
        submission.commands = Some(guesthouse_backend::CommandBuffer { address: 0x1_4000_0000, length: 4 });
        let WorkStart::Started { doorbell, .. } = backend.start_work(&mut host, &submission.work()) else {
            panic!("the batch should start")
        };
        assert_eq!(backend.ring_doorbell(7, doorbell).unwrap(), Doorbell::Rung);
        assert_eq!(pages.qword(0), 3, "the card wrote the guest's fence");
        backend.poll_work(&mut host, Instant::now());
        let mut start = [0u8; 4];
        backend.read_memory_start(&mut host, space, batch, &mut start).unwrap();
        assert_eq!(start, xe::batch::BATCH_END.to_le_bytes());
        backend.close_queue(7);
        backend.destroy_address_space(space);
        backend.release_memory(batch);
        backend.release_memory(fence);
        assert_eq!(xe.live(), 0);
        assert!(xe.calls().len() > before);
        assert!(backend.gpu_clock().unwrap() > 0);
    }

    #[test]
    fn past_listening_the_card_sets_the_cap_and_slots_may_be_guest_pages() {
        let xe = xe::fake::FakeXe::default();
        let (backend, _) = started((Stage::Everything, None), &xe);
        // The test card: 12216 MiB, of which a quarter is kept for the host's desktop.
        assert_eq!(backend.video_memory_cap(), Some((12216 - 3054) << 20));
        assert!(backend.slots_may_be_guest_pages());
        let (listening, _) = started((Stage::Listen, None), &xe);
        assert_eq!(listening.video_memory_cap(), None);
        assert!(!listening.slots_may_be_guest_pages());
    }

    #[test]
    /// A frame in rows goes into a viewer's slot by the copy engine, and by the CPU at once when the
    /// engine can't be given it, here a slot whose rows aren't a whole number of OWords apart.
    fn a_frame_goes_into_a_viewers_slot_by_the_copy_engine_or_by_the_cpu_if_it_cant_be() {
        let xe = xe::fake::FakeXe::default();
        let (mut backend, _) = started((Stage::Everything, None), &xe);
        let (frame, slot) = (work::testing::Pages::new(0x1000), work::testing::Pages::new(0x1000));
        frame.write(0, &[9u8; 0x1000]);
        let from = backend.adopt_host_pages(frame.address(), 0x1000).unwrap();
        let to = backend.adopt_host_pages(slot.address(), 0x1000).unwrap();
        let space = backend.create_address_space(&guesthouse_backend::GPU_RANGES).unwrap();
        backend.map(&work::testing::map_request(space, from, 0x400_0000, 0, 0x1000)).unwrap();
        backend.map(&work::testing::map_request(space, to, 0x500_0000, 0, 0x1000)).unwrap();
        let source = guesthouse_backend::SurfaceShape {
            width: 16,
            height: 8,
            format: 0,
            pitch: 64,
            tiling: None,
            refresh: (0, 1),
            samples: (1, 0),
            colour: 0,
        };
        let copy = SlotCopy {
            source,
            source_address: 0x400_0000,
            slot_address: 0x500_0000,
            slot_pitch: 64,
            width: 16,
            height: 8,
            own_space: space,
            slot_tiling: None,
        };
        let mut host = work::testing::FakeHost::default();
        let sent = xe.calls_named("exec").len();
        assert_eq!(backend.start_slot_copy(&mut host, &copy), StagingStart::Running);
        assert_eq!(xe.calls_named("exec").len(), sent + 1, "the copy engine was given it");
        assert_eq!(backend.poll_staging_copy(&mut host, Instant::now()), Some(Status::SUCCESS));
        assert_eq!(slot.bytes()[..512], [9u8; 512]);
        assert_eq!(slot.bytes()[512], 0);
        slot.write(0, &[0; 0x1000]);
        let uneven = SlotCopy { slot_pitch: 72, ..copy };
        assert_eq!(backend.start_slot_copy(&mut host, &uneven), StagingStart::Finished(Status::SUCCESS));
        assert_eq!(xe.calls_named("exec").len(), sent + 1, "the CPU did this one");
        for row in 0..8 {
            assert_eq!(slot.bytes()[row * 72..row * 72 + 64], [9u8; 64], "row {row}");
            assert_eq!(slot.bytes()[row * 72 + 64..row * 72 + 72], [0u8; 8], "past row {row}");
        }
        let tiled = SlotCopy { slot_tiling: Some(4), ..copy };
        assert_eq!(backend.start_slot_copy(&mut host, &tiled), StagingStart::Finished(Status::UNSUCCESSFUL));
        let outside = SlotCopy { height: 4096, ..copy };
        assert_eq!(
            backend.start_slot_copy(&mut host, &outside),
            StagingStart::Finished(Status::UNSUCCESSFUL)
        );
        assert!(backend.export_memory(from, 0x1000).is_err(), "guest pages are not the card's to export");
    }

    /// A picture laid out as the stand-in card lays out `tiling`, with every byte telling where it
    /// is, and the same picture in rows.
    fn tiled_picture(tiling: xe::batch::Tiling, pitch: u32, rows: usize, bytes: usize) -> (Vec<u8>, Vec<u8>) {
        let row_bytes = pitch as usize;
        let in_rows: Vec<u8> =
            (0..row_bytes * rows).map(|at| (at / row_bytes * 3 + at % row_bytes) as u8).collect();
        let mut tiled = vec![0u8; bytes];
        for (at, &byte) in in_rows.iter().enumerate() {
            let (x, y) = ((at % row_bytes) as u64, (at / row_bytes) as u64);
            tiled[xe::fake::surface_offset(tiling, pitch.into(), x, y) as usize] = byte;
        }
        (in_rows, tiled)
    }

    fn shape(width: u32, height: u32, pitch: u32, tiling: Option<u32>) -> SurfaceShape {
        SurfaceShape { width, height, format: 0, pitch, tiling, refresh: (0, 0), samples: (1, 0), colour: 0 }
    }

    /// The tester's windowed present: a back buffer in Tile64 (tile mode 81) of 1920 by 1071 at a
    /// pitch of 7680, in 0x880000 bytes, into a staging surface in rows. The trait's default
    /// measures no tiled surface, so the core refused it; now the core's three rules for a tiled
    /// side (a row inside the pitch, the rows inside the height, the tiles inside the memory) hold.
    /// Then the same kind of copy, small, runs through the stand-in card's copy engine, and a
    /// rectangle outside the source or a tile mode the engine isn't given is still refused.
    #[test]
    fn a_windowed_present_from_a_tiled_back_buffer_goes_by_the_copy_engine() {
        let xe = xe::fake::FakeXe::default();
        let (mut backend, _) = started((Stage::Everything, None), &xe);
        let back_buffer = SurfaceShape { colour: 0x7E, ..shape(1920, 1071, 7680, Some(81)) };
        let filled = backend.tiled_bytes(&back_buffer, 0, 0).unwrap();
        assert!(filled == 0x87_0000 && filled <= 0x88_0000 && 1920 * 4 <= back_buffer.pitch);
        let (in_rows, tiled) = tiled_picture(xe::batch::Tiling::Tile64, 1024, 130, 0x4_0000);
        let (frame, staging) = (work::testing::Pages::new(0x4_0000), work::testing::Pages::new(0x2_1000));
        frame.write(0, &tiled);
        let from = backend.adopt_host_pages(frame.address(), 0x4_0000).unwrap();
        let to = backend.adopt_host_pages(staging.address(), 0x2_1000).unwrap();
        let space = backend.create_address_space(&guesthouse_backend::GPU_RANGES).unwrap();
        backend.map(&work::testing::map_request(space, from, 0x400_0000, 0, 0x4_0000)).unwrap();
        backend.map(&work::testing::map_request(space, to, 0x800_0000, 0, 0x2_1000)).unwrap();
        let copy = StagingCopy {
            source: shape(256, 130, 1024, Some(81)),
            target: shape(256, 130, 1024, None),
            source_address: 0x400_0000,
            target_address: 0x800_0000,
            left: 0,
            top: 0,
            right: 256,
            bottom: 130,
            target_left: 0,
            target_top: 0,
            target_guest_address: None,
            is_screen: false,
            own_space: space,
        };
        let mut host = work::testing::FakeHost::default();
        assert_eq!(backend.start_staging_copy(&mut host, &copy), StagingStart::Running);
        assert_eq!(backend.poll_staging_copy(&mut host, Instant::now()), Some(Status::SUCCESS));
        assert_eq!(staging.bytes()[..in_rows.len()], in_rows[..]);
        let sent = xe.calls_named("exec").len();
        let outside = StagingCopy { bottom: 131, ..copy };
        assert_eq!(
            backend.start_staging_copy(&mut host, &outside),
            StagingStart::Finished(Status::UNSUCCESSFUL)
        );
        let other = StagingCopy { source: shape(256, 130, 1024, Some(82)), ..copy };
        assert_eq!(backend.tiled_bytes(&other.source, 0, 0), None, "Tile64 of 64-bit pixels");
        assert_eq!(
            backend.start_staging_copy(&mut host, &other),
            StagingStart::Finished(Status::UNSUCCESSFUL)
        );
        assert_eq!(xe.calls_named("exec").len(), sent, "neither reached the card");
        let empty = StagingCopy { right: 0, ..copy };
        assert_eq!(backend.start_staging_copy(&mut host, &empty), StagingStart::Finished(Status::SUCCESS));
    }

    /// The same present from a Direct3D 11 `R8G8B8A8` back buffer, red first, into the kernel's
    /// `A8R8G8B8` staging surface, blue first: red and blue change over on the way, by the copy
    /// engine's path from tiles and by the CPU's from rows, and the rest of each pixel stays. A back
    /// buffer that names no red-first format goes as it is.
    #[test]
    fn a_red_first_back_buffer_reaches_the_blue_first_staging_surface_with_red_and_blue_changed_over() {
        let xe = xe::fake::FakeXe::default();
        let (mut backend, _) = started((Stage::Everything, None), &xe);
        let (in_rows, tiled) = tiled_picture(xe::batch::Tiling::Tile64, 1024, 130, 0x4_0000);
        let turned: Vec<u8> = in_rows
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[red, green, blue, alpha]| [blue, green, red, alpha])
            .collect();
        let tiled_frame = work::testing::Pages::new(0x4_0000);
        let row_frame = work::testing::Pages::new(0x4_0000);
        let staging = work::testing::Pages::new(0x2_1000);
        tiled_frame.write(0, &tiled);
        row_frame.write(0, &in_rows);
        let space = backend.create_address_space(&guesthouse_backend::GPU_RANGES).unwrap();
        for (pages, address) in [(&tiled_frame, 0x400_0000), (&row_frame, 0x600_0000)] {
            let memory = backend.adopt_host_pages(pages.address(), 0x4_0000).unwrap();
            backend.map(&work::testing::map_request(space, memory, address, 0, 0x4_0000)).unwrap();
        }
        let to = backend.adopt_host_pages(staging.address(), 0x2_1000).unwrap();
        backend.map(&work::testing::map_request(space, to, 0x800_0000, 0, 0x2_1000)).unwrap();
        let red_first = |tiling| SurfaceShape { colour: 0x7E, ..shape(256, 130, 1024, tiling) };
        let copy = StagingCopy {
            source: red_first(Some(81)),
            target: SurfaceShape { format: 21, ..shape(256, 130, 1024, None) },
            source_address: 0x400_0000,
            target_address: 0x800_0000,
            left: 0,
            top: 0,
            right: 256,
            bottom: 130,
            target_left: 0,
            target_top: 0,
            target_guest_address: None,
            is_screen: false,
            own_space: space,
        };
        let mut host = work::testing::FakeHost::default();
        assert_eq!(backend.start_staging_copy(&mut host, &copy), StagingStart::Running);
        assert_eq!(backend.poll_staging_copy(&mut host, Instant::now()), Some(Status::SUCCESS));
        assert_eq!(staging.bytes()[..turned.len()], turned[..], "from tiles, by the copy engine");
        let from_rows = StagingCopy { source: red_first(None), source_address: 0x600_0000, ..copy };
        staging.write(0, &[0; 0x2_1000]);
        assert_eq!(backend.start_staging_copy(&mut host, &from_rows), StagingStart::Running);
        assert_eq!(backend.poll_staging_copy(&mut host, Instant::now()), Some(Status::SUCCESS));
        assert_eq!(staging.bytes()[..turned.len()], turned[..], "from rows, by the copy engine too");
        let as_it_is = StagingCopy { source: shape(256, 130, 1024, None), ..from_rows };
        assert_eq!(backend.start_staging_copy(&mut host, &as_it_is), StagingStart::Running);
        assert_eq!(backend.poll_staging_copy(&mut host, Instant::now()), Some(Status::SUCCESS));
        assert_eq!(staging.bytes()[..in_rows.len()], in_rows[..]);
        // A source whose rows the engine can't be given goes by the CPU, turned all the same.
        staging.write(0, &[0; 0x2_1000]);
        let uneven = StagingCopy { source: red_first(None), ..from_rows };
        let uneven = StagingCopy {
            source: SurfaceShape { pitch: 1032, ..uneven.source },
            right: 128,
            bottom: 64,
            ..uneven
        };
        assert_eq!(backend.start_staging_copy(&mut host, &uneven), StagingStart::Finished(Status::SUCCESS));
        let source = row_frame.bytes();
        let mut expected = vec![0u8; 1024 * 64];
        for row in 0..64 {
            for x in 0..128 {
                let at = row * 1032 + x * 4;
                let [red, green, blue, alpha]: [u8; 4] = source[at..at + 4].try_into().unwrap();
                expected[row * 1024 + x * 4..row * 1024 + x * 4 + 4]
                    .copy_from_slice(&[blue, green, red, alpha]);
            }
        }
        for row in 0..64 {
            assert_eq!(
                staging.bytes()[row * 1024..row * 1024 + 512],
                expected[row * 1024..row * 1024 + 512],
                "row {row}"
            );
        }
    }

    #[test]
    fn a_tiled_frame_goes_into_a_viewers_slot_by_the_copy_engine() {
        let xe = xe::fake::FakeXe::default();
        let (mut backend, _) = started((Stage::Everything, None), &xe);
        let (in_rows, tiled) = tiled_picture(xe::batch::Tiling::Tile4, 512, 64, 0x8000);
        let (frame, slot) = (work::testing::Pages::new(0x8000), work::testing::Pages::new(0x8000));
        frame.write(0, &tiled);
        let from = backend.adopt_host_pages(frame.address(), 0x8000).unwrap();
        let to = backend.adopt_host_pages(slot.address(), 0x8000).unwrap();
        let space = backend.create_address_space(&guesthouse_backend::GPU_RANGES).unwrap();
        backend.map(&work::testing::map_request(space, from, 0x400_0000, 0, 0x8000)).unwrap();
        backend.map(&work::testing::map_request(space, to, 0x500_0000, 0, 0x8000)).unwrap();
        let copy = SlotCopy {
            source: shape(128, 64, 512, Some(protocol::allocation::tile_mode::TILE4)),
            source_address: 0x400_0000,
            slot_address: 0x500_0000,
            slot_pitch: 512,
            width: 128,
            height: 64,
            own_space: space,
            slot_tiling: None,
        };
        let mut host = work::testing::FakeHost::default();
        assert_eq!(backend.start_slot_copy(&mut host, &copy), StagingStart::Running);
        assert_eq!(backend.poll_staging_copy(&mut host, Instant::now()), Some(Status::SUCCESS));
        assert_eq!(slot.bytes(), in_rows);
        let bigger = SlotCopy { width: 129, ..copy };
        assert_eq!(backend.start_slot_copy(&mut host, &bigger), StagingStart::Finished(Status::UNSUCCESSFUL));
    }

    #[test]
    fn a_decoder_is_given_the_cards_video_engines_in_turn_and_they_run_there() {
        use protocol::media_escape::EngineKind;
        use protocol::media_escape::tests::{question, video_decode_node};
        let card = card::tests::battlemage();
        let engines = card.engines.clone();
        let mut backend = IntelBackend::from_card(card).unwrap();
        let memory = || guesthouse_backend::VideoMemoryUse { total: 0, used: 0 };
        let processes = Vec::new;
        let books = GuestBooks { memory: &memory, processes: &processes };
        let ask = |backend: &mut IntelBackend, process| {
            let mut data = question(EngineKind::VideoDecode, 0, 0);
            backend.escape(process, &mut data, &books);
            video_decode_node(&data).unwrap()
        };
        let given = [ask(&mut backend, 0x4003_2A80), ask(&mut backend, 0x4004_B780)];
        assert_eq!(given, [1, 4], "never node 0, the 3D engine, where video commands do nothing");
        for node in given {
            let engine = work::engines::for_node(&engines, node).expect("a node the card runs");
            assert_eq!(engine.class, xe_query::Engine::VIDEO_DECODE);
        }
        backend.process_gone(0x4004_B780);
        assert_eq!(ask(&mut backend, 0x4004_B780), 4, "the gone process's engine is its turn again");
    }

    #[test]
    fn a_new_queues_data_goes_back_as_it_came_with_no_token_written() {
        let mut backend = IntelBackend::from_card(card::tests::battlemage()).unwrap();
        let mut data: Vec<u8> = (0..protocol::hw_queue::SIZE as u8).collect();
        let sent = data.clone();
        backend.finish_queue_data(0x40, &mut data, 1).unwrap();
        assert_eq!(data, sent, "the qword at +8 is written only for a native fence, which is never offered");
    }

    #[test]
    fn a_card_the_hosts_desktop_has_filled_leaves_the_guest_listening() {
        let mut busy = card::tests::battlemage();
        for region in busy.regions.iter_mut().filter(|region| region.class == xe_query::MemoryRegion::VRAM) {
            region.used = region.total - (3200 << 20);
        }
        let mut backend = IntelBackend::from_card(busy).unwrap();
        let xe = xe::fake::FakeXe::default();
        let open = || Ok(Box::new(xe.clone()) as Box<dyn Xe + Send>);
        backend.start_work_on_card(Stage::asked(None), open, None);
        assert_eq!(backend.stage(), Stage::Listen, "146 MiB past the reserve is no room for a guest");
        assert_eq!(backend.video_memory_cap(), None);
        assert_eq!(xe.live(), 0);
    }
}
