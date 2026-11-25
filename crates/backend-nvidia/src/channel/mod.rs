//! Real channels on the GPU, in the three forms this backend uses.
//!
//! A live channel runs a guest's own ring where it lies in guest memory. A list channel has its ring
//! here, for queues that hand over a list with each submission. A copy channel is the daemon's own.
//! All three share [`Channel`]: a small host area holding the channel's control block and a
//! prelude, the commands that bind the classes the guest's commands assume.

mod area;
pub mod copy;
mod group;
mod list;

pub use area::Area;
pub use copy::{COPY_AREA_EXTRA, COPY_WINDOW, CopyChannel};
pub use group::{ContextGroups, Left, Seat};
pub use list::{LIST_AREA, ListChannel, ListFull};

use crate::local_window::{
    COMPUTE_SUBCHANNEL, GuestPool, SET_LOCAL_MEMORY_ADDRESS, SET_LOCAL_MEMORY_SIZE, SET_LOCAL_MEMORY_UNITS,
    SET_LOCAL_MEMORY_WINDOW,
};
use crate::profile::{Generation, HostBranch};
use crate::pushbuffer::{increasing, ring_entry};
use crate::rm::classes::{
    CHANNEL_GROUP, CONTEXT_SHARE, CONTROL_DEFERRED_API, CONTROL_GET_CLASS_ENGINE,
    CONTROL_GET_WORK_SUBMIT_TOKEN, CONTROL_SCHEDULE_CHANNEL, CONTROL_SCHEDULE_CHANNEL_GROUP,
    CONTROL_SET_TIMESLICE, CONTROL_UPDATE_CHANNEL_INFO, DEFERRED_API, ENGINE_COPY_0, ENGINE_GRAPHICS,
};
use crate::rm::device::{AddressSpace, Device, Mapping};
use crate::rm::{Handle, Rm, RmError};
use guesthouse_backend::MemoryKind;

/// The host area of a channel on a guest's ring.
pub const AREA_SIZE: usize = 0x20000;
pub(crate) const CONTROL_BLOCK: usize = 0x10000;
const PRELUDE: usize = 0x11000;
const WORD_SIZE: u64 = 4;
pub(crate) const GP_PUT: usize = 0x8C;
const GP_GET: usize = 0x88;

const GROUP_PARAM_ERROR_CONTEXT: usize = 0x00;
const GROUP_PARAM_ADDRESS_SPACE: usize = 0x08;
const GROUP_PARAM_ENGINE: usize = 0x0C;
const SHARE_PARAM_ADDRESS_SPACE: usize = 0x00;
const SHARE_PARAM_FLAGS: usize = 0x04;
/// Graphics and video channels go on the synchronous subcontext, and copy channels on the other.
const SHARE_SYNCHRONOUS: u32 = 0;
const SHARE_ASYNCHRONOUS: u32 = 1;
/// An asynchronous subcontext from the lower ones, where the guest's driver asks for that.
const SHARE_ASYNCHRONOUS_PREFER_LOWER: u32 = 3;
const CHANNEL_PARAM_ERROR_CONTEXT: usize = 0x00;
const CHANNEL_PARAM_BUFFER: usize = 0x04;
pub(crate) const CHANNEL_PARAM_RING: usize = 0x08;
pub(crate) const CHANNEL_PARAM_RING_ENTRIES: usize = 0x10;
const CHANNEL_PARAM_FLAGS: usize = 0x14;
/// Puts a channel on its group's second runqueue. The RM takes the compute class on an
/// asynchronous subcontext of a group that also draws only on a channel made with this.
const CHANNEL_FLAG_SECOND_RUNQUEUE: u32 = 0x10;
const CHANNEL_PARAM_SHARE: usize = 0x18;
pub(crate) const CHANNEL_PARAM_CONTROL_MEMORY: usize = 0x24;
const CHANNEL_PARAM_CONTROL_OFFSET: usize = 0x48;
pub(crate) const CHANNEL_PARAM_ENGINE: usize = 0x88;

/// What a channel binds a software class with is not its class number but a number the RM gives
/// the object, which this control tells.
const CLASS_ENGINE_PARAMS: usize = 0x10;
const CLASS_ENGINE_PARAM_OBJECT: usize = 0x00;
const CLASS_ENGINE_PARAM_ID: usize = 0x04;

/// A deferred control, with a channel move inside it.
const DEFERRED_PARAMS: usize = 0x248;
const DEFERRED_PARAM_HANDLE: usize = 0x00;
const DEFERRED_PARAM_CONTROL: usize = 0x04;
const DEFERRED_PARAM_CLIENT: usize = 0x0C;
const DEFERRED_PARAM_DEVICE: usize = 0x10;
const MOVE_PARAM_CLIENT: usize = 0x18;
const MOVE_PARAM_CHANNEL: usize = 0x1C;
const MOVE_PARAM_CONTROL_MEMORY: usize = 0x20;
const MOVE_PARAM_RING_ENTRIES: usize = 0x24;
const MOVE_PARAM_RING: usize = 0x28;
const MOVE_PARAM_CONTROL_OFFSET: usize = 0x30;

/// The subchannel the host's own methods use, with no object bound, as they are the channel's.
pub(crate) const HOST_SUBCHANNEL: u32 = 6;
/// The four words of a memory operation start here, in the open kernel module's channel headers.
const METHOD_MEM_OP_A: u32 = 0x28;
const MEM_OP_WORDS: u32 = 4;
const MEM_OP_OPERATION_SHIFT: u32 = 27;
const MEM_OP_MEMBAR: u32 = 5;
const MEM_OP_L2_SYSMEM_INVALIDATE: u32 = 0xE;

/// A copy engine is reached through this subchannel, also on its own channel.
pub(crate) const COPY_SUBCHANNEL: u32 = 4;
/// So is a video engine, which is the only one on its channel.
pub(crate) const VIDEO_SUBCHANNEL: u32 = 4;
const VIDEO_CLASS_PARAMS: usize = 12;
const SET_OBJECT: u32 = 0;
const SCHEDULE_ENABLED: [u8; 3] = [1, 0, 0];
const SCHEDULE_CHANNEL_ENABLED: [u8; 2] = [1, 0];

/// Bytes of shader local memory a graphics channel starts with. A launch that needs more than the
/// channel has is failed by the GPU.
pub const LOCAL_MEMORY_BYTES: u64 = 32 << 20;
/// The pool's GPU address is aligned to this.
pub const LOCAL_MEMORY_ALIGN: u64 = 2 << 20;
/// The most units the pool is split between. The size the GPU is given is per unit (a TPC, as in
/// the open class headers and Mesa's NVK), so each unit gets the pool over this.
const LOCAL_MEMORY_SMS: u32 = 0x100;
/// A pool given to a channel that is already running goes here, with its own cache invalidate
/// after it. Well past the prelude, which is a few dozen words.
const LATE_POOL: usize = PRELUDE + 0x800;

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// What a channel is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Graphics,
    Copy,
    Video(VideoEngine),
}

/// One of the card's video engines: a decoder, an encoder or an optical flow engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoEngine {
    /// The RM's engine number.
    pub rm_engine: u32,
    pub class: u32,
    /// Which of its kind this is, counting from zero.
    pub instance: u32,
}

impl VideoEngine {
    /// The allocation parameters of the video classes: size, whether others may share, instance.
    fn class_params(&self) -> Vec<u8> {
        let words = [VIDEO_CLASS_PARAMS as u32, 0, self.instance];
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }
}

/// Where a channel's commands and ring are.
#[derive(Debug, Clone, Copy)]
pub struct ChannelPlace {
    pub ring_address: u64,
    pub ring_entries: u32,
    pub area_address: u64,
    pub area_size: usize,
}

