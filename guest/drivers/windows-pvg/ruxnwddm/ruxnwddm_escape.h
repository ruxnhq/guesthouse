// SPDX-License-Identifier: MIT
//
// The private data of ruxnwddm's escapes, shared by the driver and
// wddmtool. A program reaches the driver with D3DKMTEscape on the adapter;
// the private data starts with this header and the driver answers in the
// same buffer. Nothing here is a stable interface yet.

#ifndef RUXNWDDM_ESCAPE_H
#define RUXNWDDM_ESCAPE_H

// The task controls' structures. In user mode their codes need winioctl.h.
#ifndef _KERNEL_MODE
#include <winioctl.h>
#endif
#include "ruxnpvg_ioctl.h"

// "RXWD", so that a stray escape from another program is refused.
#define RUXNWDDM_ESCAPE_MAGIC		0x44575852u
#define RUXNWDDM_ESCAPE_VERSION		1

// What the escape asks.
#define RUXNWDDM_ESCAPE_INFO		1	// the counters below, no device access
#define RUXNWDDM_ESCAPE_RING_START	2	// turn the first ring on (not at start)
#define RUXNWDDM_ESCAPE_RING_NOP	3	// send count command 30s, each waited for
#define RUXNWDDM_ESCAPE_UMD_NOTE	4	// the user-mode driver reports an event
#define RUXNWDDM_ESCAPE_PVG		5	// one of ruxnpvg's task controls (pvg_client.c)
#define RUXNWDDM_ESCAPE_PVG_INFO	6	// the tasks', surfaces' and display's counters
#define RUXNWDDM_ESCAPE_BLT_DEST	7	// where this process's blt presents of a surface go
#define RUXNWDDM_ESCAPE_ALLOCATIONS	8	// the live allocations, made and opened (alloc_list.c)
#define RUXNWDDM_ESCAPE_LOG		9	// a line of a process that cannot write a log (klog.c)
#define RUXNWDDM_ESCAPE_LOG_READ	10	// those lines read back; administrators only

// What the user-mode driver reports: which event (UMD_NOTE_* in umd.h), a
// value, and the process it happened in. Kept as counters and last values.
#define RUXNWDDM_UMD_NOTES		8

struct ruxnwddm_escape_header {
	unsigned int magic;
	unsigned int version;
	unsigned int command;
	unsigned int status;		// the driver's NTSTATUS for this escape
};

// Where the desktop's pixels go. Every way is a CPU copy for now: the
// display-only driver model hands the driver a picture in system memory.
#define RUXNWDDM_SCANOUT_NONE		0
#define RUXNWDDM_SCANOUT_POST		1	// the framebuffer Windows handed over
#define RUXNWDDM_SCANOUT_EFB		2	// the early framebuffer found by its registers

struct ruxnwddm_escape_info {
	struct ruxnwddm_escape_header header;
	unsigned int driver_version;	// 0xMMmmpppp
	unsigned int scanout;		// RUXNWDDM_SCANOUT_*
	unsigned int post_status;	// what DxgkCbAcquirePostDisplayOwnership returned
	unsigned int post_width, post_height, post_pitch, post_format;
	unsigned long long post_address;
	unsigned int width, height, pitch;	// the mode in use
	unsigned int efb_page, efb_bytes, efb_depth, efb_mode;	// the bank as read at start
	unsigned int bar0_length, bar2_length;
	unsigned int interrupt_messages;
	unsigned int source_visible;
	unsigned int commits;		// CommitVidPn calls that set a mode
	unsigned int power_state;	// DEVICE_POWER_STATE of the adapter
	unsigned long long presents;	// PresentDisplayOnly calls
	unsigned long long presents_skipped;	// while not visible or not set up
	unsigned long long moves, dirty_rects;
	unsigned long long bytes_copied;
	unsigned long long irq_taken, irq_done, irq_events, irq_faults, irq_empty, irq_other;
	unsigned int ring_on, ring_version;
	unsigned long long ring_sent, ring_late, ring_refused;
	// The full driver's (milestone 2), zero for the display-only one.
	unsigned int model;		// 1 display-only, 2 full
	unsigned int vsync_enabled;
	unsigned long long devices, contexts, processes;
	unsigned long long allocations, allocations_freed, primaries;
	unsigned long long submits, paging_submits, paging_ops, paging_skipped;
	unsigned long long fences_written, fences_lost, kernel_presents;
	unsigned long long flips, vsyncs;
	unsigned int umd_count[RUXNWDDM_UMD_NOTES];
	unsigned int umd_value[RUXNWDDM_UMD_NOTES];
	unsigned int umd_process[RUXNWDDM_UMD_NOTES];
};

struct ruxnwddm_escape_ring {
	struct ruxnwddm_escape_header header;
	unsigned int count;		// in: commands to send, 1 to 100000
	unsigned int sent;		// out: how many went through
	unsigned int late;		// out: found by looking, not by interrupt
	unsigned int first_error;	// out: the first failure's NTSTATUS, or 0
	unsigned long long microseconds;	// out: time taken
};


struct ruxnwddm_escape_note {
	struct ruxnwddm_escape_header header;
	unsigned int what;
	unsigned int value;
	unsigned int process;
	unsigned int reserved;
};

