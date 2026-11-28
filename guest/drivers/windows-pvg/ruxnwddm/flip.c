// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Flips of the full driver, and the vertical blank that confirms them.
//
// Windows tells the driver which allocation to show with
// SetVidPnSourceAddress and waits for a vertical-blank interrupt that names
// the address now on screen; until it comes, no queued flip completes and
// the compositor stalls. The device has no vertical blank to give, so a
// timer stands in for one at 60 Hz, as viogpu3d's flip thread does.
//
// A primary the user-mode driver made over a device surface (allocation.c)
// is shown as it is: a worker thread takes the display for the driver's own
// client the first time and then has the display show the surface with
// command 8 (pvg_display.c), once for each flip. No pixel is touched by the
// CPU. SetVidPnSourceAddress may come at the interrupt's level, where
// nothing can wait, so it only records the allocation; the vertical blank's
// DPC wakes the worker.
//
// STEPPING STONE, kept only as a marked fallback: a primary with no surface
// (Windows' own standard primary, or one whose surface could not be held)
// is made visible by the worker copying its pages into the early
// framebuffer, a few times a second. That is a CPU copy of the frame, and
// it is not used once the display shows surfaces. It is off unless
// Parameters\FlipCopy is 1: a primary Windows has unpinned and evicted
// keeps its old page numbers here, and mapping one of those stopped the
// guest (run rn9: 0x1A, 0x1233, a page not locked, from copy_thread).

#include "pvg.h"

#define VSYNC_PERIOD_MS		16
#define VSYNC_PERIOD_100NS	166667LL	// 60 Hz, for the high-resolution timer
#define VSYNC_CATCH_UP_100NS	10000LL		// the soonest the timer is re-armed for, 1 ms
#define COPY_EVERY_VSYNCS	4	// about 15 copies a second at most
#define TICKS_PER_MS		10000LL
// What the early framebuffer is painted once the display shows surfaces: a
// colour no desktop has, so a picture from it could never pass for ours.
#define EFB_MARKER		0x00ff00ffU	// magenta, as B, G, R, A bytes

// At the interrupt's level, under the graphics kernel's lock.
static BOOLEAN report_vsync(PVOID context)
{
	RUXNWDDM_ADAPTER *a = context;
	DXGKARGCB_NOTIFY_INTERRUPT_DATA note;

	RtlZeroMemory(&note, sizeof(note));
	note.InterruptType = DXGK_INTERRUPT_CRTC_VSYNC;
	note.CrtcVsync.VidPnTargetId = RUXNWDDM_TARGET_ID;
	note.CrtcVsync.PhysicalAddress = a->flip.address;
	a->dxgk.DxgkCbNotifyInterrupt(a->dxgk.DeviceHandle, &note);
	a->dxgk.DxgkCbQueueDpc(a->dxgk.DeviceHandle);
	return TRUE;
}

static ULONG64 ms_of(RUXNWDDM_ADAPTER *a, LONGLONG ticks, ULONG per_ms)
{
	return a->flip.qpc_frequency ? (ULONG64)(ticks * per_ms / a->flip.qpc_frequency) : 0;
}

// The gap since the last vertical blank, by bucket.
static void note_vsync(RUXNWDDM_ADAPTER *a)
{
	LONGLONG now = KeQueryPerformanceCounter(NULL).QuadPart;
	ULONG64 ms;

	if (!a->flip.first_vsync) {
		a->flip.first_vsync = now;
	} else {
		ms = ms_of(a, now - a->flip.last_vsync, 1000);
		a->flip.vsync_gaps[ms < 12 ? 0 : ms < 20 ? 1 : ms < 28 ? 2 : 3]++;
	}
	a->flip.last_vsync = now;
}

// Each vertical blank, from the KTIMER's DPC or the high-resolution timer.
static void vsync_tick(RUXNWDDM_ADAPTER *a)
{
	RUXNWDDM_ALLOCATION *shown;
	BOOLEAN ignored;

	note_vsync(a);
	a->flip.vsyncs++;
	if (a->flip.vsync_enabled)
		a->dxgk.DxgkCbSynchronizeExecution(a->dxgk.DeviceHandle, report_vsync, a, 0, &ignored);
	shown = a->flip.shown;
	if (!shown)
		return;
	// A new surface to show, at once; the stepping stone's copy, now and
	// then.
	if ((a->flip.shown_has_surface && shown != a->flip.swapped) ||
	    (!a->flip.shown_has_surface && a->flip.vsyncs % COPY_EVERY_VSYNCS == 0))
		KeSetEvent(&a->flip.copy_wake, IO_NO_INCREMENT, FALSE);
}

