// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The device's tasks, buffer objects, surfaces, mapper and display, as
// ruxnpvg has them (task.h and screen.h there), moved under the graphics
// kernel. Only the pvg_*.c files look inside these.
//
// In ruxnpvg each open handle of \\.\ruxnpvg had a task. Here a client
// stands where the handle stood: each kernel device a program makes on our
// adapter (D3DKMTCreateDevice, or the Direct3D runtime's own) gets one when
// it first sends a task escape, and the device's end is the handle's end.
// One more client belongs to the driver itself and has no process: the
// display's, which holds the surfaces that allocations are made over and
// shows them when Windows flips (flip.c).
//
// The code is ruxnpvg's with KMDF taken out: its wait locks became the
// driver's event locks, its requests became escapes (pvg_client.c).

#ifndef RUXNWDDM_PVG_H
#define RUXNWDDM_PVG_H

#include "ruxnwddm.h"
#include "ruxnpvg_ioctl.h"

// The early framebuffer's switch; the bank's other registers are in
// ruxnwddm.h. The bank reads zeroes while it is off.
#define REG_EFB_ON			0x1200

// BAR 2 holds the mapper's 64-bit registers; the BAR is taken if it is
// longer than the last of them.
#define RUXNPVG_MAPPER_REG_LAST		0x1028

// The protocol version the ring runs (ring.c asks for it).
#define RUXNPVG_VERSION			252

// Commands of the first ring that are about a task (pvg_task.c) and about
// the display (pvg_display.c).
#define RUXNPVG_CMD_TASK_DELETE		32
#define RUXNPVG_CMD_TASK_UNMAP		34	// a part of a task's memory, as Linux's pvg_placement_put
#define RUXNPVG_CMD_TASK_DELETE_RESOURCE	37
#define RUXNPVG_CMD_TASK_DELETE_OBJECT	40
#define RUXNPVG_CMD_TASK_SYNC		53
#define RUXNPVG_CMD_TASK_DEFINE		56
#define RUXNPVG_CMD_TASK_EXEC		67
#define RUXNPVG_CMD_DISPLAY_STATE_PAGE	1
#define RUXNPVG_CMD_DISPLAY_ACK		2
#define RUXNPVG_CMD_DISPLAY_SWAP	8

// The page table has two levels. A page of the table holds 4096 entries, so
// the top page names tables of 64 MiB each. An entry is a page's number, and
// bit 31 says the host may write the page.
#define TASK_DEPTH		2
#define TASK_TABLE_SHIFT	12
#define TASK_TABLE_ENTRIES	(1U << TASK_TABLE_SHIFT)
#define TASK_ENTRY_WRITABLE	0x80000000U
// A task is 1 GiB (16 tables of 64 MiB), the guard's most_user_leaves 16.
// Parameters\TaskMiB (a multiple of 64 from 1024 to 4096) asks for more,
// only where the host's guard allows a task more than 16 tables.
#define TASK_BYTES		(1ULL << 30)
#define TASK_MOST_BYTES		(4ULL << 30)
#define TASK_TABLES		((ULONG)(TASK_MOST_BYTES >> RUXNWDDM_PAGE_SHIFT) >> TASK_TABLE_SHIFT)
ULONG64 pvg_task_bytes(void);
#define TASK_FREE_PLACES	256	// places given back that a task keeps

// Object numbers run from 1 to this less one. The device took lists of
// 4,096 and 16,384 entries each over several pages (harness scenes
// object-3000 and object-16000); a list of one count with the other at one
// page was refused.
#define TASK_MAX_OBJECTS	16384
// The object list has 12 bytes an object number and the placement list 16,
// each over whole pages of its own, page by page (not one block).
#define TASK_OBJECT_LIST_PAGES	\
	((TASK_MAX_OBJECTS * 12 + RUXNWDDM_PAGE_SIZE - 1) / RUXNWDDM_PAGE_SIZE)
#define TASK_PLACEMENT_LIST_PAGES	\
	((TASK_MAX_OBJECTS * 16 + RUXNWDDM_PAGE_SIZE - 1) / RUXNWDDM_PAGE_SIZE)
