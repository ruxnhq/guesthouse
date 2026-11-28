// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// ruxnwddm: a WDDM display miniport for the paravirtualized graphics PCI
// device. Milestone 1 is a display-only driver (the kernel-mode display-only
// model of WDDM 1.2): Windows composes the desktop in software and hands
// each changed rectangle to the driver, which copies it into the
// framebuffer the device already shows. That copy is a stepping stone,
// marked as such everywhere it happens; the full driver of the later
// milestones flips GPU-drawn surfaces instead.
//
// The device code (registers, ring, pages) is ported from ruxnpvg, which
// is a KMDF driver; here the graphics kernel owns the device and hands us
// its resources and interrupts.

#ifndef RUXNWDDM_H
#define RUXNWDDM_H

#include <ntddk.h>
#include <dispmprt.h>

#include "ruxnwddm_escape.h"
#include "full.h"

#define RUXNWDDM_DRIVER_VERSION		0x00030000u	// 0.3.0

// The device's PCI ids. The driver checks them before it maps anything.
#define RUXNWDDM_VENDOR_ID		0x106B
#define RUXNWDDM_DEVICE_ID		0xEEEE

// BAR 0 holds the device's 32-bit registers.
#define RUXNWDDM_REGS_LENGTH		0x4000

// The registers of BAR 0 this driver uses. Milestone 1 only reads the early
// framebuffer's bank; the ring's registers are written only after the
// RING_START escape, with the same sequence ruxnpvg uses.
#define REG_RING_ON			0x1000
#define REG_RING_BYTES			0x1004
#define REG_RING_WRITTEN		0x1008	// writing it is the kick
#define REG_RING_READ			0x100c
#define REG_RING_COMMANDS		0x1010
#define REG_DISPLAY_EVENTS		0x1014	// clears when read
#define REG_DONE			0x1018	// clears when read
#define REG_ROOT_PAGE			0x101c
#define REG_RING_RESUME			0x1024
#define REG_FAULT			0x102c
#define REG_RING_PAGE			0x1030
#define REG_VERSION			0x1034
#define REG_RING_REFUSED		0x103c
#define REG_MODE_COUNT			0x1204
// The early framebuffer's bank, written by the firmware's pvgfb.
#define REG_EFB_PAGE			0x1210
#define REG_EFB_BYTES			0x1214
#define REG_EFB_DEPTH			0x1218
#define REG_EFB_MODE			0x121c	// height above width, 16 bits each

// The device counts memory in 16 KiB pages.
#define RUXNWDDM_PAGE_SHIFT		14
#define RUXNWDDM_PAGE_SIZE		(1UL << RUXNWDDM_PAGE_SHIFT)

// One source, one target, one monitor.
#define RUXNWDDM_SOURCES		1
#define RUXNWDDM_CHILDREN		1
#define RUXNWDDM_SOURCE_ID		0
#define RUXNWDDM_TARGET_ID		0

// The adapter's own power state is reported for hardware id 0; the child's
// for its target id. Both happen to be 0 here, so the call says which.
#define RUXNWDDM_ADAPTER_UID		DISPLAY_ADAPTER_HW_ID

#define RUXNWDDM_BYTES_PER_PIXEL	4

// The one refresh rate the VidPN offers, and that a primary is described
// with: the graphics kernel compares the two as fractions.
#define RUXNWDDM_REFRESH_HZ		60
#define RUXNWDDM_BITS_PER_PIXEL		32

// The device asks for two MSI messages: 0 for the graphics device, 1 for
// the mapper.
#define RUXNWDDM_MSG_GRAPHICS		0
#define RUXNWDDM_MSG_MAPPER		1

// A block of device memory: zeroed, contiguous, aligned to 16 KiB.
typedef struct _RUXNWDDM_PAGES {
	PVOID allocation;
	PVOID va;
	PHYSICAL_ADDRESS phys;
	SIZE_T bytes;
} RUXNWDDM_PAGES;

// A lock for code that may wait while holding it, at PASSIVE_LEVEL.
typedef struct _RUXNWDDM_LOCK {
	KEVENT event;
} RUXNWDDM_LOCK;

static __forceinline void ruxnwddm_lock_init(RUXNWDDM_LOCK *lock)
{
	KeInitializeEvent(&lock->event, SynchronizationEvent, TRUE);
}

static __forceinline void ruxnwddm_lock(RUXNWDDM_LOCK *lock)
{
	KeWaitForSingleObject(&lock->event, Executive, KernelMode, FALSE, NULL);
}

static __forceinline void ruxnwddm_unlock(RUXNWDDM_LOCK *lock)
{
	KeSetEvent(&lock->event, IO_NO_INCREMENT, FALSE);
}

// Bumped by the interrupt routine at its IRQL.
typedef struct _RUXNWDDM_IRQ_COUNTS {
	volatile LONG64 taken;
	volatile LONG64 done;
	volatile LONG64 events;
	volatile LONG64 faults;
	volatile LONG64 empty;
	volatile LONG64 other;		// message 1, the mapper's
} RUXNWDDM_IRQ_COUNTS;