static KDEFERRED_ROUTINE vsync_dpc;

static void vsync_dpc(PKDPC dpc, PVOID context, PVOID arg1, PVOID arg2)
{
	UNREFERENCED_PARAMETER(dpc);
	UNREFERENCED_PARAMETER(arg1);
	UNREFERENCED_PARAMETER(arg2);
	vsync_tick(context);
}

static EXT_CALLBACK vsync_high_res;

// Arms the one-shot timer for the next slot of an absolute 60 Hz schedule.
// A periodic timer re-arms from its own late expiry, so under the
// hypervisor every lateness added up (expiries every 17.1 to 17.4 ms, about
// 52 vertical blanks a second, rn20); here a late callback only shortens
// the next gap, as viogpu3d does. A callback more than a period late gets
// one more blank about a millisecond later rather than losing the slot; one
// more than two periods late moves the schedule on.
static void vsync_arm(RUXNWDDM_ADAPTER *a)
{
	LONGLONG now = (LONGLONG)KeQueryInterruptTime();
	LONGLONG wait;

	a->flip.next_due += VSYNC_PERIOD_100NS;
	if (a->flip.next_due + 2 * VSYNC_PERIOD_100NS <= now) {
		a->flip.next_due = now + VSYNC_PERIOD_100NS;
		a->flip.vsyncs_resynced++;
	} else if (a->flip.next_due <= now) {
		a->flip.vsyncs_late++;
	}
	wait = a->flip.next_due - now;
	if (wait < VSYNC_CATCH_UP_100NS)
		wait = VSYNC_CATCH_UP_100NS;
	ExSetTimer(a->flip.high_res_timer, -wait, 0, NULL);
}

static void vsync_high_res(PEX_TIMER timer, PVOID context)
{
	RUXNWDDM_ADAPTER *a = context;

	UNREFERENCED_PARAMETER(timer);
	if (a->flip.stopping)
		return;
	vsync_tick(a);
	vsync_arm(a);
}

// Once, when the display is first taken: the early framebuffer, which the
// device no longer shows, is painted the marker colour. Not a frame path.
static void mark_efb(RUXNWDDM_ADAPTER *a)
{
	RUXNWDDM_SCANOUT *s = &a->scanout;
	SIZE_T i;

	if (!s->va || a->flip.efb_marked)
		return;
	for (i = 0; i + 4 <= (SIZE_T)s->pitch * s->height; i += 4)
		*(volatile ULONG *)(s->va + i) = EFB_MARKER;
	a->flip.efb_marked = TRUE;
}

// Shows the allocation's surface: the display is taken for the driver's
// client the first time, then told to show the surface. A failure is
// recorded and nothing is retried: a display command the device refused may
// have spoiled the display, and the guard's refusals are never repeated.
static BOOLEAN show_surface(RUXNWDDM_ADAPTER *a, RUXNWDDM_ALLOCATION *al)
{
	RUXNWDDM_CLIENT *display = a->pvg->display_client;
	NTSTATUS status = STATUS_SUCCESS;
	ULONG swap_done;

	if (a->flip.failures || !a->flip.powered)
		return FALSE;
	ruxnwddm_lock(&display->lock);
	if (a->pvg->display->owner != display) {
		status = pvg_display_take(display);
		if (NT_SUCCESS(status))
			mark_efb(a);
	}
	if (NT_SUCCESS(status)) {
		LONGLONG start = KeQueryPerformanceCounter(NULL).QuadPart, end;
		ULONG64 us;

		status = pvg_display_swap(display, al->surface, &swap_done);
		end = KeQueryPerformanceCounter(NULL).QuadPart;
		us = ms_of(a, end - start, 1000000);
		a->flip.swap_times[us < 2000 ? 0 : us < 4000 ? 1 : us < 8000 ? 2 : us < 16000 ? 3 : 4]++;
		if (us > a->flip.swap_most_us)
			a->flip.swap_most_us = us;
		if (!a->flip.first_swap)
			a->flip.first_swap = end;
		a->flip.last_swap = end;
	}
	ruxnwddm_unlock(&display->lock);
	if (!NT_SUCCESS(status)) {
		a->flip.failures++;
		ruxnwddm_record_ulong(L"FlipShowFailed", (ULONG)status);
		return FALSE;
	}
	a->flip.swapped = al;
	a->flip.shown_count++;
	a->flip.shown_pid = al->maker_process;
	if (a->flip.shown_count == 1)
		ruxnwddm_record_ulong(L"FlipFirstShownPid", al->maker_process);
	return TRUE;
}