/// See the module documentation.
#[derive(Debug)]
pub struct Channel {
    engine: Engine,
    area: Area,
    area_address: u64,
    memory: Handle,
    group: Handle,
    channel: Handle,
    /// The guest context whose group this channel is in, and its subcontext there. `None` for a
    /// channel with a group of its own.
    seat: Option<(u32, u8)>,
    /// A bit for each subchannel the prelude binds an object on.
    bound: u8,
    /// The object that runs deferred controls, which graphics channels have.
    deferred_api: Option<Handle>,
    /// The compute object, which graphics channels have. The RM maps it as a SKED page.
    compute: Option<Handle>,
    token: u32,
    ring_entries: u32,
    /// Words before the cache invalidate. Local memory setup stays out of the invalidate, so a
    /// later submission does not put the channel back to the pool it started with.
    setup_words: u32,
    prelude_words: u32,
    /// Shader local memory, mapped into the channel's address space.
    local_memory: Option<(Handle, u64, u64)>,
    /// Words at [`LATE_POOL`] that the next submission still has to run.
    late_pool_words: Option<u32>,
}

/// Drops every line of host memory from the GPU's L2, with a system membar either side.
///
/// Guest pages are host memory, which the guest's CPU writes without the L2 seeing it, and guests
/// map much of it cached. So a line the L2 still holds can be older than what was just written,
/// which shows as a draw with another draw's vertices. It belongs on the GPU, in order, as anything
/// the CPU had to wait for would come out of every submission.
pub(crate) fn l2_invalidate_words() -> Vec<u32> {
    [MEM_OP_MEMBAR, MEM_OP_L2_SYSMEM_INVALIDATE, MEM_OP_MEMBAR]
        .iter()
        .flat_map(|&operation| {
            let header = increasing(HOST_SUBCHANNEL, METHOD_MEM_OP_A, MEM_OP_WORDS);
            [header, 0, 0, 0, operation << MEM_OP_OPERATION_SHIFT]
        })
        .collect()
}

/// After scheduling and before any work, as this can force a channel off the GPU. The channel runs
/// fine on the RM's own slice, so a refusal only costs some speed.
fn set_timeslice<R: Rm>(device: &mut Device<R>, group: Handle, micros: u64) {
    if micros != 0
        && let Err(error) = device.control(group, CONTROL_SET_TIMESLICE, &mut micros.to_le_bytes())
    {
        log::warn!("no timeslice of {micros} µs: {error}");
    }
}

impl Channel {
    pub fn new<R: Rm>(
        device: &mut Device<R>,
        generation: &Generation,
        sizes: &HostBranch,
        space: &AddressSpace,
        place: ChannelPlace,
        engine: Engine,
    ) -> Result<Self, RmError> {
        let area = Area::new(place.area_size).map_err(|error| RmError::System(error.to_string()))?;
        let size = place.area_size as u64;
        let memory = device.alloc_host_pages(area.host_address(), size)?;
        device.map(
            space,
            &Mapping { memory, length: size, address: place.area_address, ..Mapping::default() },
        )?;
        map_doorbell_once(device, generation)?;

        let zeros = |&(class, size): &(u32, usize)| (class, vec![0; size]);
        let (engine_number, classes): (u32, Vec<(u32, Vec<u8>)>) = match engine {
            Engine::Graphics => (ENGINE_GRAPHICS, generation.graphics_classes.iter().map(zeros).collect()),
            Engine::Copy => (ENGINE_COPY_0, vec![(generation.copy_class, vec![0; 8])]),
            Engine::Video(video) => (video.rm_engine, vec![(video.class, video.class_params())]),
        };
        let mut params = vec![0; sizes.channel_group_params];
        put32(&mut params, GROUP_PARAM_ERROR_CONTEXT, memory);
        put32(&mut params, GROUP_PARAM_ADDRESS_SPACE, space.handle);
        put32(&mut params, GROUP_PARAM_ENGINE, engine_number);
        let group = device.alloc(device.device, CHANNEL_GROUP, &mut params)?;

        let mut params = vec![0; sizes.context_share_params];
        put32(&mut params, SHARE_PARAM_ADDRESS_SPACE, space.handle);
        let share_flags = if engine == Engine::Copy { SHARE_ASYNCHRONOUS } else { SHARE_SYNCHRONOUS };
        put32(&mut params, SHARE_PARAM_FLAGS, share_flags);
        let share = device.alloc(group, CONTEXT_SHARE, &mut params)?;

        let mut params = vec![0; sizes.channel_params];
        put32(&mut params, CHANNEL_PARAM_ERROR_CONTEXT, memory);
        put32(&mut params, CHANNEL_PARAM_BUFFER, memory);
        put64(&mut params, CHANNEL_PARAM_RING, place.ring_address);
        put32(&mut params, CHANNEL_PARAM_RING_ENTRIES, place.ring_entries);
        put32(&mut params, CHANNEL_PARAM_SHARE, share);
        put32(&mut params, CHANNEL_PARAM_CONTROL_MEMORY, memory);
        put64(&mut params, CHANNEL_PARAM_CONTROL_OFFSET, CONTROL_BLOCK as u64);
        put32(&mut params, CHANNEL_PARAM_ENGINE, engine_number);
        let channel = device.alloc(group, generation.channel_class, &mut params)?;

        let first = match engine {
            Engine::Graphics => 0,
            Engine::Copy => COPY_SUBCHANNEL,
            Engine::Video(_) => VIDEO_SUBCHANNEL,
        };
        let mut classes: Vec<SubchannelClass> =
            (first..).zip(classes).map(|(subchannel, (class, params))| (subchannel, class, params)).collect();
        let objects = Objects::make(device, generation, channel, &mut classes)?;
        let mut token = [0; 4];
        device.control(channel, CONTROL_GET_WORK_SUBMIT_TOKEN, &mut token)?;
        let mut schedule = SCHEDULE_ENABLED;
        device.control(group, CONTROL_SCHEDULE_CHANNEL_GROUP, &mut schedule)?;
        if engine == Engine::Graphics {
            set_timeslice(device, group, device.graphics_timeslice_micros);
        }

        let (words, setup_words) = objects.prelude();
        area.store_words(PRELUDE, &words);
        Ok(Self {
            engine,
            area,
            area_address: place.area_address,
            memory,
            group,
            channel,
            seat: None,
            bound: objects.bound(),
            deferred_api: objects.deferred_api,
            compute: objects.compute,
            token: u32::from_le_bytes(token),
            ring_entries: place.ring_entries,
            setup_words,
            prelude_words: words.len() as u32,
            local_memory: None,
            late_pool_words: None,
        })
    }

