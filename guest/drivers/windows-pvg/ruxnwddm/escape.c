// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Escapes: the private calls wddmtool makes with D3DKMTEscape. INFO reads
// the driver's counters and touches no register. RING_START and RING_NOP
// are the ring of ruxnpvg's stage B under the graphics kernel, kept apart
// so that a run decides whether the device hears anything at all.
//
// PVG carries one of ruxnpvg's task controls to the task of the kernel
// device the escape names (pvg_client.c); PVG_INFO reads the counters of
// the tasks, the surfaces, the display and the flips.
//
// Escapes come at PASSIVE_LEVEL in the caller's process. Windows lets any
// process send them, so each one is checked for size and magic, and only
// administrators may touch the ring by hand, list the live allocations, read
// the kernel log, or read more than the driver's version and the adapter's
// presence from INFO and PVG_INFO.

#include "pvg.h"

#define POOL_TAG_ESCAPE	'exur'	// PVG_INFO's room while it is filled

#define MAX_NOPS	100000

// From ntifs.h, which does not mix with the display headers.
NTKERNELAPI BOOLEAN SeTokenIsAdmin(PACCESS_TOKEN token);

static PACCESS_TOKEN effective_token(PSECURITY_SUBJECT_CONTEXT subject)
{
	return subject->ClientToken ? subject->ClientToken : subject->PrimaryToken;
}

// What any process may read: which driver this is. The rest, the display's
// addresses and sizes and the counters, is for administrators.
static void fill_info_basics(RUXNWDDM_ADAPTER *a, struct ruxnwddm_escape_info *info)
{
	RtlZeroMemory((UCHAR *)info + sizeof(info->header), sizeof(*info) - sizeof(info->header));
	info->driver_version = RUXNWDDM_DRIVER_VERSION;
	info->model = a->full ? 2 : 1;
}

static void fill_info(RUXNWDDM_ADAPTER *a, struct ruxnwddm_escape_info *info)
{
	info->driver_version = RUXNWDDM_DRIVER_VERSION;
	info->scanout = a->scanout.kind;
	info->post_status = (unsigned int)a->post_status;
	info->post_width = a->post.Width;
	info->post_height = a->post.Height;
	info->post_pitch = a->post.Pitch;
	info->post_format = (unsigned int)a->post.ColorFormat;
	info->post_address = (unsigned long long)a->post.PhysicAddress.QuadPart;
	info->width = a->scanout.width;
	info->height = a->scanout.height;
	info->pitch = a->scanout.pitch;
	info->efb_page = a->efb_page;
	info->efb_bytes = a->efb_bytes;
	info->efb_depth = a->efb_depth;
	info->efb_mode = a->efb_mode;
	info->bar0_length = a->regs_length;
	info->bar2_length = a->mapper_length;
	info->interrupt_messages = a->interrupt_messages;
	info->source_visible = a->source_visible;
	info->commits = a->commits;
	info->power_state = (unsigned int)a->adapter_power;
	info->presents = a->present.presents;
	info->presents_skipped = a->present.skipped;
	info->moves = a->present.moves;
	info->dirty_rects = a->present.dirty_rects;
	info->bytes_copied = a->present.bytes;
	info->irq_taken = (unsigned long long)a->irq.taken;
	info->irq_done = (unsigned long long)a->irq.done;
	info->irq_events = (unsigned long long)a->irq.events;
	info->irq_faults = (unsigned long long)a->irq.faults;
	info->irq_empty = (unsigned long long)a->irq.empty;
	info->irq_other = (unsigned long long)a->irq.other;
	info->ring_on = a->ring.on;
	info->ring_version = a->ring.version;
	info->ring_sent = a->ring.sent;
	info->ring_late = a->ring.late;
	info->ring_refused = a->ring.refused;
	info->model = a->full ? 2 : 1;
	info->vsync_enabled = a->flip.vsync_enabled;
	info->devices = (unsigned long long)a->counts.devices;
	info->contexts = (unsigned long long)a->counts.contexts;
	info->processes = (unsigned long long)a->counts.processes;
	info->allocations = (unsigned long long)a->counts.allocations;
	info->allocations_freed = (unsigned long long)a->counts.allocations_freed;
	info->primaries = (unsigned long long)a->counts.primaries;
	info->submits = (unsigned long long)a->counts.submits;
	info->paging_submits = (unsigned long long)a->counts.paging_submits;
	info->paging_ops = (unsigned long long)a->counts.paging_ops;
	info->paging_skipped = (unsigned long long)a->counts.paging_skipped;
	info->fences_written = (unsigned long long)a->counts.fences_written;
	info->fences_lost = (unsigned long long)a->counts.fences_lost;
	info->kernel_presents = (unsigned long long)a->counts.presents;
	info->flips = a->flip.flips;
	info->vsyncs = a->flip.vsyncs;
	for (ULONG i = 0; i < RUXNWDDM_UMD_NOTES; i++) {
		info->umd_count[i] = (unsigned int)a->counts.umd_count[i];
		info->umd_value[i] = a->counts.umd_value[i];
		info->umd_process[i] = a->counts.umd_process[i];
	}
}

