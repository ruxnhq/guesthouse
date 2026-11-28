// SPDX-License-Identifier: MIT
//
// Residency of the allocations ruxnumd.dll makes. Under WDDM 2 an
// allocation is not resident when it is made, and the graphics kernel
// refuses a present or a flip that names one that nobody asked to be
// resident; the runtime asks only for the allocations it makes itself, not
// for those AllocateCb hands back to the driver. So every allocation of
// ours is asked for once, on a paging queue of the device's own, and the
// request lasts as long as the allocation. Triton does the same
// (tritonKm.c, MIT, Copyright Turing Software LLC: its notes say why); the
// code is ours.

#include "umd.h"

// How long a queued request is waited for, at most.
#define RESIDENT_WAIT_MS	5000

static BOOL paging_queue(UMD_DEVICE *dev)
{
	D3DDDICB_CREATEPAGINGQUEUE pq;

	if (dev->paging_queue)
		return TRUE;
	if (dev->paging_queue_failed || !dev->kt->pfnCreatePagingQueueCb || !dev->kt->pfnMakeResidentCb)
		return FALSE;
	umd_zero(&pq, sizeof(pq));
	pq.Priority = D3DDDI_PAGINGQUEUE_PRIORITY_NORMAL;
	if (FAILED(dev->kt->pfnCreatePagingQueueCb(dev->runtime.handle, &pq)) || !pq.hPagingQueue) {
		dev->paging_queue_failed = TRUE;
		umd_log("CreatePagingQueue failed", 0);
		return FALSE;
	}
	dev->paging_queue = pq.hPagingQueue;
	dev->paging_fence = pq.FenceValueCPUVirtualAddress;
	return TRUE;
}

void umd_make_resident(UMD_DEVICE *dev, D3DKMT_HANDLE allocation)
{
	D3DDDI_MAKERESIDENT mr;
	ULONGLONG deadline;
	HRESULT hr;

	if (!allocation || !paging_queue(dev))
		return;
	umd_zero(&mr, sizeof(mr));
	mr.hPagingQueue = dev->paging_queue;
	mr.NumAllocations = 1;
	mr.AllocationList = &allocation;
	hr = dev->kt->pfnMakeResidentCb(dev->runtime.handle, &mr);
	if (hr == E_PENDING && dev->paging_fence) {
		// Queued: done once the paging fence reaches the value given.
		deadline = GetTickCount64() + RESIDENT_WAIT_MS;
		while (*(volatile const UINT64 *)dev->paging_fence < mr.PagingFenceValue &&
		       GetTickCount64() < deadline)
			Sleep(0);
	} else if (FAILED(hr)) {
		umd_log("MakeResident failed", (ULONG)hr);
	}
}