// Where the kernel's blt presents of one of this process's surfaces went:
// the compositor's surface for the window (blt_present.c), so that the
// user-mode driver copies into it on the device (umd_blt.c).
struct ruxnwddm_escape_blt_dest {
	struct ruxnwddm_escape_header header;
	unsigned long long src_token;	// in: the presented surface's token
	unsigned long long dst_token;	// out: 0 if no blt of it was seen
	unsigned int width, height, format;	// out: the destination's
	unsigned int reserved;
};

// A task control of ruxnpvg's (RUXNPVG_IOCTL_* of ruxnpvg_ioctl.h), sent on
// a kernel device the program made on our adapter. data holds the
// control's input, in_length bytes of it; the driver writes the control's
// output over it and says how long it is. The private data's size is this
// structure's and capacity's together. The control's own NTSTATUS comes
// back in header.status.
struct ruxnwddm_escape_pvg {
	struct ruxnwddm_escape_header header;
	unsigned int code;		// in: RUXNPVG_IOCTL_*
	unsigned int in_length;		// in
	unsigned int out_length;	// out
	unsigned int capacity;		// in: bytes of data
	unsigned char data[];
};

// The counters ruxnpvg's TASK_INFO and DISPLAY_INFO gave, and what the
// flips did with surfaces.
#define RUXNWDDM_SUBMIT_PROCESSES	32
#define RUXNWDDM_IMAGE_NAME		16	// as the kernel keeps it, 15 characters

struct ruxnwddm_escape_pvg_info {
	struct ruxnwddm_escape_header header;
	struct ruxnpvg_task_info tasks;
	struct ruxnpvg_display_info display;
	unsigned long long flips_shown;		// flips turned into a swap of a surface
	unsigned long long flips_copied;	// flips of a primary with no surface (stepping stone)
	unsigned long long flip_failures;	// swaps that failed
	unsigned long long surface_allocations;	// allocations made over a surface
	unsigned long long surface_imports_failed;
	unsigned long long present_flips, present_blts, present_to_primary;	// kernel presents by kind
	unsigned int shown_surface_pid;		// the process whose surface was shown last
	unsigned int efb_marked;		// the early framebuffer was painted the marker colour
	// Timing of the vertical blank and the swaps, by KeQueryPerformanceCounter
	// (counts in qpc_frequency a second); buckets as in full.h.
	long long first_vsync, last_vsync, first_swap, last_swap, qpc_frequency;
	unsigned long long vsyncs, flips;
	unsigned long long vsync_gaps[4];	// <12, <20, <28, more ms
	unsigned long long swap_times[5];	// <2, <4, <8, <16, more ms
	unsigned long long swap_most_us, superseded;
	unsigned int high_res_vsync;
	// Power cycles: a resume may come back on a new device (app3), after
	// which the mapper is told of its ring again (mapper_started grows).
	unsigned long long power_downs, power_ups, mapper_power_downs, mapper_started;
	unsigned long long flip_failures_before_power_up;
	// Submits (command 67) by the process that sent them: the first
	// RUXNWDDM_SUBMIT_PROCESSES processes to submit, with how many went
	// through and how many the device refused.
	unsigned int submit_pid[RUXNWDDM_SUBMIT_PROCESSES];
	unsigned int submit_done[RUXNWDDM_SUBMIT_PROCESSES];
	unsigned int submit_refused[RUXNWDDM_SUBMIT_PROCESSES];
	char submit_image[RUXNWDDM_SUBMIT_PROCESSES][RUXNWDDM_IMAGE_NAME];
};

// RUXNWDDM_ESCAPE_ALLOCATIONS: the first RUXNWDDM_ALLOCATIONS_MOST live
// allocations, made ones (opener 0) and opened copies (opener set).
#define RUXNWDDM_ALLOCATIONS_MOST	512

struct ruxnwddm_escape_allocation {
	unsigned int process;		// the maker, or for a copy the opener
	unsigned int opened;		// 1 for a copy a device opened
	unsigned int flags, format, width, height;
	unsigned int surface_holders;	// of the surface it holds, 0 if none
	unsigned int reserved;
	unsigned long long surface_token;
};

struct ruxnwddm_escape_allocations {
	struct ruxnwddm_escape_header header;
	unsigned int live, returned;
	struct ruxnwddm_escape_allocation list[RUXNWDDM_ALLOCATIONS_MOST];
};

// RUXNWDDM_ESCAPE_LOG: one line, kept with its process id while
// Parameters\SandboxLog is 1; STATUS_NOT_SUPPORTED in the status when not,
// after which the sender stops sending.
struct ruxnwddm_escape_log {
	struct ruxnwddm_escape_header header;
	unsigned int length;
	unsigned int reserved;
	char text[496];
};

// RUXNWDDM_ESCAPE_LOG_READ: what was written after byte `from` of the log
// (or from the oldest byte kept), `length` bytes of it, and how many bytes
// were ever written.
struct ruxnwddm_escape_log_read {
	struct ruxnwddm_escape_header header;
	unsigned long long from;
	unsigned long long total;
	unsigned int length;
	unsigned int reserved;
	char text[8192];
};

#endif