// Kept in memory, and the first of each in the registry, so a run can read
// it after the fact even if wddmtool cannot reach the adapter.
static void umd_note(RUXNWDDM_ADAPTER *a, const struct ruxnwddm_escape_note *note)
{
	static const PCWSTR names[RUXNWDDM_UMD_NOTES] = {
		L"UmdNote0", L"UmdOpenAdapter", L"UmdCreateDevice", L"UmdCreateResource",
		L"UmdPresent", L"UmdDraws", L"UmdFailed", L"UmdNote7",
	};
	ULONG what = note->what < RUXNWDDM_UMD_NOTES ? note->what : 0;
	ULONG process = (ULONG)(ULONG_PTR)PsGetCurrentProcessId();

	if (InterlockedIncrement(&a->counts.umd_count[what]) <= 4) {
		ruxnwddm_record_ulong(names[what], note->value);
		ruxnwddm_record_count(L"UmdNotes");
	}
	a->counts.umd_value[what] = note->value;
	// The process is the caller, whatever the note says.
	a->counts.umd_process[what] = process;
}

static void fill_pvg_info(RUXNWDDM_ADAPTER *a, struct ruxnwddm_escape_pvg_info *info)
{
	pvg_task_info(a->pvg, &info->tasks);
	pvg_display_info(a->pvg, &info->display);
	info->flips_shown = a->flip.shown_count;
	info->flips_copied = a->flip.copied_count;
	info->flip_failures = a->flip.failures;
	info->surface_allocations = (unsigned long long)a->counts.surface_allocations;
	info->surface_imports_failed = (unsigned long long)a->counts.surface_imports_failed;
	info->present_flips = (unsigned long long)a->counts.present_flips;
	info->present_blts = (unsigned long long)a->counts.present_blts;
	info->present_to_primary = (unsigned long long)a->counts.present_to_primary;
	info->shown_surface_pid = a->flip.shown_pid;
	info->efb_marked = a->flip.efb_marked;
	info->first_vsync = a->flip.first_vsync;
	info->last_vsync = a->flip.last_vsync;
	info->first_swap = a->flip.first_swap;
	info->last_swap = a->flip.last_swap;
	info->qpc_frequency = a->flip.qpc_frequency;
	info->vsyncs = a->flip.vsyncs;
	info->flips = a->flip.flips;
	RtlCopyMemory(info->vsync_gaps, a->flip.vsync_gaps, sizeof(info->vsync_gaps));
	RtlCopyMemory(info->swap_times, a->flip.swap_times, sizeof(info->swap_times));
	info->swap_most_us = a->flip.swap_most_us;
	info->superseded = a->flip.superseded;
	info->high_res_vsync = a->flip.high_res_timer != NULL;
	info->power_downs = (unsigned long long)a->pvg->power_downs;
	info->power_ups = (unsigned long long)a->pvg->power_ups;
	info->mapper_power_downs = (unsigned long long)a->pvg->screen.mapper_power_downs;
	info->mapper_started = (unsigned long long)a->pvg->screen.mapper_started;
	info->flip_failures_before_power_up = a->flip.failures_before_power_up;
	for (ULONG i = 0; i < RUXNWDDM_SUBMIT_PROCESSES; i++) {
		info->submit_pid[i] = a->pvg->submit_pid[i];
		info->submit_done[i] = a->pvg->submit_done[i];
		info->submit_refused[i] = a->pvg->submit_refused[i];
		RtlCopyMemory(info->submit_image[i], a->pvg->submit_image[i], RUXNWDDM_IMAGE_NAME);
	}
}

static BOOLEAN caller_is_admin(void)
{
	SECURITY_SUBJECT_CONTEXT subject;
	BOOLEAN admin;

	SeCaptureSubjectContext(&subject);
	admin = SeTokenIsAdmin(effective_token(&subject));
	SeReleaseSubjectContext(&subject);
	return admin;
}

