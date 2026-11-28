// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Buffer objects: memory a program maps and the host reads and writes,
// ported from pvg_bo.c of the Linux driver.
//
// Linux makes a buffer object of separate device pages from the page
// allocator. Here it is made of separate 16 KiB blocks, each aligned to
// 16 KiB, which the memory manager hands out as one MDL
// (MmAllocatePagesForMdlEx with contiguous chunks of one device page). The
// task's table names each device page on its own, as Linux's does, so the
// device never needs the blocks to lie together; the harness scene
// indices-across draws from a buffer whose pages lie apart and out of order.
// A buffer object was one contiguous block up to 0.3.57, and a guest whose
// memory had been in use for a while ran out of contiguous 4 MiB blocks
// long before it ran out of memory (rn88).
//
// The memory is mapped into the process that made it, at its making, with
// an MDL. A user mapping must be undone in its own process, so it is undone
// when the handle is cleaned up, which Windows does in the process that
// closes the last handle: the opener, since only the opener may use the
// handle. Should the last handle be closed in another process (a copy made
// with DuplicateHandle), the mapping cannot be undone from here, and the
// memory is left allocated rather than freed under a live mapping.

#include "pvg.h"

#define POOL_TAG_BO	'bxur'

// Windows pages in one device page.
#define WINDOWS_PAGES_A_PAGE	(RUXNWDDM_PAGE_SIZE / PAGE_SIZE)
// A task's table has 31 bits for a page's number, bit 31 being the flag
// that the host may write the page, so its memory lies below this.
#define HIGHEST_TASK_ADDRESS	((1ULL << (31 + RUXNWDDM_PAGE_SHIFT)) - 1)

// The device page number of a buffer object's page.
ULONG pvg_bo_page(const struct pvg_bo *bo, ULONG page)
{
	const PFN_NUMBER *pfns = MmGetMdlPfnArray(bo->mdl);

	return (ULONG)(pfns[(SIZE_T)page * WINDOWS_PAGES_A_PAGE] / WINDOWS_PAGES_A_PAGE);
}

// Each device page must start on a 16 KiB boundary and hold four Windows
// pages in a row below the task's limit, which the flags ask for; this
// checks what came back rather than trust it.
static BOOLEAN pages_fit(PMDL mdl, ULONG64 bytes)
{
	const PFN_NUMBER *pfns = MmGetMdlPfnArray(mdl);
	SIZE_T count = (SIZE_T)(bytes / PAGE_SIZE), i;

	if (MmGetMdlByteCount(mdl) != bytes)
		return FALSE;
	for (i = 0; i < count; i++) {
		if (i % WINDOWS_PAGES_A_PAGE == 0 ? pfns[i] % WINDOWS_PAGES_A_PAGE != 0
						  : pfns[i] != pfns[i - 1] + 1)
			return FALSE;
		if (((ULONG64)pfns[i] << PAGE_SHIFT) > HIGHEST_TASK_ADDRESS)
			return FALSE;
	}
	return TRUE;
}

static void free_pages(PMDL mdl)
{
	MmFreePagesFromMdl(mdl);
	ExFreePool(mdl);
}

// Zeroed memory in device pages and the MDL that describes it, not yet
// mapped. bytes is a whole number of device pages.
NTSTATUS pvg_bo_alloc(RUXNWDDM_PVG *pvg, ULONG64 bytes, struct pvg_bo **out)
{
	PHYSICAL_ADDRESS lowest, highest, chunk;
	struct pvg_bo *bo;
	PMDL mdl;

	*out = NULL;
	bo = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*bo), POOL_TAG_BO);
	if (!bo)
		return STATUS_INSUFFICIENT_RESOURCES;

	lowest.QuadPart = 0;
	highest.QuadPart = (LONGLONG)HIGHEST_TASK_ADDRESS;
	chunk.QuadPart = RUXNWDDM_PAGE_SIZE;
	mdl = MmAllocatePagesForMdlEx(lowest, highest, chunk, (SIZE_T)bytes, MmCached,
				      MM_ALLOCATE_REQUIRE_CONTIGUOUS_CHUNKS |
				      MM_ALLOCATE_FULLY_REQUIRED);
	if (mdl && !pages_fit(mdl, bytes)) {
		free_pages(mdl);
		mdl = NULL;
	}
	if (!mdl) {
		ExFreePoolWithTag(bo, POOL_TAG_BO);
		return STATUS_INSUFFICIENT_RESOURCES;
	}
	bo->mdl = mdl;
	bo->bytes = bytes;

	InterlockedIncrement64(&pvg->tasks.bos);
	InterlockedAdd64(&pvg->tasks.bo_bytes, (LONG64)bytes);
	*out = bo;
	return STATUS_SUCCESS;
}

// Maps the memory into the current process, which must be the opener.
// Called in the caller's context only.
NTSTATUS pvg_bo_map(struct pvg_bo *bo)
{
	PVOID user = NULL;

	// A user mapping that fails raises an exception rather than return.
	__try {
		user = MmMapLockedPagesSpecifyCache(bo->mdl, UserMode, MmCached, NULL, FALSE,
						    NormalPagePriority | MdlMappingNoExecute);
	} __except (EXCEPTION_EXECUTE_HANDLER) {
		user = NULL;
	}
	if (!user)
		return STATUS_INSUFFICIENT_RESOURCES;
	bo->user = user;
	return STATUS_SUCCESS;
}

// Undoes the mapping if this is the process it is in, then frees the
// memory unless the caller says the device may still use it or the mapping
// could not be undone.
void pvg_bo_release(RUXNWDDM_CLIENT *client, struct pvg_bo *bo, BOOLEAN keep_memory)
{
	RUXNWDDM_PVG *pvg = client->pvg;

	if (bo->user) {
		if (PsGetCurrentProcess() == client->process) {
			MmUnmapLockedPages(bo->user, bo->mdl);
			bo->user = NULL;
			InterlockedIncrement64(&pvg->tasks.unmapped);
		} else {
			keep_memory = TRUE;
		}
	}
	if (keep_memory) {
		InterlockedIncrement64(&pvg->tasks.bos_kept);
		return;
	}
	free_pages(bo->mdl);
	InterlockedAdd64(&pvg->tasks.bo_bytes, -(LONG64)bo->bytes);
	InterlockedIncrement64(&pvg->tasks.bos_freed);
	ExFreePoolWithTag(bo, POOL_TAG_BO);
}