// The first command ring, as in ruxnpvg's ring.c.
typedef struct _RUXNWDDM_RING {
	RUXNWDDM_LOCK lock;
	BOOLEAN on;
	ULONG version;
	RUXNWDDM_PAGES root;
	RUXNWDDM_PAGES pages;
	volatile ULONG *mem;
	ULONG written;
	ULONG waited;
	ULONG rewind_at;
	ULONG last_stamp;
	ULONG64 sent;
	ULONG64 late;
	ULONG64 rewinds;
	ULONG64 refused;
	KEVENT wake;
	volatile LONG irq_stamp;
	volatile LONG faulted;
} RUXNWDDM_RING;

// Where the desktop is copied to, and in what shape.
typedef struct _RUXNWDDM_SCANOUT {
	ULONG kind;			// RUXNWDDM_SCANOUT_*
	PHYSICAL_ADDRESS phys;
	SIZE_T bytes;			// mapped length
	PUCHAR va;			// mapped write-combined
	ULONG width, height, pitch;
	D3DDDIFORMAT format;
} RUXNWDDM_SCANOUT;

// What the presents have done.
typedef struct _RUXNWDDM_PRESENT_COUNTS {
	ULONG64 presents;
	ULONG64 skipped;
	ULONG64 moves;
	ULONG64 dirty_rects;
	ULONG64 bytes;
} RUXNWDDM_PRESENT_COUNTS;

struct _RUXNWDDM_ADAPTER {
	PDEVICE_OBJECT pdo;
	// The driver model this start runs: display-only (milestone 1) or
	// full render and display (milestone 2), chosen at DriverEntry.
	BOOLEAN full;
	DXGKRNL_INTERFACE dxgk;
	DXGK_DEVICE_INFO device_info;
	BOOLEAN started;

	// BAR 0 and BAR 2 as assigned. BAR 2 is found, not mapped, for now.
	PHYSICAL_ADDRESS regs_phys;
	ULONG regs_length;
	volatile ULONG *regs;
	PHYSICAL_ADDRESS mapper_phys;
	ULONG mapper_length;
	ULONG interrupt_messages;

	// The early framebuffer's bank as read at start.
	ULONG efb_page, efb_bytes, efb_depth, efb_mode;

	// What Windows handed over at start, and its status.
	DXGK_DISPLAY_INFORMATION post;
	NTSTATUS post_status;

	RUXNWDDM_SCANOUT scanout;
	RUXNWDDM_PRESENT_COUNTS present;

	// The VidPN's state, as committed.
	BOOLEAN source_visible;
	BOOLEAN mode_set;
	ULONG commits;
	DEVICE_POWER_STATE adapter_power;
	DEVICE_POWER_STATE monitor_power;

	RUXNWDDM_IRQ_COUNTS irq;
	RUXNWDDM_RING ring;

	// The tasks, surfaces, mapper and display of ruxnpvg (pvg.h), for the
	// full driver; allocated at add-device.
	struct _RUXNWDDM_PVG *pvg;

	// The full driver's parts; unused by the display-only one.
	RUXNWDDM_FENCE fence;
	RUXNWDDM_FLIP flip;
	RUXNWDDM_VA_MAP va_map;
	RUXNWDDM_FULL_COUNTS counts;
	RUXNWDDM_BLT_DESTS blt_dests;
	RUXNWDDM_ALLOC_LIST allocs;
};

static __forceinline ULONG ruxnwddm_read(RUXNWDDM_ADAPTER *a, ULONG reg)
{
	return READ_REGISTER_ULONG((volatile ULONG *)((volatile UCHAR *)a->regs + reg));
}

static __forceinline void ruxnwddm_write(RUXNWDDM_ADAPTER *a, ULONG reg, ULONG value)
{
	WRITE_REGISTER_ULONG((volatile ULONG *)((volatile UCHAR *)a->regs + reg), value);
}

// adapter.c: start, stop, power, adapter and child queries.
NTSTATUS ruxnwddm_start(RUXNWDDM_ADAPTER *a, DXGK_START_INFO *start_info,
			DXGKRNL_INTERFACE *dxgk, ULONG *views, ULONG *children);
NTSTATUS ruxnwddm_stop(RUXNWDDM_ADAPTER *a);
NTSTATUS ruxnwddm_set_power(RUXNWDDM_ADAPTER *a, ULONG uid, DEVICE_POWER_STATE state,
			    POWER_ACTION action);
NTSTATUS ruxnwddm_query_adapter_info(RUXNWDDM_ADAPTER *a, const DXGKARG_QUERYADAPTERINFO *args);
NTSTATUS ruxnwddm_query_child_relations(RUXNWDDM_ADAPTER *a, DXGK_CHILD_DESCRIPTOR *children,
					ULONG size);
