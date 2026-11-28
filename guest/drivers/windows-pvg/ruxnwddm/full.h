// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The state and the functions of the full (render and display) driver,
// milestone 2: allocations, devices and contexts, the fence of the one
// engine, flips and the stand-in vertical blank, and the map of GPU virtual
// addresses that monitored fences need. Included by ruxnwddm.h, before the
// adapter, which holds the adapter-wide parts.

#ifndef RUXNWDDM_FULL_H
#define RUXNWDDM_FULL_H

#include "ruxnwddm_umd.h"

typedef struct _RUXNWDDM_ADAPTER RUXNWDDM_ADAPTER;
typedef struct _RUXNWDDM_DEVICE RUXNWDDM_DEVICE;
struct _RUXNWDDM_CLIENT;
struct pvg_surface;

// An allocation as created, or as one device opened it.
typedef struct _RUXNWDDM_ALLOCATION {
	struct ruxnwddm_alloc_info info;
	RUXNWDDM_DEVICE *opened_by;	// NULL for the created one
	// The guest page behind each 4 KiB page, for primaries only, as the
	// last page-table update named them (paging.c).
	ULONG64 *pages;
	ULONG page_count;
	// The device surface it lies over, held by the display's client, for
	// allocations made with RUXNWDDM_ALLOC_SURFACE; NULL otherwise.
	struct pvg_surface *surface;
	ULONG maker_process;		// who made it (or opened it), for the escapes
	LIST_ENTRY link;		// in the adapter's list of live ones (alloc_list.c)
} RUXNWDDM_ALLOCATION;

struct _RUXNWDDM_DEVICE {
	RUXNWDDM_ADAPTER *adapter;
	HANDLE runtime_handle;
	// Its task's client, made at its first task escape (pvg_client.c).
	struct _RUXNWDDM_CLIENT *volatile client;
};

typedef struct _RUXNWDDM_CONTEXT {
	RUXNWDDM_DEVICE *device;
	UINT node;
	BOOLEAN system;
} RUXNWDDM_CONTEXT;

// The fence of the one engine. Read and written under the graphics
// kernel's interrupt lock (DxgkCbSynchronizeExecution).
typedef struct _RUXNWDDM_FENCE {
	volatile UINT pending;		// the newest fence submitted
	UINT completed;			// the newest reported done
	UINT preempt;
	volatile LONG preempt_pending;
	KDPC dpc;			// reports completion after the submit returns
} RUXNWDDM_FENCE;

#define RUXNWDDM_VSYNC_GAP_BUCKETS	4
#define RUXNWDDM_SWAP_TIME_BUCKETS	5

// The flip state and the timer that stands in for the vertical blank.
typedef struct _RUXNWDDM_FLIP {
	BOOLEAN running;
	volatile BOOLEAN stopping;
	volatile BOOLEAN vsync_enabled;
	PHYSICAL_ADDRESS address;	// as SetVidPnSourceAddress gave it
	// Written at the interrupt's level, so with interlocked stores only.
	RUXNWDDM_ALLOCATION *volatile shown;
	RUXNWDDM_ALLOCATION *volatile copying;
	// The allocation whose surface the display was last told to show.
	RUXNWDDM_ALLOCATION *swapped;
	volatile BOOLEAN shown_has_surface;	// the shown allocation lies over a surface
	ULONG64 shown_count;		// flips turned into a swap of a surface
	ULONG64 copied_count;		// flips shown by the stepping-stone copy
	ULONG64 failures;		// swaps that failed; showing stops after one
	ULONG64 failures_before_power_up;	// of devices before the last power-up
	volatile BOOLEAN powered;	// the adapter is in D0
	ULONG shown_pid;
	BOOLEAN efb_marked;
	BOOLEAN copy_allowed;		// the stepping stone, only with FlipCopy 1
	KTIMER timer;
	KDPC dpc;
	PEX_TIMER high_res_timer;	// one-shot, re-armed against next_due (flip.c)
	LONGLONG next_due;		// the next vertical blank, in interrupt time (100 ns)
	ULONG64 vsyncs_late;		// callbacks a period or more late
	ULONG64 vsyncs_resynced;	// times the schedule was moved on
	KEVENT copy_wake;
	// Timing, all from KeQueryPerformanceCounter, for the frame rate's
	// questions (run rn16: 60 frames presented, about 47 swaps shown).
	LONGLONG first_vsync, last_vsync, first_swap, last_swap, qpc_frequency;
	ULONG64 vsync_gaps[RUXNWDDM_VSYNC_GAP_BUCKETS];	// <12, <20, <28, more ms
	ULONG64 swap_times[RUXNWDDM_SWAP_TIME_BUCKETS];	// <2, <4, <8, <16, more ms
	ULONG64 swap_most_us;
	ULONG64 superseded;		// flips replaced before the worker showed them
	ULONG64 flips_seen;		// a->flip.flips when the worker last looked
	PKTHREAD thread;
	ULONG64 vsyncs;
	ULONG64 flips;
} RUXNWDDM_FLIP;

