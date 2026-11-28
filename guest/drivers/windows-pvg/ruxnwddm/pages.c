// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Memory for the device, as ruxnpvg's pages.c gets it. The device reads
// guest memory by its physical address (the host maps the guest's RAM as a
// whole and knows nothing of an IOMMU), so this memory comes straight from
// the memory manager, physically contiguous and aligned to the device's
// 16 KiB pages, not through a DMA adapter.

#include "ruxnwddm.h"

// The device's page numbers are 32-bit, so its memory must lie below this.
#define HIGHEST_DEVICE_ADDRESS	((1ULL << (32 + RUXNWDDM_PAGE_SHIFT)) - 1)
// A task's page table has 31 bits for a page's number, bit 31 being the
// flag that the host may write the page, so a task's memory lies below this.
#define HIGHEST_TASK_ADDRESS	((1ULL << (31 + RUXNWDDM_PAGE_SHIFT)) - 1)
// The mapper names a page by its number shifted left by two, with bit 0 for
// valid, so its pages lie below this.
#define HIGHEST_MAPPER_ADDRESS	((1ULL << (30 + RUXNWDDM_PAGE_SHIFT)) - 1)

// Windows hands out 4 KiB pages and the device wants 16 KiB ones. Asking
// for one device page less a Windows page more than needed leaves room to
// move the start up to the next 16 KiB boundary.
static NTSTATUS alloc_below(RUXNWDDM_PAGES *block, SIZE_T bytes, ULONG64 highest_address)
{
	SIZE_T slack = RUXNWDDM_PAGE_SIZE - PAGE_SIZE;
	PHYSICAL_ADDRESS lowest, highest, boundary, phys;
	ULONG64 aligned;
	PVOID allocation;

	lowest.QuadPart = 0;
	highest.QuadPart = (LONGLONG)highest_address;
	boundary.QuadPart = 0;

	allocation = MmAllocateContiguousMemorySpecifyCache(bytes + slack, lowest, highest,
							    boundary, MmCached);
	if (!allocation)
		return STATUS_INSUFFICIENT_RESOURCES;

	phys = MmGetPhysicalAddress(allocation);
	aligned = ((ULONG64)phys.QuadPart + RUXNWDDM_PAGE_SIZE - 1) &
		  ~((ULONG64)RUXNWDDM_PAGE_SIZE - 1);

	block->allocation = allocation;
	block->va = (PUCHAR)allocation + (aligned - (ULONG64)phys.QuadPart);
	block->phys.QuadPart = (LONGLONG)aligned;
	block->bytes = bytes;
	RtlZeroMemory(block->va, bytes);
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_alloc_pages(RUXNWDDM_PAGES *block, SIZE_T bytes)
{
	return alloc_below(block, bytes, HIGHEST_DEVICE_ADDRESS);
}

// For a task's own pages. bytes is a whole number of device pages.
// Buffer objects are made of separate device pages instead (pvg_bo.c).
NTSTATUS ruxnwddm_alloc_task_pages(RUXNWDDM_PAGES *block, SIZE_T bytes)
{
	return alloc_below(block, bytes, HIGHEST_TASK_ADDRESS);
}

// For the mapper's ring, table and descriptors, and for surfaces.
NTSTATUS ruxnwddm_alloc_mapper_pages(RUXNWDDM_PAGES *block, SIZE_T bytes)
{
	return alloc_below(block, bytes, HIGHEST_MAPPER_ADDRESS);
}

void ruxnwddm_free_pages(RUXNWDDM_PAGES *block)
{
	if (block->allocation)
		MmFreeContiguousMemory(block->allocation);
	RtlZeroMemory(block, sizeof(*block));
}

ULONG ruxnwddm_page_number(const RUXNWDDM_PAGES *block)
{
	return (ULONG)((ULONG64)block->phys.QuadPart >> RUXNWDDM_PAGE_SHIFT);
}
