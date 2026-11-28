// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Blt presents: a windowed swap chain of the older model presents by a
// blt from the program's back buffer into the compositor's surface for the
// window. The graphics kernel names that surface only here, in the
// present's allocation list, and the user-mode driver is not told it (run
// rn43: "first present1: destination 0, flags Blt"). This driver copies
// nothing itself: it notes, for the presenting process and its back
// buffer's surface, which surface the blt went to, and the user-mode
// driver asks for it (RUXNWDDM_ESCAPE_BLT_DEST) and copies on the device
// before its next presents (umd_blt.c). The first present of a window
// shows nothing new; every later one shows its frame.

#include "ruxnwddm.h"

void ruxnwddm_blt_init(RUXNWDDM_ADAPTER *a)
{
	KeInitializeSpinLock(&a->blt_dests.lock);
}

// From the present DDI, in the presenting process.
void ruxnwddm_blt_note(RUXNWDDM_ADAPTER *a, const RUXNWDDM_ALLOCATION *src, const RUXNWDDM_ALLOCATION *dst)
{
	RUXNWDDM_BLT_DESTS *t = &a->blt_dests;
	ULONG process = HandleToULong(PsGetCurrentProcessId());
	RUXNWDDM_BLT_DEST *d = NULL;
	KIRQL irql;
	UINT i;

	if (!src || !dst)
		return;
	// What a blt's two ends are, the first few times (run rn48: no
	// destination reached the user-mode driver, so one end has no surface).
	if (InterlockedIncrement(&t->noted) <= 4) {
		ruxnwddm_record_ulong64(L"BltSrcInfo", (ULONG64)src->info.kind << 48 | (ULONG64)src->info.flags << 32 |
							      src->info.format);
		ruxnwddm_record_ulong64(L"BltDstInfo", (ULONG64)dst->info.kind << 48 | (ULONG64)dst->info.flags << 32 |
							      dst->info.format);
		ruxnwddm_record_ulong64(L"BltDstSize", (ULONG64)dst->info.width << 32 | dst->info.height);
		ruxnwddm_record_ulong(L"BltDstMagic", dst->info.magic);
	}
	if (!(src->info.flags & RUXNWDDM_ALLOC_SURFACE) || !(dst->info.flags & RUXNWDDM_ALLOC_SURFACE))
		return;
	KeAcquireSpinLock(&t->lock, &irql);
	for (i = 0; i < RUXNWDDM_BLT_DEST_COUNT; i++)
		if (t->dest[i].process == process && t->dest[i].src_token == src->info.surface_token)
			d = &t->dest[i];
	if (!d) {
		d = &t->dest[t->next];
		t->next = (t->next + 1) % RUXNWDDM_BLT_DEST_COUNT;
		RtlZeroMemory(d, sizeof(*d));
		d->process = process;
		d->src_token = src->info.surface_token;
	}
	d->dst_token = dst->info.surface_token;
	d->width = dst->info.width;
	d->height = dst->info.height;
	d->format = dst->info.format;
	d->presents++;
	KeReleaseSpinLock(&t->lock, irql);
}

// The escape: only the asking process's own entries are answered.
NTSTATUS ruxnwddm_blt_dest(RUXNWDDM_ADAPTER *a, struct ruxnwddm_escape_blt_dest *q)
{
	RUXNWDDM_BLT_DESTS *t = &a->blt_dests;
	ULONG process = HandleToULong(PsGetCurrentProcessId());
	KIRQL irql;
	UINT i;

	q->dst_token = 0;
	q->width = q->height = q->format = 0;
	KeAcquireSpinLock(&t->lock, &irql);
	for (i = 0; i < RUXNWDDM_BLT_DEST_COUNT; i++) {
		const RUXNWDDM_BLT_DEST *d = &t->dest[i];

		if (d->process == process && d->src_token == q->src_token && q->src_token) {
			q->dst_token = d->dst_token;
			q->width = d->width;
			q->height = d->height;
			q->format = d->format;
		}
	}
	KeReleaseSpinLock(&t->lock, irql);
	return STATUS_SUCCESS;
}