    /// A graphics channel in its guest context's group, on the seat its queue's private data
    /// names. The first channel of a context makes the group, and the first on a subcontext makes
    /// that. If anything is refused, all this made is freed again and nothing of the group has
    /// changed, so the caller can make the channel in a group of its own.
    pub fn new_in_context<R: Rm>(
        device: &mut Device<R>,
        generation: &Generation,
        sizes: &HostBranch,
        space: &AddressSpace,
        place: ChannelPlace,
        context: u32,
        seat: Seat,
    ) -> Result<Self, RmError> {
        if seat.slot() == 0 && seat != Seat::Drawing {
            return Err(RmError::System("the first subcontext is the drawing channel's".to_owned()));
        }
        let area = Area::new(place.area_size).map_err(|error| RmError::System(error.to_string()))?;
        let size = place.area_size as u64;
        let memory = device.alloc_host_pages(area.host_address(), size)?;
        let mapping = Mapping { memory, length: size, address: place.area_address, ..Mapping::default() };
        if let Err(error) = device.map(space, &mapping) {
            let _ = device.free(memory);
            return Err(error);
        }
        let mut made = Vec::new();
        let seated = map_doorbell_once(device, generation).and_then(|()| {
            Seated::make(device, generation, sizes, space, &place, memory, (context, seat), &mut made)
        });
        let seated = match seated {
            Ok(seated) => seated,
            Err(error) => {
                // Newest first: the channel, a subcontext it made, a group it made.
                for handle in made.into_iter().rev() {
                    if let Err(error) = device.free(handle) {
                        log::warn!("a refused channel left {handle:#x} behind: {error}");
                    }
                }
                if let Err(error) = device.unmap(space, memory, place.area_address, size) {
                    log::warn!("a refused channel's area stays mapped: {error}");
                }
                let _ = device.free(memory);
                return Err(error);
            }
        };
        device.context_groups.join(context, seated.group, seat.slot(), seated.share);
        let objects = seated.objects;
        let (words, setup_words) = objects.prelude();
        area.store_words(PRELUDE, &words);
        Ok(Self {
            engine: Engine::Graphics,
            area,
            area_address: place.area_address,
            memory,
            group: seated.group,
            channel: seated.channel,
            seat: Some((context, seat.slot())),
            bound: objects.bound(),
            deferred_api: objects.deferred_api,
            compute: objects.compute,
            token: seated.token,
            ring_entries: place.ring_entries,
            setup_words,
            prelude_words: words.len() as u32,
            local_memory: None,
            late_pool_words: None,
        })
    }

    /// Whether this channel shares its group with its context's other channels.
    pub fn in_context_group(&self) -> bool {
        self.seat.is_some()
    }

    /// The subchannels with an object, as a bit each, for a channel that binds fewer classes than
    /// the guest's driver has on its own. Commands for any other subchannel would fault there.
    pub fn only_bound(&self) -> Option<u8> {
        (self.seat.is_some_and(|(_, slot)| slot != 0)).then_some(self.bound)
    }

    /// The compute object, for a graphics channel.
    pub fn compute_object(&self) -> Option<Handle> {
        self.compute
    }

    /// A slice of its own for this channel's group. Only before the channel has been given work, as
    /// for the slice set when it was made.
    pub fn set_timeslice<R: Rm>(&self, device: &mut Device<R>, micros: u64) {
        set_timeslice(device, self.group, micros);
    }

    /// Maps a shader local-memory pool and names it in the prelude, ahead of the cache invalidate.
    /// Only before the prelude has been handed out. Graphics channels only. A channel that never
    /// gets one has a pool of nothing, and a compute launch that needs local memory is failed.
    /// `window` is where the guest's driver puts shaders' local addresses.
    pub fn add_local_memory<R: Rm>(
        &mut self,
        device: &mut Device<R>,
        space: &AddressSpace,
        address: u64,
        window: u64,
    ) -> Result<(), RmError> {
        let Some(extra) = self.map_local_memory(device, space, address, window)? else { return Ok(()) };
        let mut words = self.area.load_words(PRELUDE, self.prelude_words as usize);
        let at = self.setup_words as usize;
        words.splice(at..at, extra.iter().copied());
        self.area.store_words(PRELUDE, &words);
        self.setup_words += extra.len() as u32;
        self.prelude_words = words.len() as u32;
        Ok(())
    }

    /// The same for a channel that is already running. The prelude and the invalidate stay as they
    /// are, as the GPU may still be reading them, and the pool goes ahead of the next submission.
    pub fn add_late_local_memory<R: Rm>(
        &mut self,
        device: &mut Device<R>,
        space: &AddressSpace,
        address: u64,
        window: u64,
    ) -> Result<(), RmError> {
        let Some(mut words) = self.map_local_memory(device, space, address, window)? else { return Ok(()) };
        words.extend(l2_invalidate_words());
        self.area.store_words(LATE_POOL, &words);
        self.late_pool_words = Some(words.len() as u32);
        Ok(())
    }

    /// Points a running channel at a pool the guest's driver made, ahead of its next submission.
    /// Comes after a pool of ours that hasn't gone yet, so the guest's wins.
    pub(crate) fn add_late_pool_methods(&mut self, pool: &GuestPool) {
        if self.engine != Engine::Graphics {
            return;
        }
        let mut words = match self.late_pool_words {
            Some(count) => self.area.load_words(LATE_POOL, count as usize),
            None => Vec::new(),
        };
        words.extend(local_memory_methods(pool));
        words.extend(l2_invalidate_words());
        self.area.store_words(LATE_POOL, &words);
        self.late_pool_words = Some(words.len() as u32);
    }

    /// Allocates and maps the pool, and gives the methods that name it. `None` if this channel
    /// takes no pool.
    fn map_local_memory<R: Rm>(
        &mut self,
        device: &mut Device<R>,
        space: &AddressSpace,
        address: u64,
        window: u64,
    ) -> Result<Option<Vec<u32>>, RmError> {
        if self.engine != Engine::Graphics || self.local_memory.is_some() {
            return Ok(None);
        }
        let (handle, given) = device.alloc_video_memory(LOCAL_MEMORY_BYTES, MemoryKind::PLAIN)?;
        let mapped = device.map(
            space,
            &Mapping {
                memory: handle,
                offset: 0,
                length: given,
                address,
                small_pages: false,
                gpu_cached: true,
                page_kind: None,
            },
        );
        if let Err(error) = mapped {
            let _ = device.free(handle);
            return Err(error);
        }
        self.local_memory = Some((handle, address, given));
        log::info!("shader local memory {given:#x} bytes at {address:#x}");
        let pool = GuestPool {
            address,
            per_unit: given / u64::from(LOCAL_MEMORY_SMS),
            units: LOCAL_MEMORY_SMS,
            window,
        };
        Ok(Some(local_memory_methods(&pool)))
    }

    /// Whether this channel has a pool, in its prelude or still to come.
    pub fn has_local_memory(&self) -> bool {
        self.local_memory.is_some()
    }

    pub fn engine(&self) -> Engine {
        self.engine
    }

    pub fn area(&self) -> &Area {
        &self.area
    }

    pub fn area_address(&self) -> u64 {
        self.area_address
    }

    pub fn ring_entries(&self) -> u32 {
        self.ring_entries
    }

    pub fn token(&self) -> u32 {
        self.token
    }

    /// How far through the ring the GPU says it has got.
    pub fn gp_get(&self) -> u32 {
        self.area.load(CONTROL_BLOCK + GP_GET)
    }

    /// The channel's own handle, for anything that binds to the channel.
    pub fn handle(&self) -> Handle {
        self.channel
    }

    /// The ring entry that runs the prelude, in the same form as the guest's entries.
    pub fn prelude_entry(&self) -> u64 {
        ring_entry(self.area_address + PRELUDE as u64, self.prelude_words)
    }

    /// The ring entry that runs the L2 invalidate.
    pub fn l2_invalidate_entry(&self) -> u64 {
        let address = self.area_address + PRELUDE as u64 + u64::from(self.setup_words) * WORD_SIZE;
        ring_entry(address, self.prelude_words - self.setup_words)
    }

    /// The ring entry to go ahead of a submission: the L2 invalidate, with a pool given late the
    /// first time. Call [`Self::entry_ahead_placed`] once the entry is on the ring.
    pub fn entry_ahead(&self) -> u64 {
        match self.late_pool_words {
            Some(words) => ring_entry(self.area_address + LATE_POOL as u64, words),
            None => self.l2_invalidate_entry(),
        }
    }

    /// The entry from [`Self::entry_ahead`] is on the ring, so a late pool has been sent.
    pub fn entry_ahead_placed(&mut self) {
        self.late_pool_words = None;
    }