// GPU virtual page to guest page, for the few pages that hold fences.
#define RUXNWDDM_VA_MAP_SIZE	256

typedef struct _RUXNWDDM_VA_MAP {
	KSPIN_LOCK lock;
	ULONG next;
	struct {
		HANDLE process;
		ULONG64 va;
		ULONG64 phys;
	} entry[RUXNWDDM_VA_MAP_SIZE];
} RUXNWDDM_VA_MAP;

// What the full driver has done, for the INFO escape.
typedef struct _RUXNWDDM_FULL_COUNTS {
	volatile LONG64 devices;
	volatile LONG64 contexts;
	volatile LONG64 processes;
	volatile LONG64 allocations;
	volatile LONG64 allocations_freed;
	volatile LONG64 primaries;
	volatile LONG64 submits;
	volatile LONG64 paging_submits;
	volatile LONG64 paging_ops;
	volatile LONG64 paging_skipped;
	volatile LONG64 fences_written;
	volatile LONG64 fences_lost;
	volatile LONG64 presents;
	volatile LONG64 resources;
	volatile LONG64 present_flips;
	volatile LONG64 present_blts;
	volatile LONG64 present_to_primary;
	volatile LONG64 surface_allocations;
	volatile LONG64 surface_imports_failed;
	// The user-mode driver's notes, by event.
	volatile LONG umd_count[RUXNWDDM_UMD_NOTES];
	ULONG umd_value[RUXNWDDM_UMD_NOTES];
	ULONG umd_process[RUXNWDDM_UMD_NOTES];
} RUXNWDDM_FULL_COUNTS;

// Where blt presents go (blt_present.c): for each program's presented
// surface, the surface of the compositor's window it was blt to.
#define RUXNWDDM_BLT_DEST_COUNT	16

typedef struct _RUXNWDDM_BLT_DEST {
	ULONG process;
	ULONG64 src_token, dst_token;
	UINT width, height, format;
	ULONG64 presents;
} RUXNWDDM_BLT_DEST;

// Every live allocation, made or opened, for the ALLOCATIONS escape: which
// ones outlive the programs that made them (run rn64).
typedef struct _RUXNWDDM_ALLOC_LIST {
	KSPIN_LOCK lock;
	LIST_ENTRY live;
	ULONG count;
} RUXNWDDM_ALLOC_LIST;

typedef struct _RUXNWDDM_BLT_DESTS {
	KSPIN_LOCK lock;
	RUXNWDDM_BLT_DEST dest[RUXNWDDM_BLT_DEST_COUNT];
	UINT next;			// the entry taken next when none matches
	volatile LONG noted;		// blts noted, for the first few records
	volatile LONG seen;		// blt presents seen, likewise
} RUXNWDDM_BLT_DESTS;

// caps.c
NTSTATUS ruxnwddm_query_adapter_info_full(RUXNWDDM_ADAPTER *a, const DXGKARG_QUERYADAPTERINFO *args);

// alloc_list.c
void ruxnwddm_alloc_list_init(RUXNWDDM_ADAPTER *a);
ULONG pvg_surface_holders(const struct pvg_surface *surface);	// pvg_surface.c
void ruxnwddm_alloc_listed(RUXNWDDM_ADAPTER *a, RUXNWDDM_ALLOCATION *al);
void ruxnwddm_alloc_unlisted(RUXNWDDM_ADAPTER *a, RUXNWDDM_ALLOCATION *al);
struct ruxnwddm_escape_allocations;
void ruxnwddm_alloc_list_fill(RUXNWDDM_ADAPTER *a, struct ruxnwddm_escape_allocations *out);