// STEPPING STONE: copies the primary's pages, one at a time, into the early
// framebuffer. The pages are what the last page-table update named
// (paging.c).
static void copy_primary(RUXNWDDM_ADAPTER *a, RUXNWDDM_ALLOCATION *al)
{
	RUXNWDDM_SCANOUT *s = &a->scanout;
	SIZE_T bytes = min((SIZE_T)al->info.size, (SIZE_T)s->pitch * s->height);
	ULONG i;

	if (!s->va || !al->pages || al->info.pitch != s->pitch)
		return;
	for (i = 0; i < al->page_count && (SIZE_T)i * PAGE_SIZE < bytes; i++) {
		PHYSICAL_ADDRESS pa;
		PVOID page;

		if (!al->pages[i])
			continue;
		pa.QuadPart = (LONGLONG)al->pages[i];
		page = MmMapIoSpaceEx(pa, PAGE_SIZE, PAGE_READONLY);
		if (!page)
			continue;
		RtlCopyMemory(s->va + (SIZE_T)i * PAGE_SIZE, page,
			      min((SIZE_T)PAGE_SIZE, bytes - (SIZE_T)i * PAGE_SIZE));
		MmUnmapIoSpace(page, PAGE_SIZE);
	}
	a->present.bytes += bytes;
	a->present.presents++;
}

static KSTART_ROUTINE copy_thread;

static void copy_thread(PVOID context)
{
	RUXNWDDM_ADAPTER *a = context;

	while (!a->flip.stopping) {
		RUXNWDDM_ALLOCATION *al;

		KeWaitForSingleObject(&a->flip.copy_wake, Executive, KernelMode, FALSE, NULL);
		if (a->flip.stopping)
			break;
		// Claim the shown allocation, then check it is still shown: a
		// destroy in between waits for the claim to go (flip_forget).
		al = a->flip.shown;
		InterlockedExchangePointer((PVOID volatile *)&a->flip.copying, al);
		if (al && a->flip.shown == al) {
			ULONG64 flips = a->flip.flips;

			// More than one flip since the last look: the ones between
			// were replaced before they could be shown.
			if (al->surface && flips > a->flip.flips_seen + 1 && a->flip.flips_seen)
				a->flip.superseded += flips - a->flip.flips_seen - 1;
			a->flip.flips_seen = flips;
			if (al->surface)
				show_surface(a, al);
			else if (a->flip.copy_allowed && !a->flip.shown_count) {
				// The stepping stone, only until surfaces are shown.
				copy_primary(a, al);
				a->flip.copied_count++;
			}
		}
		InterlockedExchangePointer((PVOID volatile *)&a->flip.copying, NULL);
	}
	PsTerminateSystemThread(STATUS_SUCCESS);
}

NTSTATUS ruxnwddm_flip_start(RUXNWDDM_ADAPTER *a)
{
	LARGE_INTEGER due, freq;
	HANDLE thread;
	NTSTATUS status;

	KeInitializeEvent(&a->flip.copy_wake, SynchronizationEvent, FALSE);
	a->flip.stopping = FALSE;
	a->flip.powered = TRUE;
	a->flip.copy_allowed = ruxnwddm_record_read(L"FlipCopy") == 1;
	status = PsCreateSystemThread(&thread, THREAD_ALL_ACCESS, NULL, NULL, NULL, copy_thread, a);
	if (!NT_SUCCESS(status))
		return status;
	ObReferenceObjectByHandle(thread, THREAD_ALL_ACCESS, NULL, KernelMode,
				  (PVOID *)&a->flip.thread, NULL);
	ZwClose(thread);

	KeQueryPerformanceCounter(&freq);
	a->flip.qpc_frequency = freq.QuadPart;
	// A one-shot high-resolution timer on an absolute schedule (vsync_arm).
	// Only if none can be had, a KTIMER of 16 ms, which fires on the system
	// clock's ticks (15.6 ms unless something asks for finer), so its gaps
	// go 15.6 and 31.2 ms.
	a->flip.vsyncs_late = 0;
	a->flip.vsyncs_resynced = 0;
	a->flip.high_res_timer = ExAllocateTimer(vsync_high_res, a, EX_TIMER_HIGH_RESOLUTION);
	if (a->flip.high_res_timer) {
		a->flip.next_due = (LONGLONG)KeQueryInterruptTime();
		vsync_arm(a);
	} else {
		KeInitializeDpc(&a->flip.dpc, vsync_dpc, a);
		KeInitializeTimerEx(&a->flip.timer, NotificationTimer);
		due.QuadPart = -VSYNC_PERIOD_MS * TICKS_PER_MS;
		KeSetTimerEx(&a->flip.timer, due, VSYNC_PERIOD_MS, &a->flip.dpc);
	}
	a->flip.running = TRUE;
	return STATUS_SUCCESS;
}