    /// Keeps a move to another ring for the GPU to make when this channel reaches the method
    /// that names it, which the guest's driver puts at the end of the old ring. `handle` is what the
    /// driver was given for the new ring, and the move goes once it has run.
    pub fn register_move<R: Rm>(
        &self,
        device: &mut Device<R>,
        handle: Handle,
        ring_address: u64,
        ring_entries: u32,
    ) -> Result<(), RmError> {
        let Some(deferred_api) = self.deferred_api else {
            return Err(RmError::System("this channel runs no deferred controls".into()));
        };
        let mut params = vec![0; DEFERRED_PARAMS];
        put32(&mut params, DEFERRED_PARAM_HANDLE, handle);
        put32(&mut params, DEFERRED_PARAM_CONTROL, CONTROL_UPDATE_CHANNEL_INFO);
        put32(&mut params, DEFERRED_PARAM_CLIENT, device.client);
        put32(&mut params, DEFERRED_PARAM_DEVICE, device.device);
        put32(&mut params, MOVE_PARAM_CLIENT, device.client);
        put32(&mut params, MOVE_PARAM_CHANNEL, self.channel);
        put32(&mut params, MOVE_PARAM_CONTROL_MEMORY, self.memory);
        put32(&mut params, MOVE_PARAM_RING_ENTRIES, ring_entries);
        put64(&mut params, MOVE_PARAM_RING, ring_address);
        put64(&mut params, MOVE_PARAM_CONTROL_OFFSET, CONTROL_BLOCK as u64);
        device.control(deferred_api, CONTROL_DEFERRED_API, &mut params)
    }

    /// Positions count on the new ring from now on.
    pub fn follow_ring(&mut self, ring_entries: u32) {
        self.ring_entries = ring_entries;
    }

    /// Tells the GPU the ring is filled up to `position`. This is what starts commands.
    pub fn advance<R: Rm>(&self, device: &mut Device<R>, position: u32) {
        // One store. If the GPU read a half-written field it could take the ring as wrapped, and
        // run on past the real end.
        self.area.store(CONTROL_BLOCK + GP_PUT, position % self.ring_entries);
        device.rm.ring_doorbell(self.token);
    }

    /// Frees the channel. A guest's ring stays, because it belongs to the guest.
    pub fn close<R: Rm>(self, device: &mut Device<R>, space: &AddressSpace) -> Result<(), RmError> {
        if let Some((handle, address, length)) = self.local_memory {
            device.unmap(space, handle, address, length)?;
            device.free(handle)?;
        }
        match self.seat {
            None => device.free(self.group)?,
            Some((context, slot)) => {
                // The channel, then what it was the last to use. The books are settled first, so a
                // refused free leaves no seat that a later channel would try to sit on.
                let left = device.context_groups.leave(context, slot);
                let freed: Vec<Result<(), RmError>> = [Some(self.channel), left.share, left.group]
                    .into_iter()
                    .flatten()
                    .map(|handle| device.free(handle))
                    .collect();
                freed.into_iter().collect::<Result<(), RmError>>()?;
            }
        }
        device.unmap(space, self.memory, self.area_address, self.area.size() as u64)?;
        device.free(self.memory)
    }
}

/// A class for a channel, with the subchannel the guest's commands reach it on and its parameters.
type SubchannelClass = (u32, u32, Vec<u8>);

/// The objects a channel was given.
struct Objects {
    deferred_api: Option<Handle>,
    compute: Option<Handle>,
    /// Each subchannel, and what binds its object there.
    bindings: Vec<(u32, u32)>,
}

impl Objects {
    fn make<R: Rm>(
        device: &mut Device<R>,
        generation: &Generation,
        channel: Handle,
        classes: &mut [SubchannelClass],
    ) -> Result<Self, RmError> {
        let mut objects =
            Self { deferred_api: None, compute: None, bindings: Vec::with_capacity(classes.len()) };
        for (subchannel, class, params) in classes {
            let object = device.alloc(channel, *class, params)?;
            let mut binding = *class;
            if *class == DEFERRED_API {
                objects.deferred_api = Some(object);
                binding = class_engine_id(device, channel, object).unwrap_or(*class);
            }
            if *class == generation.compute_class {
                objects.compute = Some(object);
            }
            objects.bindings.push((*subchannel, binding));
        }
        Ok(objects)
    }

    /// The prelude's words, and how many of them come before the cache invalidate. Local memory
    /// is inserted between the class binds and the invalidate.
    fn prelude(&self) -> (Vec<u32>, u32) {
        let mut words = Vec::with_capacity(self.bindings.len() * 2);
        for &(subchannel, binding) in &self.bindings {
            words.extend([increasing(subchannel, SET_OBJECT, 1), binding]);
        }
        let setup_words = words.len() as u32;
        words.extend(l2_invalidate_words());
        (words, setup_words)
    }

    fn bound(&self) -> u8 {
        self.bindings.iter().fold(0, |bound, &(subchannel, _)| bound | 1 << subchannel)
    }
}

/// A channel made in its context's group.
struct Seated {
    group: Handle,
    share: Handle,
    channel: Handle,
    objects: Objects,
    token: u32,
}

impl Seated {
    /// Makes the group and the subcontext unless the context has them, then the channel and its
    /// objects, and starts the channel. Every handle made goes on `made` as it is made, for the
    /// caller to free if a later step is refused.
    #[allow(clippy::too_many_arguments)]
    fn make<R: Rm>(
        device: &mut Device<R>,
        generation: &Generation,
        sizes: &HostBranch,
        space: &AddressSpace,
        place: &ChannelPlace,
        memory: Handle,
        (context, seat): (u32, Seat),
        made: &mut Vec<Handle>,
    ) -> Result<Self, RmError> {
        let running = device.context_groups.group(context);
        let group = match running {
            Some(group) => group,
            None => {
                // No error context: the group outlives its first channel, whose memory that would
                // be. Each channel has its own.
                let mut params = vec![0; sizes.channel_group_params];
                put32(&mut params, GROUP_PARAM_ADDRESS_SPACE, space.handle);
                put32(&mut params, GROUP_PARAM_ENGINE, ENGINE_GRAPHICS);
                let group = device.alloc(device.device, CHANNEL_GROUP, &mut params)?;
                made.push(group);
                group
            }
        };
        let share = match device.context_groups.share(context, seat.slot()) {
            Some(share) => share,
            None => {
                let flags = match seat {
                    Seat::Drawing => SHARE_SYNCHRONOUS,
                    Seat::Compute { prefer_lower: false, .. } => SHARE_ASYNCHRONOUS,
                    Seat::Compute { prefer_lower: true, .. } => SHARE_ASYNCHRONOUS_PREFER_LOWER,
                };
                let mut params = vec![0; sizes.context_share_params];
                put32(&mut params, SHARE_PARAM_ADDRESS_SPACE, space.handle);
                put32(&mut params, SHARE_PARAM_FLAGS, flags);
                let share = device.alloc(group, CONTEXT_SHARE, &mut params)?;
                made.push(share);
                share
            }
        };
        let computes_only = matches!(seat, Seat::Compute { .. });
        let mut params = vec![0; sizes.channel_params];
        put32(&mut params, CHANNEL_PARAM_ERROR_CONTEXT, memory);
        put32(&mut params, CHANNEL_PARAM_BUFFER, memory);
        put64(&mut params, CHANNEL_PARAM_RING, place.ring_address);
        put32(&mut params, CHANNEL_PARAM_RING_ENTRIES, place.ring_entries);
        if computes_only {
            put32(&mut params, CHANNEL_PARAM_FLAGS, CHANNEL_FLAG_SECOND_RUNQUEUE);
        }
        put32(&mut params, CHANNEL_PARAM_SHARE, share);
        put32(&mut params, CHANNEL_PARAM_CONTROL_MEMORY, memory);
        put64(&mut params, CHANNEL_PARAM_CONTROL_OFFSET, CONTROL_BLOCK as u64);
        put32(&mut params, CHANNEL_PARAM_ENGINE, ENGINE_GRAPHICS);
        let channel = device.alloc(group, generation.channel_class, &mut params)?;
        made.push(channel);

        // Each class keeps the subchannel the guest's commands assume, whatever is left out.
        let kept = |class: u32| !computes_only || class == generation.compute_class || class == DEFERRED_API;
        let mut classes: Vec<SubchannelClass> = (0..)
            .zip(generation.graphics_classes)
            .filter(|(_, (class, _))| kept(*class))
            .map(|(subchannel, &(class, size))| (subchannel, class, vec![0; size]))
            .collect();
        let objects = Objects::make(device, generation, channel, &mut classes)?;
        let mut token = [0; 4];
        device.control(channel, CONTROL_GET_WORK_SUBMIT_TOKEN, &mut token)?;
        match running {
            None => {
                let mut schedule = SCHEDULE_ENABLED;
                device.control(group, CONTROL_SCHEDULE_CHANNEL_GROUP, &mut schedule)?;
                set_timeslice(device, group, device.graphics_timeslice_micros);
            }
            Some(_) => {
                let mut schedule = SCHEDULE_CHANNEL_ENABLED;
                if let Err(error) = device.control(channel, CONTROL_SCHEDULE_CHANNEL, &mut schedule) {
                    log::debug!("a channel that joined a group starts with the group instead: {error}");
                    let mut schedule = SCHEDULE_ENABLED;
                    device.control(group, CONTROL_SCHEDULE_CHANNEL_GROUP, &mut schedule)?;
                }
            }
        }
        Ok(Self { group, share, channel, objects, token: u32::from_le_bytes(token) })
    }
}