// The driver's part of a task: the object list, the placement list and 128
// pages of descriptions (entered as they are needed), then the buffer
// objects from 4 MiB on. 16 pages ran out at 2,064 objects in run rn82
// (a texture's description takes two 64-byte pieces); 128 give every
// object number up to 128 bytes.
#define TASK_DESCRIPTION_PAGES	128
#define TASK_FIRST_BO_PAGE	256
// Descriptions are given room in pieces of this size.
#define TASK_DESCRIPTION_SLOT	64
#define TASK_DESCRIPTION_SLOTS	\
	(TASK_DESCRIPTION_PAGES * RUXNWDDM_PAGE_SIZE / TASK_DESCRIPTION_SLOT)

// Mapper commands, and the outcome for a mapping a display still shows.
#define MAPPER_CMD_MAP		1
#define MAPPER_CMD_UNMAP	2
#define MAPPER_OUTCOME_BUSY	0xe00002d5U

// Mapping ids run from 1 to 8191. A page of descriptors serves 32 ids,
// 0x200 bytes each.
#define MAPPER_IDS		8192
#define MAPPER_IDS_PER_PAGE	32
#define MAPPER_DESCRIPTOR	0x200
#define MAPPER_DESCRIPTOR_PAGES	(MAPPER_IDS / MAPPER_IDS_PER_PAGE)

typedef struct _RUXNWDDM_PVG RUXNWDDM_PVG;
typedef struct _RUXNWDDM_CLIENT RUXNWDDM_CLIENT;

// A buffer object: device pages, each 16 KiB and aligned, lying anywhere,
// mapped into the process that made it. It lies in its client's task from
// its making to the client's end.
struct pvg_bo {
	ULONG64 bytes;		// a whole number of device pages
	PMDL mdl;		// its memory, from MmAllocatePagesForMdlEx
	PVOID user;		// where it is mapped in the maker, or NULL
	ULONG first_page;	// where it lies in the task, in device pages
	ULONG objects;		// objects of the task that name it
};

struct pvg_surface;

struct pvg_object {
	UCHAR kind;		// 0 while the number is free
	BOOLEAN retired;	// destroyed by its program, kept by the host
	USHORT delete_op;	// what deletes it, if it is no resource
	USHORT slots;		// the description's room
	ULONG slot;
	struct pvg_bo *bo;	// the memory it names, if it names some
	struct pvg_surface *surface;	// the surface a kind 11 names
	ULONG64 bytes;		// a buffer's length, for textures over it (pvg_view.c)
	USHORT levels;		// a texture's levels and pixel format, for views of it
	USHORT format;
	UCHAR type;		// its MTLTextureType, and its slices (six faces to a cube)
	ULONG slices;
};

struct pvg_task {
	ULONG id;
	BOOLEAN defined;	// the device knows the task
	BOOLEAN lost;		// the device would not let it go: its pages stay

	RUXNWDDM_PAGES root;
	RUXNWDDM_PAGES top;
	RUXNWDDM_PAGES tables[TASK_TABLES];
	RUXNWDDM_PAGES object_pages[TASK_OBJECT_LIST_PAGES];
	RUXNWDDM_PAGES placement_pages[TASK_PLACEMENT_LIST_PAGES];
	RUXNWDDM_PAGES descriptions[TASK_DESCRIPTION_PAGES];
	ULONG description_slots[TASK_DESCRIPTION_SLOTS / 32];

	// The next device page no buffer object has had, and the places given
	// back by buffer objects the device unmapped (task_unplace), by first
	// page, none touching, handed out again first fit.
	ULONG next_page;
	struct pvg_place { ULONG first, pages; } free_places[TASK_FREE_PLACES];
	ULONG free_place_count;
	struct pvg_object objects[TASK_MAX_OBJECTS];
};

// A client of the tasks: a kernel device of a program, or the display's
// own (process NULL). Lock order: the device's clients_lock, a client's
// lock, the display's, the mapper's, the ring's.
struct _RUXNWDDM_CLIENT {
	RUXNWDDM_PVG *pvg;
	LIST_ENTRY link;	// among the device's clients, under clients_lock
	BOOLEAN listed;
	// Held across a whole escape and across the task's teardown, so it is
	// also the task's lock.
	RUXNWDDM_LOCK lock;
	PEPROCESS process;	// the maker, referenced; NULL for the display's
	BOOLEAN dead;		// the device was powered down under it
	struct pvg_task *task;
	struct pvg_bo *bos[RUXNPVG_MAX_BOS];	// by handle less one
	ULONG bo_count;
	// Surfaces by handle less one; a destroyed one leaves a hole.
	struct pvg_surface *surfaces[RUXNPVG_MAX_SURFACES];
	ULONG surface_count;
};

