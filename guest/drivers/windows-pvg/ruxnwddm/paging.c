// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Paging of the full driver. The video memory manager moves allocations in
// and out of the aperture segment and keeps each process's GPU page tables
// by asking the driver to build paging buffers. The device reads none of
// it (see caps.c), so nearly every operation is accepted and done; three
// are not empty:
//
// - a leaf page-table update for an allocation names the guest pages behind
//   it; for a primary they are kept, so the scanout can find its pixels
//   (flip.c);
// - small leaf updates are also kept as a map from GPU virtual address to
//   guest page, so that
// - a monitored fence the GPU is asked to signal can be written by the CPU.
//   The work it waits for has already completed by then (render.c completes
//   every submission at once), so the write is never early.
//
// The operations and their reasons follow viogpu3d's BuildPagingBuffer in
// utmapp/viogpu-wddm (driver.cpp: BSD-3-Clause, Copyright (C) 2019-2020 Red
// Hat, Inc.); the code is ours.

#include "ruxnwddm.h"

#define POOL_TAG		'pwxR'
#define ROOT_PAGE_TABLE_BYTES	4096
// Only small updates map fence storage; larger ones would only push those
// out of the map.
#define MOST_ENTRIES_KEPT	16

static void va_map_set(RUXNWDDM_ADAPTER *a, HANDLE process, ULONG64 va, ULONG64 phys)
{
	RUXNWDDM_VA_MAP *m = &a->va_map;
	KIRQL irql;
	ULONG i, slot = RUXNWDDM_VA_MAP_SIZE;

	KeAcquireSpinLock(&m->lock, &irql);
	for (i = 0; i < RUXNWDDM_VA_MAP_SIZE; i++) {
		if (m->entry[i].process == process && m->entry[i].va == va) {
			slot = i;
			break;
		}
		if (!m->entry[i].process && slot == RUXNWDDM_VA_MAP_SIZE)
			slot = i;
	}
	if (slot == RUXNWDDM_VA_MAP_SIZE) {
		// Full: take the next slot round, oldest first.
		slot = m->next++ % RUXNWDDM_VA_MAP_SIZE;
	}
	m->entry[slot].process = process;
	m->entry[slot].va = va;
	m->entry[slot].phys = phys;
	KeReleaseSpinLock(&m->lock, irql);
}

static void va_map_clear(RUXNWDDM_ADAPTER *a, HANDLE process, ULONG64 va)
{
	RUXNWDDM_VA_MAP *m = &a->va_map;
	KIRQL irql;
	ULONG i;

	KeAcquireSpinLock(&m->lock, &irql);
	for (i = 0; i < RUXNWDDM_VA_MAP_SIZE; i++)
		if (m->entry[i].process == process && m->entry[i].va == va)
			RtlZeroMemory(&m->entry[i], sizeof(m->entry[i]));
	KeReleaseSpinLock(&m->lock, irql);
}

static BOOLEAN va_map_find(RUXNWDDM_ADAPTER *a, ULONG64 va, ULONG64 *phys)
{
	RUXNWDDM_VA_MAP *m = &a->va_map;
	ULONG64 page = va & ~((ULONG64)PAGE_SIZE - 1);
	BOOLEAN found = FALSE;
	KIRQL irql;
	ULONG i;

	KeAcquireSpinLock(&m->lock, &irql);
	for (i = 0; i < RUXNWDDM_VA_MAP_SIZE; i++) {
		if (m->entry[i].process && m->entry[i].va == page) {
			*phys = m->entry[i].phys + (va - page);
			found = TRUE;
			break;
		}
	}
	KeReleaseSpinLock(&m->lock, irql);
	return found;
}

// Keeps the guest page behind each page of a primary, by its place in the
// allocation.
static void keep_primary_pages(RUXNWDDM_ALLOCATION *al, const DXGK_BUILDPAGINGBUFFER_UPDATEPAGETABLE *u)
{
	ULONG64 first = u->AllocationOffsetInBytes / PAGE_SIZE;
	UINT i;

	if (!(al->info.flags & RUXNWDDM_ALLOC_PRIMARY))
		return;
	if (!al->pages) {
		al->page_count = (ULONG)((al->info.size + PAGE_SIZE - 1) / PAGE_SIZE);
		al->pages = ExAllocatePool2(POOL_FLAG_NON_PAGED, al->page_count * sizeof(ULONG64),
					    POOL_TAG);
		if (!al->pages)
			return;
	}
	for (i = 0; i < u->NumPageTableEntries && first + i < al->page_count; i++) {
		const DXGK_PTE *p = &u->pPageTableEntries[u->Flags.Repeat ? 0 : i];

		al->pages[first + i] = (p->Valid && p->Segment == 0 && !p->Zero) ?
			(ULONG64)p->PageAddress << PAGE_SHIFT : 0;
	}
}

