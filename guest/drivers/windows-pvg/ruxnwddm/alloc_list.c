// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The live allocations, for the lab: every allocation a device made or
// opened is on the adapter's list from its making to its end, and the
// ALLOCATIONS escape (wddmtool allocs) copies the list out. Run rn64 found
// allocations over surfaces outliving the programs that made them, each
// holding its surface's memory, until new ones could not be made; this
// shows which ones stay and who still has them open.

#include "ruxnwddm.h"
#include "ruxnwddm_escape.h"

void ruxnwddm_alloc_list_init(RUXNWDDM_ADAPTER *a)
{
	KeInitializeSpinLock(&a->allocs.lock);
	InitializeListHead(&a->allocs.live);
	a->allocs.count = 0;
}

void ruxnwddm_alloc_listed(RUXNWDDM_ADAPTER *a, RUXNWDDM_ALLOCATION *al)
{
	KIRQL irql;

	al->maker_process = (ULONG)(ULONG_PTR)PsGetCurrentProcessId();
	KeAcquireSpinLock(&a->allocs.lock, &irql);
	InsertTailList(&a->allocs.live, &al->link);
	a->allocs.count++;
	KeReleaseSpinLock(&a->allocs.lock, irql);
}

void ruxnwddm_alloc_unlisted(RUXNWDDM_ADAPTER *a, RUXNWDDM_ALLOCATION *al)
{
	KIRQL irql;

	if (!al->link.Flink)
		return;
	KeAcquireSpinLock(&a->allocs.lock, &irql);
	RemoveEntryList(&al->link);
	a->allocs.count--;
	KeReleaseSpinLock(&a->allocs.lock, irql);
	al->link.Flink = al->link.Blink = NULL;
}

// The surface's holder count is read without the mapper's lock: a number
// for a person to read, which may be a moment old. The surface itself
// cannot go while the allocation is listed, as the allocation holds it.
void ruxnwddm_alloc_list_fill(RUXNWDDM_ADAPTER *a, struct ruxnwddm_escape_allocations *out)
{
	LIST_ENTRY *link;
	KIRQL irql;
	ULONG n = 0;

	KeAcquireSpinLock(&a->allocs.lock, &irql);
	out->live = a->allocs.count;
	for (link = a->allocs.live.Flink; link != &a->allocs.live && n < RUXNWDDM_ALLOCATIONS_MOST;
	     link = link->Flink) {
		RUXNWDDM_ALLOCATION *al = CONTAINING_RECORD(link, RUXNWDDM_ALLOCATION, link);
		struct ruxnwddm_escape_allocation *e = &out->list[n++];

		e->process = al->maker_process;
		e->opened = al->opened_by != NULL;
		e->flags = al->info.flags;
		e->format = al->info.format;
		e->width = al->info.width;
		e->height = al->info.height;
		e->surface_token = al->info.surface_token;
		e->surface_holders = al->surface ? (unsigned int)pvg_surface_holders(al->surface) : 0;
		e->reserved = 0;
	}
	KeReleaseSpinLock(&a->allocs.lock, irql);
	out->returned = n;
}