// allocation.c
NTSTATUS ruxnwddm_create_allocation(RUXNWDDM_ADAPTER *a, DXGKARG_CREATEALLOCATION *args);
NTSTATUS ruxnwddm_destroy_allocation(RUXNWDDM_ADAPTER *a, const DXGKARG_DESTROYALLOCATION *args);
NTSTATUS ruxnwddm_describe_allocation(RUXNWDDM_ADAPTER *a, DXGKARG_DESCRIBEALLOCATION *args);
NTSTATUS ruxnwddm_standard_allocation(RUXNWDDM_ADAPTER *a,
				      DXGKARG_GETSTANDARDALLOCATIONDRIVERDATA *args);
NTSTATUS ruxnwddm_open_allocation(RUXNWDDM_DEVICE *dev, const DXGKARG_OPENALLOCATION *args);
NTSTATUS ruxnwddm_close_allocation(RUXNWDDM_ADAPTER *a, const DXGKARG_CLOSEALLOCATION *args);

// paging.c
NTSTATUS ruxnwddm_build_paging_buffer(RUXNWDDM_ADAPTER *a, DXGKARG_BUILDPAGINGBUFFER *args);
SIZE_T ruxnwddm_root_page_table_size(DXGKARG_GETROOTPAGETABLESIZE *args);
void ruxnwddm_free_pages_list(RUXNWDDM_ALLOCATION *al);

// render.c
NTSTATUS ruxnwddm_create_device(RUXNWDDM_ADAPTER *a, DXGKARG_CREATEDEVICE *args);
NTSTATUS ruxnwddm_destroy_device(RUXNWDDM_DEVICE *dev);
NTSTATUS ruxnwddm_create_context(RUXNWDDM_DEVICE *dev, DXGKARG_CREATECONTEXT *args);
NTSTATUS ruxnwddm_destroy_context(RUXNWDDM_CONTEXT *ctx);
NTSTATUS ruxnwddm_create_process(RUXNWDDM_ADAPTER *a, DXGKARG_CREATEPROCESS *args);
void ruxnwddm_fence_init(RUXNWDDM_ADAPTER *a);
NTSTATUS ruxnwddm_submit_virtual(RUXNWDDM_ADAPTER *a, const DXGKARG_SUBMITCOMMANDVIRTUAL *args);
NTSTATUS ruxnwddm_submit(RUXNWDDM_ADAPTER *a, const DXGKARG_SUBMITCOMMAND *args);
NTSTATUS ruxnwddm_preempt(RUXNWDDM_ADAPTER *a, const DXGKARG_PREEMPTCOMMAND *args);
NTSTATUS ruxnwddm_query_current_fence(RUXNWDDM_ADAPTER *a, DXGKARG_QUERYCURRENTFENCE *args);
NTSTATUS ruxnwddm_query_engine_status(RUXNWDDM_ADAPTER *a, DXGKARG_QUERYENGINESTATUS *args);
NTSTATUS ruxnwddm_reset_engine(RUXNWDDM_ADAPTER *a, DXGKARG_RESETENGINE *args);
NTSTATUS ruxnwddm_node_metadata(UINT node, DXGKARG_GETNODEMETADATA *args);
NTSTATUS ruxnwddm_calibrate_clock(DXGKARG_CALIBRATEGPUCLOCK *args);

// flip.c
NTSTATUS ruxnwddm_flip_start(RUXNWDDM_ADAPTER *a);
void ruxnwddm_flip_power(RUXNWDDM_ADAPTER *a, BOOLEAN on);
void ruxnwddm_flip_stop(RUXNWDDM_ADAPTER *a);
NTSTATUS ruxnwddm_set_source_address(RUXNWDDM_ADAPTER *a, const DXGKARG_SETVIDPNSOURCEADDRESS *args);
void ruxnwddm_flip_forget(RUXNWDDM_ADAPTER *a, RUXNWDDM_ALLOCATION *al);
NTSTATUS ruxnwddm_control_interrupt(RUXNWDDM_ADAPTER *a, DXGK_INTERRUPT_TYPE type, BOOLEAN enable);
NTSTATUS ruxnwddm_get_scan_line(RUXNWDDM_ADAPTER *a, DXGKARG_GETSCANLINE *args);

// driver_full.c
void ruxnwddm_fill_full(DRIVER_INITIALIZATION_DATA *init);

#endif