static void update_page_table(RUXNWDDM_ADAPTER *a, const DXGK_BUILDPAGINGBUFFER_UPDATEPAGETABLE *u)
{
	ULONG64 va_base = (ULONG64)u->FirstPteVirtualAddress & ~((ULONG64)PAGE_SIZE - 1);
	BOOLEAN repeat = u->Flags.Repeat != 0;
	UINT i;

	if (u->PageTableLevel != 0 || !u->pPageTableEntries || u->Flags.Use64KBPages)
		return;
	if (u->hAllocation)
		keep_primary_pages(u->hAllocation, u);
	for (i = 0; i < u->NumPageTableEntries; i++) {
		const DXGK_PTE *p = &u->pPageTableEntries[repeat ? 0 : i];
		ULONG64 va = va_base + ((ULONG64)i << PAGE_SHIFT);

		// Only system memory (segment 0) has physical page numbers.
		if (u->NumPageTableEntries <= MOST_ENTRIES_KEPT && p->Valid && !p->Zero &&
		    !p->LargePage && p->Segment == 0)
			va_map_set(a, u->hProcess, va, (ULONG64)p->PageAddress << PAGE_SHIFT);
		else
			va_map_clear(a, u->hProcess, va);
	}
}

// Writes a monitored fence's new value through a short mapping of its page.
static void signal_fence(RUXNWDDM_ADAPTER *a, const DXGK_BUILDPAGINGBUFFER_SIGNALMONITOREDFENCE *s)
{
	PHYSICAL_ADDRESS pa;
	ULONG64 phys;
	PUCHAR page;

	if (!va_map_find(a, (ULONG64)s->MonitoredFenceGpuVa, &phys)) {
		InterlockedIncrement64(&a->counts.fences_lost);
		return;
	}
	pa.QuadPart = (LONGLONG)(phys & ~((ULONG64)PAGE_SIZE - 1));
	page = MmMapIoSpaceEx(pa, PAGE_SIZE, PAGE_READWRITE);
	if (!page) {
		InterlockedIncrement64(&a->counts.fences_lost);
		return;
	}
	*(volatile UINT64 *)(page + (phys & (PAGE_SIZE - 1))) = s->MonitoredFenceValue;
	MmUnmapIoSpace(page, PAGE_SIZE);
	InterlockedIncrement64(&a->counts.fences_written);
}

NTSTATUS ruxnwddm_build_paging_buffer(RUXNWDDM_ADAPTER *a, DXGKARG_BUILDPAGINGBUFFER *args)
{
	InterlockedIncrement64(&a->counts.paging_ops);
	switch (args->Operation) {
	case DXGK_OPERATION_UPDATE_PAGE_TABLE:
		update_page_table(a, &args->UpdatePageTable);
		return STATUS_SUCCESS;
	case DXGK_OPERATION_SIGNAL_MONITORED_FENCE:
		signal_fence(a, &args->SignalMonitoredFence);
		return STATUS_SUCCESS;
	case DXGK_OPERATION_TRANSFER:
	case DXGK_OPERATION_FILL:
	case DXGK_OPERATION_VIRTUAL_TRANSFER:
	case DXGK_OPERATION_VIRTUAL_FILL:
		// Contents are not moved or filled: nothing on the device holds
		// them yet. Counted, so a run can say how often it was asked.
		InterlockedIncrement64(&a->counts.paging_skipped);
		return STATUS_SUCCESS;
	case DXGK_OPERATION_MAP_APERTURE_SEGMENT:
	case DXGK_OPERATION_UNMAP_APERTURE_SEGMENT:
	case DXGK_OPERATION_DISCARD_CONTENT:
	case DXGK_OPERATION_NOTIFY_RESIDENCY:
	case DXGK_OPERATION_READ_PHYSICAL:
	case DXGK_OPERATION_WRITE_PHYSICAL:
	case DXGK_OPERATION_FLUSH_TLB:
	case DXGK_OPERATION_COPY_PAGE_TABLE_ENTRIES:
	case DXGK_OPERATION_UPDATE_CONTEXT_ALLOCATION:
	case DXGK_OPERATION_INIT_CONTEXT_RESOURCE:
		return STATUS_SUCCESS;
	default:
		return STATUS_NOT_SUPPORTED;
	}
}

// One page for any root page table: four levels of 512 entries.
SIZE_T ruxnwddm_root_page_table_size(INOUT_PDXGKARG_GETROOTPAGETABLESIZE args)
{
	UNREFERENCED_PARAMETER(args);
	return ROOT_PAGE_TABLE_BYTES;
}

void ruxnwddm_free_pages_list(RUXNWDDM_ALLOCATION *al)
{
	if (al->pages)
		ExFreePoolWithTag(al->pages, POOL_TAG);
	al->pages = NULL;
	al->page_count = 0;
}