struct pvg_mapper {
	// One command at a time, the ids, the lists.
	RUXNWDDM_LOCK lock;
	// The device has been told of this start's ring and table.
	BOOLEAN started;
	// A command was not done in time: no more are sent this start.
	BOOLEAN broken;
	RUXNWDDM_PAGES ring_page;
	RUXNWDDM_PAGES table_page;
	RUXNWDDM_PAGES descriptor_pages[MAPPER_DESCRIPTOR_PAGES];
	// How many commands the guest has written. A counter that runs on;
	// the slot is the counter modulo the ring's length.
	ULONG64 written;
	// The DPC sets it when message 1 came.
	KEVENT wake;
	ULONG ids[MAPPER_IDS / 32];
	LIST_ENTRY mapped;	// surfaces that have a mapping id
	// Surfaces given up while a display showed them, which keeps them
	// mapped. The mapper owns these.
	LIST_ENTRY held;
	// Surfaces whose maker has handed out a token for them. The lock
	// above also guards the tokens and every surface's holders.
	LIST_ENTRY exported;
};

// A surface: one block of device pages, contiguous and aligned to 16 KiB,
// mapped into the process that made it, with a page that lists its device
// pages one by one for the mapper. Shared by token as in ruxnpvg's
// surface.c; the display's client imports the ones Windows scans out.
struct pvg_surface {
	RUXNWDDM_PAGES memory;
	RUXNWDDM_PAGES list;
	PMDL mdl;
	PVOID user;		// where it is mapped in the maker, or NULL
	RUXNWDDM_CLIENT *maker;	// NULL once the maker let it go
	ULONG holders;		// clients that hold it, the maker among them
	ULONG64 token;		// 0 while not exported
	LIST_ENTRY exported;	// on the mapper's list while it has a token
	ULONG width;
	ULONG height;
	ULONG pitch;
	ULONG64 bytes;		// a whole number of device pages
	ULONG id;		// the mapping id, 0 while not mapped
	LIST_ENTRY link;	// on the mapper's mapped or held list while mapped
	volatile LONG objects;	// kind 11 objects, of any task, that name it
	// The device may still read it: its memory is never given back.
	BOOLEAN kept;
};

// Who owns the screen, and how.
#define SCREEN_FREE		0
#define SCREEN_EFB_OFF		1	// the early framebuffer off, no display
#define SCREEN_DISPLAY		2	// the display taken, with task 0

struct pvg_display {
	// Taking, showing and letting go, one at a time.
	RUXNWDDM_LOCK lock;
	RUXNWDDM_CLIENT *owner;
	ULONG mode;		// SCREEN_*
	ULONG64 owner_process;
	// A display command failed. The device may hold the display spoiled,
	// so the driver takes it no more until it is loaded again.
	BOOLEAN spoiled;
	// The state page may be read by the message 0 DPC.
	volatile LONG live;
	volatile LONG events_pending;	// the handler saw 0x1014 non-zero
	volatile LONG modes_waiting;	// a mode list waits to be acknowledged

	RUXNWDDM_PAGES task_root;
	RUXNWDDM_PAGES task_table;
	RUXNWDDM_PAGES glyph;
	RUXNWDDM_PAGES state_page;
	ULONG sequence;
	ULONG first_mode;
	ULONG shown_id;

	// The early framebuffer's bank as read before turning it off, put back
	// as it was when the screen is given back.
	BOOLEAN bank_off;
	ULONG bank[4];		// page, bytes, depth, mode
};

// What the tasks have done, for the PVG_INFO escape. Bumped with
// Interlocked calls.
typedef struct _RUXNWDDM_TASK_COUNTS {
	volatile LONG64 defined;	// tasks the device was told of
	volatile LONG64 deleted;	// and told to forget
	volatile LONG64 lost;		// that the device would not let go
	volatile LONG64 submits;
	volatile LONG64 refused;	// submits the device refused
	volatile LONG64 bos;		// buffer objects made
	volatile LONG64 bos_freed;
	volatile LONG64 bos_kept;	// left allocated, see pvg_bo.c
	volatile LONG64 bos_unplaced;	// given back by their makers, unmapped by the device
	volatile LONG64 bos_not_unplaced;	// given back, but the device did not say it unmapped them
	volatile LONG64 unmapped;	// mappings undone in their own process
	volatile LONG64 bo_bytes;	// bytes of buffer objects allocated now
} RUXNWDDM_TASK_COUNTS;