void ruxnwddm_flip_stop(RUXNWDDM_ADAPTER *a)
{
	if (!a->flip.running)
		return;
	// First, so that the one-shot timer's callback re-arms no more.
	a->flip.stopping = TRUE;
	if (a->flip.high_res_timer) {
		ExDeleteTimer(a->flip.high_res_timer, TRUE, TRUE, NULL);
		a->flip.high_res_timer = NULL;
	} else {
		KeCancelTimer(&a->flip.timer);
	}
	KeFlushQueuedDpcs();
	KeSetEvent(&a->flip.copy_wake, IO_NO_INCREMENT, FALSE);
	if (a->flip.thread) {
		KeWaitForSingleObject(a->flip.thread, Executive, KernelMode, FALSE, NULL);
		ObDereferenceObject(a->flip.thread);
		a->flip.thread = NULL;
	}
	a->flip.running = FALSE;
}

// DXGI's four-byte blue-green-red formats, which the display takes as it is.
static BOOLEAN is_bgra(unsigned int format)
{
	return format == 87 || format == 88 || format == 90 || format == 91;	// B8G8R8A8/X8, typeless, sRGB
}

// May come at the interrupt's level for a flip: no lock, no waiting, no
// paging; the two fields are written with single stores.
NTSTATUS ruxnwddm_set_source_address(RUXNWDDM_ADAPTER *a, const DXGKARG_SETVIDPNSOURCEADDRESS *args)
{
	RUXNWDDM_ALLOCATION *al = args->hAllocation;

	if (args->VidPnSourceId != RUXNWDDM_SOURCE_ID)
		return STATUS_INVALID_PARAMETER;
	a->flip.address = args->PrimaryAddress;
	// Read here, while the allocation is surely alive, so that the DPC
	// never needs to look inside one that may be going.
	// Only blue-green-red surfaces are shown as they are; any other (an
	// RGBA8 shared texture) would come out with red and blue swapped.
	a->flip.shown_has_surface = al && al->surface && is_bgra(al->info.format);
	InterlockedExchangePointer((PVOID volatile *)&a->flip.shown, al);
	a->flip.flips++;
	return STATUS_SUCCESS;
}

// An allocation about to be freed must not be copied from any more.
void ruxnwddm_flip_forget(RUXNWDDM_ADAPTER *a, RUXNWDDM_ALLOCATION *al)
{
	LARGE_INTEGER wait;

	if (!a->flip.running)
		return;
	InterlockedCompareExchangePointer((PVOID volatile *)&a->flip.shown, NULL, al);
	wait.QuadPart = -TICKS_PER_MS;
	while (a->flip.copying == al)
		KeDelayExecutionThread(KernelMode, FALSE, &wait);
}

NTSTATUS ruxnwddm_control_interrupt(RUXNWDDM_ADAPTER *a, DXGK_INTERRUPT_TYPE type, BOOLEAN enable)
{
	// Only the vertical blank is ours to give; other kinds are accepted and
	// never raised.
	if (type == DXGK_INTERRUPT_CRTC_VSYNC)
		a->flip.vsync_enabled = enable;
	return STATUS_SUCCESS;
}

// Always just past the blank: the timer is the only clock there is.
NTSTATUS ruxnwddm_get_scan_line(RUXNWDDM_ADAPTER *a, DXGKARG_GETSCANLINE *args)
{
	UNREFERENCED_PARAMETER(a);
	args->InVerticalBlank = FALSE;
	args->ScanLine = 0;
	return STATUS_SUCCESS;
}

// The adapter's power, as the graphics kernel sets it. Off, nothing is
// shown (run rn24b: a take tried while the device was off counted as a
// failure, and the gate that stops showing after one kept the screen black
// after the resume). Back on, it may be a new device, with nothing taken
// and nothing shown: the failures of the device before are recorded and
// the gate opens again. That is not a refused command sent again: the
// device that refused, if any, is gone.
void ruxnwddm_flip_power(RUXNWDDM_ADAPTER *a, BOOLEAN on)
{
	if (!on) {
		a->flip.powered = FALSE;
		return;
	}
	if (a->flip.failures) {
		ruxnwddm_record_ulong64(L"FlipFailuresBeforePowerUp", a->flip.failures);
		a->flip.failures_before_power_up += a->flip.failures;
	}
	a->flip.failures = 0;
	a->flip.swapped = NULL;
	a->flip.powered = TRUE;
}