/// The compute methods that point a channel at a local-memory pool. The size is per unit, so the
/// GPU takes `per_unit` times the units it has, up to `units`.
pub(crate) fn local_memory_methods(pool: &GuestPool) -> Vec<u32> {
    let subchannel = COMPUTE_SUBCHANNEL;
    let GuestPool { address, per_unit, units, window } = *pool;
    let mut words = Vec::with_capacity(11);
    words.extend([
        increasing(subchannel, SET_LOCAL_MEMORY_ADDRESS, 2),
        (address >> 32) as u32,
        address as u32,
    ]);
    words.extend([
        increasing(subchannel, SET_LOCAL_MEMORY_SIZE, 2),
        (per_unit >> 32) as u32,
        per_unit as u32,
    ]);
    words.extend([increasing(subchannel, SET_LOCAL_MEMORY_WINDOW, 2), (window >> 32) as u32, window as u32]);
    words.extend([increasing(subchannel, SET_LOCAL_MEMORY_UNITS, 1), units]);
    words
}

/// What binds an object to a subchannel.
fn class_engine_id<R: Rm>(device: &mut Device<R>, channel: Handle, object: Handle) -> Option<u32> {
    let mut params = [0; CLASS_ENGINE_PARAMS];
    put32(&mut params, CLASS_ENGINE_PARAM_OBJECT, object);
    match device.control(channel, CONTROL_GET_CLASS_ENGINE, &mut params) {
        Ok(()) => Some(u32::from_le_bytes(
            params[CLASS_ENGINE_PARAM_ID..CLASS_ENGINE_PARAM_ID + 4].try_into().ok()?,
        )),
        Err(error) => {
            log::debug!("an object binds by its class, as the RM gave no number: {error}");
            None
        }
    }
}