// What the mapper, the surfaces and the display have done, as ruxnpvg's
// DISPLAY_INFO has it.
typedef struct _RUXNWDDM_SCREEN_COUNTS {
	volatile LONG64 mapper_started;
	volatile LONG64 mapper_commands;
	volatile LONG64 mapper_irqs;
	volatile LONG64 mapper_early;
	volatile LONG64 mapper_late;
	volatile LONG64 mapper_timeouts;
	volatile LONG64 mapper_power_downs;	// started mappers forgotten at a power-down
	volatile LONG64 maps;
	volatile LONG64 unmaps;
	volatile LONG64 outcomes_busy;
	volatile LONG64 outcomes_other;
	volatile LONG64 ids_passed;
	volatile LONG64 mapped_now;
	volatile LONG64 held_now;
	volatile LONG64 surfaces;
	volatile LONG64 surfaces_freed;
	volatile LONG64 surfaces_kept;
	volatile LONG64 surface_bytes;
	volatile LONG64 takes;
	volatile LONG64 releases;
	volatile LONG64 task0_defined;
	volatile LONG64 task0_deleted;
	volatile LONG64 hand_overs;
	volatile LONG64 let_gos;
	volatile LONG64 mode_lists;
	volatile LONG64 acknowledgements;
	volatile LONG64 swaps;
	volatile LONG64 swap_done;
	volatile LONG64 events_taken;
	volatile LONG64 display_failed;
	volatile LONG64 bank_reads;
	volatile LONG64 bank_offs;
	volatile LONG64 bank_restores;
	volatile LONG64 exports;
	volatile LONG64 imports;
	volatile LONG64 imports_refused;
	volatile LONG64 import_releases;
	volatile LONG64 exported_now;
	volatile LONG64 holders_now;
	volatile LONG64 outlived_maker;
	volatile LONG64 display_events;	// events handed to the display while it was live
} RUXNWDDM_SCREEN_COUNTS;

// The device core: what ruxnpvg's device context held for these parts.
struct _RUXNWDDM_PVG {
	RUXNWDDM_ADAPTER *adapter;
	// BAR 2, the mapper's, mapped at start; NULL if the device has none.
	volatile UCHAR *mapper_regs;

	// The clients, each with its task once it has needed one.
	RUXNWDDM_LOCK clients_lock;
	LIST_ENTRY clients;
	// Tasks may be made: the adapter is started and powered. Set at start
	// and power-up, cleared at power-down before the tasks are deleted.
	volatile LONG task_gate;
	volatile LONG64 power_downs, power_ups;	// D3 and back, as the graphics kernel asks
	KSPIN_LOCK ids_lock;		// the bitmap below, nothing else
	ULONG task_ids[256 / 32];	// ids in use, 1 to 255
	RUXNWDDM_TASK_COUNTS tasks;

	struct pvg_mapper mapper_state;
	struct pvg_display display_state;
	// Pointers to the two above, as the ported code names them.
	struct pvg_mapper *mapper;
	struct pvg_display *display;
	RUXNWDDM_SCREEN_COUNTS screen;
	volatile LONG mapper_irq_pending;	// message 1 came since the last DPC
	// Submits by process (pvg_object.c), for the PVG_INFO escape.
	KSPIN_LOCK submit_lock;
	ULONG submit_pid[RUXNWDDM_SUBMIT_PROCESSES];
	ULONG submit_done[RUXNWDDM_SUBMIT_PROCESSES];
	ULONG submit_refused[RUXNWDDM_SUBMIT_PROCESSES];
	char submit_image[RUXNWDDM_SUBMIT_PROCESSES][RUXNWDDM_IMAGE_NAME];

	// The display's own client (flip.c), made at start.
	RUXNWDDM_CLIENT *display_client;
};

static __forceinline ULONG64 pvg_mapper_read(RUXNWDDM_PVG *pvg, ULONG reg)
{
	return READ_REGISTER_ULONG64((volatile ULONG64 *)(pvg->mapper_regs + reg));
}

static __forceinline void pvg_mapper_write(RUXNWDDM_PVG *pvg, ULONG reg, ULONG64 value)
{
	WRITE_REGISTER_ULONG64((volatile ULONG64 *)(pvg->mapper_regs + reg), value);
}