static NTSTATUS ring_nops(RUXNWDDM_ADAPTER *a, struct ruxnwddm_escape_ring *ring)
{
	LONGLONG start = (LONGLONG)KeQueryInterruptTime();
	ULONG i;

	ring->sent = 0;
	ring->late = 0;
	ring->first_error = 0;
	if (ring->count == 0 || ring->count > MAX_NOPS)
		return STATUS_INVALID_PARAMETER;
	for (i = 0; i < ring->count; i++) {
		BOOLEAN late;
		NTSTATUS status = ruxnwddm_ring_nop(a, &late);

		if (!NT_SUCCESS(status)) {
			ring->first_error = (unsigned int)status;
			break;
		}
		ring->sent++;
		if (late)
			ring->late++;
	}
	// KeQueryInterruptTime counts in 100 ns.
	ring->microseconds = (unsigned long long)(KeQueryInterruptTime() - start) / 10;
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_escape(RUXNWDDM_ADAPTER *a, const DXGKARG_ESCAPE *args)
{
	struct ruxnwddm_escape_header *header = args->pPrivateDriverData;
	UINT size = args->PrivateDriverDataSize;
	NTSTATUS status;

	if (!header || size < sizeof(*header) || header->magic != RUXNWDDM_ESCAPE_MAGIC ||
	    header->version != RUXNWDDM_ESCAPE_VERSION)
		return STATUS_INVALID_PARAMETER;

	switch (header->command) {
	case RUXNWDDM_ESCAPE_INFO:
		if (size < sizeof(struct ruxnwddm_escape_info))
			return STATUS_BUFFER_TOO_SMALL;
		if (caller_is_admin())
			fill_info(a, (struct ruxnwddm_escape_info *)header);
		else
			fill_info_basics(a, (struct ruxnwddm_escape_info *)header);
		status = STATUS_SUCCESS;
		break;
	case RUXNWDDM_ESCAPE_RING_START:
		status = caller_is_admin() ? ruxnwddm_ring_start(a) : STATUS_ACCESS_DENIED;
		break;
	case RUXNWDDM_ESCAPE_UMD_NOTE:
		if (size < sizeof(struct ruxnwddm_escape_note))
			return STATUS_BUFFER_TOO_SMALL;
		umd_note(a, (const struct ruxnwddm_escape_note *)header);
		status = STATUS_SUCCESS;
		break;
	case RUXNWDDM_ESCAPE_RING_NOP:
		if (size < sizeof(struct ruxnwddm_escape_ring))
			return STATUS_BUFFER_TOO_SMALL;
		status = caller_is_admin() ?
			ring_nops(a, (struct ruxnwddm_escape_ring *)header) : STATUS_ACCESS_DENIED;
		break;
	case RUXNWDDM_ESCAPE_PVG: {
		RUXNWDDM_DEVICE *dev = args->hDevice;

		if (!a->pvg)
			return STATUS_INVALID_PARAMETER;
		status = pvg_escape(a->pvg, dev ? (RUXNWDDM_CLIENT **)&dev->client : NULL, header,
				    size);
		break;
	}
	case RUXNWDDM_ESCAPE_ALLOCATIONS:
		if (size < sizeof(struct ruxnwddm_escape_allocations))
			return STATUS_BUFFER_TOO_SMALL;
		if (!caller_is_admin()) {
			status = STATUS_ACCESS_DENIED;
			break;
		}
		ruxnwddm_alloc_list_fill(a, (struct ruxnwddm_escape_allocations *)header);
		status = STATUS_SUCCESS;
		break;
	case RUXNWDDM_ESCAPE_LOG:
		if (size < sizeof(struct ruxnwddm_escape_log))
			return STATUS_BUFFER_TOO_SMALL;
		status = ruxnwddm_klog_line((struct ruxnwddm_escape_log *)header);
		break;
	case RUXNWDDM_ESCAPE_LOG_READ:
		if (size < sizeof(struct ruxnwddm_escape_log_read))
			return STATUS_BUFFER_TOO_SMALL;
		status = caller_is_admin() ? ruxnwddm_klog_read((struct ruxnwddm_escape_log_read *)header) :
					     STATUS_ACCESS_DENIED;
		break;
	case RUXNWDDM_ESCAPE_BLT_DEST:
		if (size < sizeof(struct ruxnwddm_escape_blt_dest))
			return STATUS_BUFFER_TOO_SMALL;
		status = ruxnwddm_blt_dest(a, (struct ruxnwddm_escape_blt_dest *)header);
		break;
	case RUXNWDDM_ESCAPE_PVG_INFO: {
		// As much as the caller has room for: fields are only ever added
		// at the end, and winemetal finds the adapter with this escape, so
		// a caller built against an older header must still be answered
		// (run rn19: one was not, and nothing drew).
		struct ruxnwddm_escape_pvg_info *info;
		ULONG room = min(size, (ULONG)sizeof(*info));

		if (!a->pvg)
			return STATUS_INVALID_PARAMETER;
		info = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*info), POOL_TAG_ESCAPE);
		if (!info)
			return STATUS_INSUFFICIENT_RESOURCES;
		RtlCopyMemory(info, header, sizeof(*header));
		// Any process may ask, to find the adapter by the answer's header;
		// the counters and the processes behind them are for administrators.
		if (caller_is_admin())
			fill_pvg_info(a, info);
		RtlCopyMemory((UCHAR *)header + sizeof(*header), (UCHAR *)info + sizeof(*header),
			      room - sizeof(*header));
		ExFreePoolWithTag(info, POOL_TAG_ESCAPE);
		status = STATUS_SUCCESS;
		break;
	}
	default:
		return STATUS_INVALID_PARAMETER;
	}
	header->status = (unsigned int)status;
	return STATUS_SUCCESS;
}