NTSTATUS ruxnwddm_query_child_status(RUXNWDDM_ADAPTER *a, DXGK_CHILD_STATUS *status);
NTSTATUS ruxnwddm_release_post_display(RUXNWDDM_ADAPTER *a, D3DDDI_VIDEO_PRESENT_TARGET_ID target,
				       DXGK_DISPLAY_INFORMATION *info);

// scanout.c: finding and mapping the framebuffer the desktop is copied to.
NTSTATUS ruxnwddm_scanout_open(RUXNWDDM_ADAPTER *a);
void ruxnwddm_scanout_close(RUXNWDDM_ADAPTER *a);

// vidpn.c: the one source, target and mode.
NTSTATUS ruxnwddm_is_supported_vidpn(RUXNWDDM_ADAPTER *a, DXGKARG_ISSUPPORTEDVIDPN *args);
NTSTATUS ruxnwddm_enum_cofunc_modality(RUXNWDDM_ADAPTER *a,
				       const DXGKARG_ENUMVIDPNCOFUNCMODALITY *args);
NTSTATUS ruxnwddm_commit_vidpn(RUXNWDDM_ADAPTER *a, const DXGKARG_COMMITVIDPN *args);
NTSTATUS ruxnwddm_recommend_monitor_modes(RUXNWDDM_ADAPTER *a,
					  const DXGKARG_RECOMMENDMONITORMODES *args);

// present.c: the desktop's pixels into the scanout, by the CPU.
NTSTATUS ruxnwddm_present(RUXNWDDM_ADAPTER *a, const DXGKARG_PRESENT_DISPLAYONLY *args);
void ruxnwddm_blank(RUXNWDDM_ADAPTER *a);
NTSTATUS ruxnwddm_system_display_enable(RUXNWDDM_ADAPTER *a, UINT *width, UINT *height,
					D3DDDIFORMAT *format);
void ruxnwddm_system_display_write(RUXNWDDM_ADAPTER *a, const VOID *source, UINT width,
				   UINT height, UINT stride, UINT x, UINT y);

// interrupt.c
BOOLEAN ruxnwddm_interrupt(RUXNWDDM_ADAPTER *a, ULONG message);
void ruxnwddm_dpc(RUXNWDDM_ADAPTER *a);

// escape.c
NTSTATUS ruxnwddm_escape(RUXNWDDM_ADAPTER *a, const DXGKARG_ESCAPE *args);

// pages.c
NTSTATUS ruxnwddm_alloc_pages(RUXNWDDM_PAGES *block, SIZE_T bytes);
void ruxnwddm_free_pages(RUXNWDDM_PAGES *block);
ULONG ruxnwddm_page_number(const RUXNWDDM_PAGES *block);
NTSTATUS ruxnwddm_alloc_task_pages(RUXNWDDM_PAGES *block, SIZE_T bytes);
NTSTATUS ruxnwddm_alloc_mapper_pages(RUXNWDDM_PAGES *block, SIZE_T bytes);

// ring.c
void ruxnwddm_ring_init(RUXNWDDM_ADAPTER *a);
NTSTATUS ruxnwddm_ring_start(RUXNWDDM_ADAPTER *a);
void ruxnwddm_ring_stop(RUXNWDDM_ADAPTER *a);
void ruxnwddm_ring_free(RUXNWDDM_ADAPTER *a);
void ruxnwddm_ring_irq(RUXNWDDM_ADAPTER *a, BOOLEAN fault);
NTSTATUS ruxnwddm_ring_nop(RUXNWDDM_ADAPTER *a, BOOLEAN *late);
NTSTATUS ruxnwddm_ring_send(RUXNWDDM_ADAPTER *a, ULONG number, const ULONG *payload, ULONG words,
			    ULONG timeout_ms);

// record.c
void ruxnwddm_record_ulong(PCWSTR name, ULONG value);
void ruxnwddm_record_ulong64(PCWSTR name, ULONG64 value);
ULONG ruxnwddm_record_count(PCWSTR name);
ULONG ruxnwddm_record_read(PCWSTR name);
NTSTATUS ruxnwddm_note(PCWSTR name, NTSTATUS status);
NTSTATUS ruxnwddm_record_init(PUNICODE_STRING registry_path);
void ruxnwddm_record_free(void);

// blt_present.c
void ruxnwddm_blt_init(RUXNWDDM_ADAPTER *a);
void ruxnwddm_blt_note(RUXNWDDM_ADAPTER *a, const RUXNWDDM_ALLOCATION *src, const RUXNWDDM_ALLOCATION *dst);
NTSTATUS ruxnwddm_blt_dest(RUXNWDDM_ADAPTER *a, struct ruxnwddm_escape_blt_dest *q);
// klog.c: lines of processes that cannot write a log of their own.
NTSTATUS ruxnwddm_klog_line(struct ruxnwddm_escape_log *line);
NTSTATUS ruxnwddm_klog_read(struct ruxnwddm_escape_log_read *read);
void ruxnwddm_klog_event(const char *what, ULONG64 a, ULONG64 b, ULONG64 c, LONG status);

#endif