// pvg_client.c: clients, the task escapes, power.
void pvg_init(RUXNWDDM_PVG *pvg, RUXNWDDM_ADAPTER *a);
NTSTATUS pvg_start(RUXNWDDM_PVG *pvg);
void pvg_power_down(RUXNWDDM_PVG *pvg);
void pvg_stop(RUXNWDDM_PVG *pvg);
void pvg_power_up(RUXNWDDM_PVG *pvg);
RUXNWDDM_CLIENT *pvg_client_create(RUXNWDDM_PVG *pvg, BOOLEAN own_process);
void pvg_client_close(RUXNWDDM_CLIENT *client);
NTSTATUS pvg_escape(RUXNWDDM_PVG *pvg, RUXNWDDM_CLIENT **client_slot, void *data, UINT size);
void pvg_interrupt(RUXNWDDM_PVG *pvg, ULONG message, ULONG events);
void pvg_dpc(RUXNWDDM_PVG *pvg);

// pvg_task.c and pvg_object.c: one task per client.
NTSTATUS pvg_task_dispatch(RUXNWDDM_CLIENT *client, ULONG code, UCHAR *in, SIZE_T in_length,
			   PVOID out, SIZE_T out_length, ULONG_PTR *returned);
void pvg_client_end_task(RUXNWDDM_CLIENT *client);
ULONG64 pvg_open_clients(RUXNWDDM_PVG *pvg);
void pvg_task_info(RUXNWDDM_PVG *pvg, struct ruxnpvg_task_info *info);

// pvg_bo.c
NTSTATUS pvg_bo_alloc(RUXNWDDM_PVG *pvg, ULONG64 bytes, struct pvg_bo **out);
NTSTATUS pvg_bo_map(struct pvg_bo *bo);
ULONG pvg_bo_page(const struct pvg_bo *bo, ULONG page);
void pvg_bo_release(RUXNWDDM_CLIENT *client, struct pvg_bo *bo, BOOLEAN keep_memory);

// pvg_mapper.c
void pvg_mapper_init(RUXNWDDM_PVG *pvg);
NTSTATUS pvg_mapper_start(RUXNWDDM_PVG *pvg);
NTSTATUS pvg_mapper_command(RUXNWDDM_PVG *pvg, ULONG command, ULONG id, ULONG *outcome);
volatile ULONG *pvg_mapper_descriptor(RUXNWDDM_PVG *pvg, ULONG id);
void pvg_mapper_irq(RUXNWDDM_PVG *pvg);
void pvg_mapper_release(RUXNWDDM_PVG *pvg);
void pvg_mapper_power_down(RUXNWDDM_PVG *pvg);

// pvg_surface.c
NTSTATUS pvg_surface_create(RUXNWDDM_CLIENT *client, ULONG width, ULONG height,
			    struct pvg_surface **out);
NTSTATUS pvg_surface_export(RUXNWDDM_CLIENT *client, struct pvg_surface *surface,
			    ULONG64 *token);
NTSTATUS pvg_surface_import(RUXNWDDM_CLIENT *client, ULONG64 token, struct pvg_surface **out);
NTSTATUS pvg_surface_map_user(struct pvg_surface *surface);
NTSTATUS pvg_surface_map(RUXNWDDM_PVG *pvg, struct pvg_surface *surface);
void pvg_surface_release(RUXNWDDM_CLIENT *client, struct pvg_surface *surface);
void pvg_surface_reap(RUXNWDDM_PVG *pvg);
void pvg_surfaces_unmap_all(RUXNWDDM_PVG *pvg);
void pvg_surface_forget(RUXNWDDM_PVG *pvg, struct pvg_surface *surface);

// pvg_display.c
void pvg_display_init(RUXNWDDM_PVG *pvg);
NTSTATUS pvg_efb_read(RUXNWDDM_PVG *pvg, struct ruxnpvg_efb *out);
NTSTATUS pvg_efb_off(RUXNWDDM_CLIENT *client, struct ruxnpvg_efb *out);
NTSTATUS pvg_display_take(RUXNWDDM_CLIENT *client);
NTSTATUS pvg_display_swap(RUXNWDDM_CLIENT *client, struct pvg_surface *surface,
			  ULONG *swap_done);
NTSTATUS pvg_display_release(RUXNWDDM_CLIENT *client);
void pvg_display_file_closing(RUXNWDDM_CLIENT *client);
void pvg_display_power_down(RUXNWDDM_PVG *pvg);
void pvg_display_irq(RUXNWDDM_PVG *pvg, ULONG events);
void pvg_display_dpc(RUXNWDDM_PVG *pvg);
void pvg_display_info(RUXNWDDM_PVG *pvg, struct ruxnpvg_display_info *info);

#endif