fn map_doorbell_once<R: Rm>(device: &mut Device<R>, generation: &Generation) -> Result<(), RmError> {
    if device.doorbell_mapped {
        return Ok(());
    }
    let usermode = device.alloc(device.subdevice, generation.usermode_class, &mut [])?;
    device.rm.map_doorbell(device.subdevice, usermode)?;
    device.doorbell_mapped = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{BLACKWELL, HOST_615};
    use crate::pushbuffer::RingEntry;
    use crate::rm::fake::{Call, FakeRm};

    const AREA_ADDRESS: u64 = 0x800_0000;
    /// A made-up window, as a guest's driver would set it.
    const WINDOW: u64 = 0x1234_5600_0000;

    fn device() -> (Device<FakeRm>, AddressSpace) {
        let mut device = Device::open(FakeRm::new(), &HOST_615).unwrap();
        let space = device.create_address_space(&[(0x400_0000, 0x1_0000_0000)]).unwrap();
        (device, space)
    }

    fn place() -> ChannelPlace {
        ChannelPlace {
            ring_address: 0x900_0000,
            ring_entries: 1024,
            area_address: AREA_ADDRESS,
            area_size: AREA_SIZE,
        }
    }

    fn channel(device: &mut Device<FakeRm>, space: &AddressSpace, engine: Engine) -> Channel {
        Channel::new(device, &BLACKWELL, &HOST_615, space, place(), engine).unwrap()
    }

    #[test]
    fn a_move_is_a_deferred_control_naming_the_handle_and_the_new_ring() {
        let (mut device, space) = device();
        let channel = channel(&mut device, &space, Engine::Graphics);
        channel.register_move(&mut device, 0xC1D0_0042, 0xA00_0000, 0x1_0000).unwrap();
        let Some(Call::Control { object, command, params }) = device.rm.calls.last() else {
            panic!("no control was sent");
        };
        assert_eq!(*object, channel.deferred_api.unwrap());
        assert_eq!(*command, CONTROL_DEFERRED_API);
        assert_eq!(params.len(), DEFERRED_PARAMS);
        let word = |at: usize| u32::from_le_bytes(params[at..at + 4].try_into().unwrap());
        let long = |at: usize| u64::from_le_bytes(params[at..at + 8].try_into().unwrap());
        assert_eq!(word(DEFERRED_PARAM_HANDLE), 0xC1D0_0042);
        assert_eq!(word(DEFERRED_PARAM_CONTROL), CONTROL_UPDATE_CHANNEL_INFO);
        assert_eq!(word(MOVE_PARAM_CHANNEL), channel.channel);
        assert_eq!(word(MOVE_PARAM_CONTROL_MEMORY), channel.memory);
        assert_eq!(word(MOVE_PARAM_RING_ENTRIES), 0x1_0000);
        assert_eq!(long(MOVE_PARAM_RING), 0xA00_0000);
        assert_eq!(long(MOVE_PARAM_CONTROL_OFFSET), CONTROL_BLOCK as u64);
    }

    #[test]
    fn a_copy_channel_has_no_move_to_offer() {
        let (mut device, space) = device();
        let channel = channel(&mut device, &space, Engine::Copy);
        assert!(channel.register_move(&mut device, 1, 0xA00_0000, 0x1_0000).is_err());
    }

    #[test]
    fn a_graphics_channel_binds_the_classes_the_guest_assumes_in_order() {
        let (mut device, space) = device();
        let channel = channel(&mut device, &space, Engine::Graphics);
        let words = channel.area().load_words(PRELUDE, 4);
        assert_eq!(words[0], 0x2001_0000);
        assert_eq!(words[1], BLACKWELL.graphics_classes[0].0);
        assert_eq!(words[2], 0x2001_2000);
        let invalidate = 3 * (MEM_OP_WORDS + 1) as u64;
        assert_eq!(channel.prelude_entry() >> 42, 2 * BLACKWELL.graphics_classes.len() as u64 + invalidate);
        assert_eq!(channel.l2_invalidate_entry() >> 42, invalidate);
    }

    #[test]
    fn a_graphics_channel_names_its_local_memory() {
        let (mut device, space) = device();
        let mut channel = channel(&mut device, &space, Engine::Graphics);
        let invalidate = channel.l2_invalidate_entry() >> 42;
        let address = 0x5000_0000;
        channel.add_local_memory(&mut device, &space, address, WINDOW).unwrap();
        let words = channel.area().load_words(PRELUDE, channel.prelude_words as usize);
        assert!(words.windows(2).any(|pair| pair == [0, address as u32]), "{words:x?}");
        let window = [(WINDOW >> 32) as u32, WINDOW as u32];
        assert!(words.windows(2).any(|pair| pair == window), "{words:x?}");
        assert_eq!(channel.l2_invalidate_entry() >> 42, invalidate);
        let mapped = device.rm.calls.iter().any(|call| {
            matches!(call, Call::MapDma(map) if map.address == address && map.length >= LOCAL_MEMORY_BYTES)
        });
        assert!(mapped, "the pool is mapped at the address the methods name");
        channel.close(&mut device, &space).unwrap();
    }

    #[test]
    fn a_late_pool_runs_once_ahead_of_the_next_submission_and_leaves_the_prelude_alone() {
        let (mut device, space) = device();
        let mut channel = channel(&mut device, &space, Engine::Graphics);
        let (prelude, invalidate) = (channel.prelude_entry(), channel.l2_invalidate_entry());
        let prelude_words = channel.area().load_words(PRELUDE, channel.prelude_words as usize);
        assert_eq!(channel.entry_ahead(), invalidate);
        let address = 0x5000_0000;
        channel.add_late_local_memory(&mut device, &space, address, WINDOW).unwrap();
        assert!(channel.has_local_memory());
        assert_eq!((channel.prelude_entry(), channel.l2_invalidate_entry()), (prelude, invalidate));
        assert_eq!(channel.area().load_words(PRELUDE, channel.prelude_words as usize), prelude_words);

        let ahead = RingEntry::parse(channel.entry_ahead().to_le_bytes());
        let words = channel.area().load_words((ahead.address - AREA_ADDRESS) as usize, ahead.length as usize);
        assert_eq!(
            words[..3],
            [increasing(COMPUTE_SUBCHANNEL, SET_LOCAL_MEMORY_ADDRESS, 2), 0, address as u32]
        );
        assert!(words.windows(2).any(|pair| pair == [(WINDOW >> 32) as u32, WINDOW as u32]), "{words:x?}");
        assert!(words.ends_with(&l2_invalidate_words()), "the late pool still drops the L2");
        channel.entry_ahead_placed();
        assert_eq!(channel.entry_ahead(), invalidate);
        channel.close(&mut device, &space).unwrap();
    }

    #[test]
    fn our_pool_is_split_between_its_units() {
        let (mut device, space) = device();
        let mut channel = channel(&mut device, &space, Engine::Graphics);
        channel.add_late_local_memory(&mut device, &space, 0x5000_0000, WINDOW).unwrap();
        let ahead = RingEntry::parse(channel.entry_ahead().to_le_bytes());
        let words = channel.area().load_words((ahead.address - AREA_ADDRESS) as usize, ahead.length as usize);
        let per_unit = LOCAL_MEMORY_BYTES / u64::from(LOCAL_MEMORY_SMS);
        let size = [increasing(COMPUTE_SUBCHANNEL, SET_LOCAL_MEMORY_SIZE, 2), 0, per_unit as u32];
        assert!(words.windows(3).any(|three| three == size), "{words:x?}");
        let units = [increasing(COMPUTE_SUBCHANNEL, SET_LOCAL_MEMORY_UNITS, 1), LOCAL_MEMORY_SMS];
        assert!(words.windows(2).any(|two| two == units), "{words:x?}");
        channel.close(&mut device, &space).unwrap();
    }

    #[test]
    fn a_guest_pool_goes_after_ours_and_before_the_invalidate() {
        let (mut device, space) = device();
        let mut channel = channel(&mut device, &space, Engine::Graphics);
        channel.add_late_local_memory(&mut device, &space, 0x5000_0000, WINDOW).unwrap();
        let pool = GuestPool { address: 0x5_0580_0000, per_unit: 0x3F_0000, units: 0x55, window: WINDOW };
        channel.add_late_pool_methods(&pool);
        let ahead = RingEntry::parse(channel.entry_ahead().to_le_bytes());
        let words = channel.area().load_words((ahead.address - AREA_ADDRESS) as usize, ahead.length as usize);
        let theirs = local_memory_methods(&pool);
        let at =
            words.windows(theirs.len()).rposition(|run| run == theirs.as_slice()).expect("the guest's pool");
        assert!(at > 0, "ours comes first");
        assert!(words[at + theirs.len()..].ends_with(&l2_invalidate_words()));
        channel.entry_ahead_placed();
        channel.close(&mut device, &space).unwrap();
    }

    #[test]
    fn a_channel_takes_one_pool_only() {
        let (mut device, space) = device();
        let mut channel = channel(&mut device, &space, Engine::Graphics);
        channel.add_local_memory(&mut device, &space, 0x5000_0000, WINDOW).unwrap();
        let calls = device.rm.calls.len();
        channel.add_late_local_memory(&mut device, &space, 0x5200_0000, WINDOW).unwrap();
        assert_eq!(device.rm.calls.len(), calls);
        assert_eq!(channel.entry_ahead(), channel.l2_invalidate_entry());
    }

    #[test]
    fn a_closed_channel_gives_its_local_memory_back() {
        let (mut device, space) = device();
        let mut channel = channel(&mut device, &space, Engine::Graphics);
        let address = 0x5000_0000;
        channel.add_local_memory(&mut device, &space, address, WINDOW).unwrap();
        let (pool, _, _) = channel.local_memory.unwrap();
        channel.close(&mut device, &space).unwrap();
        let calls = &device.rm.calls;
        let unmapped = calls.iter().any(|call| matches!(call, Call::UnmapDma { memory, address: at, .. } if *memory == pool && *at == address));
        assert!(unmapped, "the pool is unmapped");
        assert!(
            calls.iter().any(|call| matches!(call, Call::Free { handle, .. } if *handle == pool)),
            "and freed"
        );
    }

    #[test]
    fn a_pool_that_cannot_be_mapped_is_freed_and_left_out() {
        let (mut device, space) = device();
        let mut channel = channel(&mut device, &space, Engine::Graphics);
        let before = channel.area().load_words(PRELUDE, channel.prelude_words as usize);
        let address = 0x5000_0000;
        device.rm.refuse_map_at = Some(address);
        assert!(channel.add_local_memory(&mut device, &space, address, WINDOW).is_err());
        assert_eq!(channel.area().load_words(PRELUDE, channel.prelude_words as usize), before);
        assert!(channel.local_memory.is_none());
        assert!(matches!(device.rm.calls.last(), Some(Call::Free { .. })), "the pool's memory is freed");
    }

    #[test]
    fn copy_and_video_channels_get_no_local_memory() {
        let (mut device, space) = device();
        let mut channel = channel(&mut device, &space, Engine::Copy);
        let before = channel.area().load_words(PRELUDE, channel.prelude_words as usize);
        let calls = device.rm.calls.len();
        channel.add_local_memory(&mut device, &space, 0x5000_0000, WINDOW).unwrap();
        channel.add_late_local_memory(&mut device, &space, 0x5000_0000, WINDOW).unwrap();
        assert_eq!(channel.entry_ahead(), channel.l2_invalidate_entry());
        assert_eq!(channel.area().load_words(PRELUDE, channel.prelude_words as usize), before);
        assert_eq!(device.rm.calls.len(), calls, "nothing is allocated or mapped");
    }

    fn timeslices_asked_for(device: &Device<FakeRm>) -> Vec<u64> {
        let asked = device.rm.calls.iter().filter_map(|call| match call {
            Call::Control { command: CONTROL_SET_TIMESLICE, params, .. } => {
                Some(u64::from_le_bytes(params.as_slice().try_into().unwrap()))
            }
            _ => None,
        });
        asked.collect()
    }

    #[test]
    fn the_timeslice_can_be_changed() {
        let (mut device, space) = device();
        device.graphics_timeslice_micros = 4000;
        channel(&mut device, &space, Engine::Graphics);
        assert_eq!(timeslices_asked_for(&device), [4000]);
    }

    #[test]
    fn the_timeslice_is_the_rms_unless_someone_asks() {
        let (mut device, space) = device();
        channel(&mut device, &space, Engine::Graphics);
        assert!(timeslices_asked_for(&device).is_empty());
    }

    #[test]
    fn a_channel_can_be_given_a_slice_of_its_own_once_made() {
        let (mut device, space) = device();
        let channel = channel(&mut device, &space, Engine::Graphics);
        channel.set_timeslice(&mut device, 250);
        assert_eq!(timeslices_asked_for(&device), [250]);
    }

    #[test]
    fn a_copy_channel_keeps_the_rms_timeslice() {
        let (mut device, space) = device();
        channel(&mut device, &space, Engine::Copy);
        assert!(timeslices_asked_for(&device).is_empty());
    }

    #[test]
    fn a_copy_channel_uses_the_copy_subchannel() {
        let (mut device, space) = device();
        let channel = channel(&mut device, &space, Engine::Copy);
        assert_eq!(channel.area().load(PRELUDE), 0x2001_8000);
        assert_eq!(channel.area().load(PRELUDE + 4), BLACKWELL.copy_class);
    }

    #[test]
    fn the_doorbell_page_is_mapped_once() {
        let (mut device, space) = device();
        let first = channel(&mut device, &space, Engine::Copy);
        first.close(&mut device, &space).unwrap();
        channel(&mut device, &space, Engine::Copy);
        let maps = device.rm.calls.iter().filter(|call| matches!(call, crate::rm::fake::Call::MapDoorbell));
        assert_eq!(maps.count(), 1);
    }

    const CONTEXT: u32 = 0x4000_0040;
    const COMPUTE_SEAT: Seat = Seat::Compute { index: 1, prefer_lower: false };

    fn place_at(slot: u64) -> ChannelPlace {
        let area_address = AREA_ADDRESS + slot * AREA_SIZE as u64;
        ChannelPlace { area_address, ring_address: 0x900_0000 + slot * 0x1_0000, ..place() }
    }

    fn seated(device: &mut Device<FakeRm>, space: &AddressSpace, slot: u64, seat: Seat) -> Channel {
        Channel::new_in_context(device, &BLACKWELL, &HOST_615, space, place_at(slot), CONTEXT, seat).unwrap()
    }

    fn allocations(device: &Device<FakeRm>, of: u32) -> Vec<(Handle, Handle, Vec<u8>)> {
        let made = device.rm.calls.iter().filter_map(|call| match call {
            Call::Alloc { parent, handle, class, params } if *class == of => {
                Some((*parent, *handle, params.clone()))
            }
            _ => None,
        });
        made.collect()
    }

    fn controls(device: &Device<FakeRm>, of: u32) -> Vec<Handle> {
        let sent = device.rm.calls.iter().filter_map(|call| match call {
            Call::Control { object, command, .. } if *command == of => Some(*object),
            _ => None,
        });
        sent.collect()
    }

    fn frees(device: &Device<FakeRm>) -> Vec<Handle> {
        let freed = device.rm.calls.iter().filter_map(|call| match call {
            Call::Free { handle, .. } => Some(*handle),
            _ => None,
        });
        freed.collect()
    }

    fn word_at(params: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(params[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn a_contexts_channels_share_one_group_with_a_subcontext_for_each_seat() {
        let (mut device, space) = device();
        let drawing = seated(&mut device, &space, 0, Seat::Drawing);
        let compute = seated(&mut device, &space, 1, COMPUTE_SEAT);
        let groups = allocations(&device, CHANNEL_GROUP);
        assert_eq!(groups.len(), 1, "one group for both");
        let (_, group, params) = &groups[0];
        assert_eq!(word_at(params, GROUP_PARAM_ERROR_CONTEXT), 0, "the group outlives any one channel");
        assert_eq!(word_at(params, GROUP_PARAM_ADDRESS_SPACE), space.handle);
        assert_eq!((drawing.group, compute.group), (*group, *group));

        let shares = allocations(&device, CONTEXT_SHARE);
        let flags: Vec<u32> =
            shares.iter().map(|(_, _, params)| word_at(params, SHARE_PARAM_FLAGS)).collect();
        assert_eq!(flags, [SHARE_SYNCHRONOUS, SHARE_ASYNCHRONOUS]);
        assert!(shares.iter().all(|(parent, _, params)| {
            parent == group && word_at(params, SHARE_PARAM_ADDRESS_SPACE) == space.handle
        }));

        let channels = allocations(&device, BLACKWELL.channel_class);
        assert!(channels.iter().all(|(parent, _, _)| parent == group));
        let placed: Vec<(u32, u32)> = channels
            .iter()
            .map(|(_, _, params)| {
                (word_at(params, CHANNEL_PARAM_FLAGS), word_at(params, CHANNEL_PARAM_SHARE))
            })
            .collect();
        assert_eq!(placed, [(0, shares[0].1), (CHANNEL_FLAG_SECOND_RUNQUEUE, shares[1].1)]);
    }

    #[test]
    fn a_compute_seat_gets_the_compute_class_and_deferred_controls_on_their_usual_subchannels() {
        let (mut device, space) = device();
        seated(&mut device, &space, 0, Seat::Drawing);
        let compute = seated(&mut device, &space, 1, COMPUTE_SEAT);
        let on_channel = |class: u32| {
            allocations(&device, class).iter().filter(|(parent, _, _)| *parent == compute.channel).count()
        };
        for &(class, _) in BLACKWELL.graphics_classes {
            let expected = usize::from(class == BLACKWELL.compute_class || class == DEFERRED_API);
            assert_eq!(on_channel(class), expected, "class {class:#x}");
        }
        assert!(compute.compute_object().is_some());
        let words = compute.area().load_words(PRELUDE, compute.setup_words as usize);
        assert_eq!(words[0], increasing(COMPUTE_SUBCHANNEL, SET_OBJECT, 1));
        assert_eq!(words[1], BLACKWELL.compute_class);
        assert_eq!(words[2], increasing(5, SET_OBJECT, 1));
        assert_eq!(words.len(), 4);
        assert_eq!(compute.only_bound(), Some(1 << COMPUTE_SUBCHANNEL | 1 << 5));
        assert!(compute.in_context_group());
    }

    #[test]
    fn a_drawing_seat_binds_every_class_and_is_not_held_to_some() {
        let (mut device, space) = device();
        let drawing = seated(&mut device, &space, 0, Seat::Drawing);
        let alone =
            Channel::new(&mut device, &BLACKWELL, &HOST_615, &space, place_at(1), Engine::Graphics).unwrap();
        let (seat, own) = (drawing.setup_words as usize, alone.setup_words as usize);
        assert_eq!(drawing.area().load_words(PRELUDE, seat), alone.area().load_words(PRELUDE, own));
        assert_eq!(drawing.only_bound(), None);
        assert_eq!(alone.only_bound(), None);
        assert!(!alone.in_context_group());
    }

    #[test]
    fn the_group_is_started_once_and_a_channel_that_joins_is_started_alone() {
        let (mut device, space) = device();
        let drawing = seated(&mut device, &space, 0, Seat::Drawing);
        let compute = seated(&mut device, &space, 1, COMPUTE_SEAT);
        assert_eq!(controls(&device, CONTROL_SCHEDULE_CHANNEL_GROUP), [drawing.group]);
        assert_eq!(controls(&device, CONTROL_SCHEDULE_CHANNEL), [compute.channel]);
    }

    #[test]
    fn a_channel_the_rm_will_not_start_alone_is_started_with_its_group() {
        let (mut device, space) = device();
        let drawing = seated(&mut device, &space, 0, Seat::Drawing);
        device.rm.refuse_control = Some(CONTROL_SCHEDULE_CHANNEL);
        seated(&mut device, &space, 1, COMPUTE_SEAT);
        assert_eq!(controls(&device, CONTROL_SCHEDULE_CHANNEL_GROUP), [drawing.group, drawing.group]);
        assert_eq!(device.context_groups.members(CONTEXT), 2);
    }

    #[test]
    fn a_compute_seat_can_come_first() {
        let (mut device, space) = device();
        let compute = seated(&mut device, &space, 0, COMPUTE_SEAT);
        let drawing = seated(&mut device, &space, 1, Seat::Drawing);
        assert_eq!(controls(&device, CONTROL_SCHEDULE_CHANNEL_GROUP), [compute.group]);
        assert_eq!(controls(&device, CONTROL_SCHEDULE_CHANNEL), [drawing.channel]);
        assert_eq!(allocations(&device, CHANNEL_GROUP).len(), 1);
    }

    #[test]
    fn queues_that_name_one_subcontext_share_it() {
        let (mut device, space) = device();
        seated(&mut device, &space, 0, COMPUTE_SEAT);
        seated(&mut device, &space, 1, COMPUTE_SEAT);
        seated(&mut device, &space, 2, Seat::Compute { index: 2, prefer_lower: true });
        let shares = allocations(&device, CONTEXT_SHARE);
        let flags: Vec<u32> =
            shares.iter().map(|(_, _, params)| word_at(params, SHARE_PARAM_FLAGS)).collect();
        assert_eq!(flags, [SHARE_ASYNCHRONOUS, SHARE_ASYNCHRONOUS_PREFER_LOWER]);
    }

    #[test]
    fn another_context_gets_another_group() {
        let (mut device, space) = device();
        seated(&mut device, &space, 0, Seat::Drawing);
        let other = Channel::new_in_context(
            &mut device,
            &BLACKWELL,
            &HOST_615,
            &space,
            place_at(1),
            CONTEXT + 0x40,
            Seat::Drawing,
        );
        other.unwrap();
        assert_eq!(allocations(&device, CHANNEL_GROUP).len(), 2);
        assert_eq!(device.context_groups.len(), 2);
    }

    #[test]
    fn a_seat_the_rm_refuses_frees_what_it_made_and_leaves_the_group_as_it_was() {
        let (mut device, space) = device();
        let drawing = seated(&mut device, &space, 0, Seat::Drawing);
        let before = device.rm.calls.len();
        device.rm.refuse_class = Some(BLACKWELL.compute_class);
        let refused = Channel::new_in_context(
            &mut device,
            &BLACKWELL,
            &HOST_615,
            &space,
            place_at(1),
            CONTEXT,
            COMPUTE_SEAT,
        );
        assert!(refused.is_err());
        device.rm.refuse_class = None;
        let shares = allocations(&device, CONTEXT_SHARE);
        let channels = allocations(&device, BLACKWELL.channel_class);
        let freed: Vec<Handle> = device.rm.calls[before..]
            .iter()
            .filter_map(|call| match call {
                Call::Free { handle, .. } => Some(*handle),
                _ => None,
            })
            .collect();
        assert_eq!(freed[..2], [channels[1].1, shares[1].1], "the channel, then the subcontext it made");
        assert!(!freed.contains(&drawing.group), "the group is the drawing channel's too");
        assert_eq!(device.context_groups.members(CONTEXT), 1);
        assert_eq!(device.context_groups.share(CONTEXT, 1), None);
        let unmapped = device.rm.calls[before..].iter().any(
            |call| matches!(call, Call::UnmapDma { address, .. } if *address == place_at(1).area_address),
        );
        assert!(unmapped, "its area is unmapped, so the address can be used again");
        // The same place can be used for a channel in a group of its own.
        Channel::new(&mut device, &BLACKWELL, &HOST_615, &space, place_at(1), Engine::Graphics).unwrap();
    }

    #[test]
    fn a_first_channel_the_rm_refuses_takes_its_new_group_with_it() {
        let (mut device, space) = device();
        device.rm.refuse_class = Some(BLACKWELL.compute_class);
        let refused = Channel::new_in_context(
            &mut device,
            &BLACKWELL,
            &HOST_615,
            &space,
            place_at(0),
            CONTEXT,
            COMPUTE_SEAT,
        );
        assert!(refused.is_err());
        let group = allocations(&device, CHANNEL_GROUP)[0].1;
        assert!(frees(&device).contains(&group));
        assert!(device.context_groups.is_empty());
    }

    #[test]
    fn a_compute_seat_on_the_first_subcontext_is_refused_before_anything_is_made() {
        let (mut device, space) = device();
        let calls = device.rm.calls.len();
        let seat = Seat::Compute { index: 0, prefer_lower: false };
        let refused =
            Channel::new_in_context(&mut device, &BLACKWELL, &HOST_615, &space, place_at(0), CONTEXT, seat);
        assert!(refused.is_err());
        assert_eq!(device.rm.calls.len(), calls);
    }

    #[test]
    fn a_channel_that_leaves_takes_its_subcontext_and_the_last_one_the_group() {
        let (mut device, space) = device();
        let drawing = seated(&mut device, &space, 0, Seat::Drawing);
        let first = seated(&mut device, &space, 1, COMPUTE_SEAT);
        let second = seated(&mut device, &space, 2, COMPUTE_SEAT);
        let (group, shares) = (drawing.group, allocations(&device, CONTEXT_SHARE));
        let (drawing_channel, first_channel, second_channel) =
            (drawing.channel, first.channel, second.channel);

        let before = frees(&device).len();
        first.close(&mut device, &space).unwrap();
        assert_eq!(frees(&device)[before..][0], first_channel);
        assert!(!frees(&device).contains(&shares[1].1), "the other compute channel is still on it");

        // The drawing channel can go before the rest, and the group stays for them.
        let before = frees(&device).len();
        drawing.close(&mut device, &space).unwrap();
        assert_eq!(frees(&device)[before..][..2], [drawing_channel, shares[0].1]);
        assert!(!frees(&device).contains(&group));

        let before = frees(&device).len();
        second.close(&mut device, &space).unwrap();
        assert_eq!(frees(&device)[before..][..3], [second_channel, shares[1].1, group]);
        assert!(device.context_groups.is_empty());
    }

    #[test]
    fn a_group_of_its_own_still_goes_whole() {
        let (mut device, space) = device();
        let alone = channel(&mut device, &space, Engine::Graphics);
        let group = alone.group;
        let before = frees(&device).len();
        alone.close(&mut device, &space).unwrap();
        assert_eq!(frees(&device)[before], group);
    }

    #[test]
    fn a_seated_channel_gets_no_slice_unless_every_graphics_channel_does() {
        let (mut device, space) = device();
        seated(&mut device, &space, 0, Seat::Drawing);
        assert!(timeslices_asked_for(&device).is_empty());
        let (mut device, space) = self::device();
        device.graphics_timeslice_micros = 4000;
        seated(&mut device, &space, 0, Seat::Drawing);
        seated(&mut device, &space, 1, COMPUTE_SEAT);
        assert_eq!(timeslices_asked_for(&device), [4000], "once, for the group");
    }

    #[test]
    fn advancing_stores_the_position_then_rings() {
        let (mut device, space) = device();
        let channel = channel(&mut device, &space, Engine::Graphics);
        channel.advance(&mut device, 1024 + 5);
        assert_eq!(channel.area().load(CONTROL_BLOCK + GP_PUT), 5);
        assert_eq!(device.rm.doorbells(), [channel.token()]);
    }
}
