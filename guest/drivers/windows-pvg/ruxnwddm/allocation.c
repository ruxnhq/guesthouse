// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Allocations of the full driver: the memory objects Windows' video memory
// manager places and pages for the user-mode driver and for itself.
//
// Each allocation is described by a struct ruxnwddm_alloc_info (see
// ruxnwddm_umd.h), written by the user-mode driver, or by this file for the
// standard allocations Windows makes on its own (the shared primary, shadow
// and staging surfaces of the desktop). Every allocation lives in the one
// aperture segment, so its pages are ordinary guest memory that Windows
// hands over in an MDL when it maps it (paging.c).
//
// An allocation made with RUXNWDDM_ALLOC_SURFACE lies over a device surface
// the user-mode driver made in its own task and exported: its pixels are the
// surface's, drawn by the GPU, and this driver's display client holds the
// surface by its token for as long as the allocation lives, so that a flip
// to it can show it with command 8 (flip.c). Its WDDM pages are bookkeeping
// only. Every other allocation keeps nothing on the device.
//
// The placement fields follow viogpu3d's DxgkCreateAllocation in
// utmapp/viogpu-wddm (BSD-2-Clause, Copyright 2019-2025 Red Hat, Inc. and/or
// its affiliates); the code is ours.

#include "pvg.h"

// DXGI_FORMAT numbers of the one-byte formats; the kernel has no dxgiformat.h.
#define FORMAT_R8_UNORM	61
#define FORMAT_A8_UNORM	65

#define POOL_TAG		'awxR'
#define APERTURE_SEGMENT_SET	0x1	// segment 1, the only one
#define BYTES_PER_PIXEL		4

static RUXNWDDM_ALLOCATION *alloc_new(const struct ruxnwddm_alloc_info *info)
{
	RUXNWDDM_ALLOCATION *al = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*al), POOL_TAG);

	if (al)
		al->info = *info;
	return al;
}

// The description in the creation's private data, from the allocation's own
// or the whole creation's block, whichever is large enough.
static const struct ruxnwddm_alloc_info *find_info(const DXGKARG_CREATEALLOCATION *args,
						   const DXGK_ALLOCATIONINFO *ai)
{
	const struct ruxnwddm_alloc_info *info = NULL;

	if (ai->pPrivateDriverData && ai->PrivateDriverDataSize >= sizeof(*info))
		info = ai->pPrivateDriverData;
	else if (args->pPrivateDriverData && args->PrivateDriverDataSize >= sizeof(*info))
		info = args->pPrivateDriverData;
	if (!info || info->magic != RUXNWDDM_UMD_MAGIC || !info->size)
		return NULL;
	return info;
}

static void place(DXGK_ALLOCATIONINFO *ai, const struct ruxnwddm_alloc_info *info)
{
	ai->Alignment = 0;
	ai->Size = (SIZE_T)info->size;
	ai->PitchAlignedSize = 0;
	ai->HintedBank.Value = 0;
	ai->PreferredSegment.Value = 0;
	ai->PreferredSegment.SegmentId0 = 1;
	ai->SupportedReadSegmentSet = APERTURE_SEGMENT_SET;
	ai->SupportedWriteSegmentSet = APERTURE_SEGMENT_SET;
	ai->EvictionSegmentSet = APERTURE_SEGMENT_SET;
	ai->PhysicalAdapterIndex = 0;
	ai->AllocationPriority = D3DDDI_ALLOCATIONPRIORITY_NORMAL;
	ai->pAllocationUsageHint = NULL;
	ai->Flags.Value = 0;
	ai->Flags.CpuVisible = 1;
	// Write-back pages for what the CPU reads; a primary may not be cached.
	if (!(info->flags & RUXNWDDM_ALLOC_PRIMARY))
		ai->Flags.Cached = 1;
}

// Takes the display client's hold on the surface the allocation names, if
// it names one of the same shape. Without it, a flip to the allocation
// falls back to the stepping-stone copy of its pages.
static void hold_surface(RUXNWDDM_ADAPTER *a, RUXNWDDM_ALLOCATION *al)
{
	RUXNWDDM_CLIENT *display = a->pvg ? a->pvg->display_client : NULL;
	struct pvg_surface *surface = NULL;
	NTSTATUS status = STATUS_NOT_FOUND;
	BOOLEAN one_byte;

	al->maker_process = (ULONG)(ULONG_PTR)PsGetCurrentProcessId();
	if (!(al->info.flags & RUXNWDDM_ALLOC_SURFACE))
		return;
	// The hold also keeps a shared surface alive for as long as the
	// allocation, after its maker lets its texture go (run rn16: two
	// one-byte surfaces left unheld were gone when the compositor opened
	// them). A texture of one byte a pixel lies over a surface a quarter
	// as wide and is never shown, so only its width is checked.
	one_byte = al->info.format == FORMAT_A8_UNORM || al->info.format == FORMAT_R8_UNORM;
	if (display) {
		ruxnwddm_lock(&display->lock);
		status = pvg_surface_import(display, al->info.surface_token, &surface);
		if (NT_SUCCESS(status) && one_byte &&
		    (surface->width != RUXNWDDM_SURFACE_WIDTH_ONE_BYTE(al->info.width) ||
		     surface->height != al->info.height)) {
			pvg_surface_release(display, surface);
			status = STATUS_INVALID_PARAMETER;
		} else if (NT_SUCCESS(status) && !one_byte &&
			   (surface->width != RUXNWDDM_SURFACE_WIDTH(al->info.width) ||
			    surface->height != al->info.height ||
			    surface->pitch != al->info.pitch)) {
			pvg_surface_release(display, surface);
			status = STATUS_INVALID_PARAMETER;
		}
		ruxnwddm_unlock(&display->lock);
	}
	if (!NT_SUCCESS(status)) {
		InterlockedIncrement64(&a->counts.surface_imports_failed);
		ruxnwddm_record_ulong(L"SurfaceImportFailed", (ULONG)status);
		return;
	}
	al->surface = surface;
	InterlockedIncrement64(&a->counts.surface_allocations);
}

static void let_surface_go(RUXNWDDM_ADAPTER *a, RUXNWDDM_ALLOCATION *al)
{
	RUXNWDDM_CLIENT *display = a->pvg->display_client;

	if (!al->surface)
		return;
	ruxnwddm_lock(&display->lock);
	pvg_surface_release(display, al->surface);
	ruxnwddm_unlock(&display->lock);
	al->surface = NULL;
}

NTSTATUS ruxnwddm_create_allocation(RUXNWDDM_ADAPTER *a, DXGKARG_CREATEALLOCATION *args)
{
	UINT i;

	for (i = 0; i < args->NumAllocations; i++) {
		DXGK_ALLOCATIONINFO *ai = &args->pAllocationInfo[i];
		const struct ruxnwddm_alloc_info *info = find_info(args, ai);
		RUXNWDDM_ALLOCATION *al;

		if (!info)
			goto undo;
		al = alloc_new(info);
		if (!al)
			goto undo;
		ai->hAllocation = al;
		place(ai, info);
		hold_surface(a, al);
		ruxnwddm_alloc_listed(a, al);
		InterlockedIncrement64(&a->counts.allocations);
		if (info->flags & RUXNWDDM_ALLOC_PRIMARY)
			InterlockedIncrement64(&a->counts.primaries);
	}
	// A resource handle is wanted back; the driver keeps nothing for it but
	// hands out a unique, non-zero number.
	if (args->Flags.Resource && !args->hResource)
		args->hResource = (HANDLE)(ULONG_PTR)InterlockedIncrement64(&a->counts.resources);
	return STATUS_SUCCESS;

undo:
	while (i--) {
		ruxnwddm_alloc_unlisted(a, args->pAllocationInfo[i].hAllocation);
		let_surface_go(a, args->pAllocationInfo[i].hAllocation);
		ExFreePoolWithTag(args->pAllocationInfo[i].hAllocation, POOL_TAG);
		args->pAllocationInfo[i].hAllocation = NULL;
	}
	return STATUS_INVALID_PARAMETER;
}

NTSTATUS ruxnwddm_destroy_allocation(RUXNWDDM_ADAPTER *a, const DXGKARG_DESTROYALLOCATION *args)
{
	UINT i;

	for (i = 0; i < args->NumAllocations; i++) {
		RUXNWDDM_ALLOCATION *al = args->pAllocationList[i];

		if (!al)
			continue;
		ruxnwddm_alloc_unlisted(a, al);
		ruxnwddm_flip_forget(a, al);
		let_surface_go(a, al);
		ruxnwddm_free_pages_list(al);
		ExFreePoolWithTag(al, POOL_TAG);
		InterlockedIncrement64(&a->counts.allocations_freed);
	}
	return STATUS_SUCCESS;
}

// The DXGI formats a primary may have, as the graphics kernel names them.
static D3DDDIFORMAT ddi_format(unsigned int dxgi)
{
	switch (dxgi) {
	case 88:	// DXGI_FORMAT_B8G8R8X8_UNORM
	case 93:	// DXGI_FORMAT_B8G8R8X8_UNORM_SRGB
		return D3DDDIFMT_X8R8G8B8;
	case 28:	// DXGI_FORMAT_R8G8B8A8_UNORM
	case 29:	// DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
		return D3DDDIFMT_A8B8G8R8;
	case 24:	// DXGI_FORMAT_R10G10B10A2_UNORM
		return D3DDDIFMT_A2B10G10R10;
	case 10:	// DXGI_FORMAT_R16G16B16A16_FLOAT
		return D3DDDIFMT_A16B16G16R16F;
	default:	// DXGI_FORMAT_B8G8R8A8_UNORM and its sRGB form
		return D3DDDIFMT_A8R8G8B8;
	}
}

// What a primary is, for the graphics kernel to compare with the mode
// before it flips to it. Run rn3: with a refresh rate of 60000/1000 where
// the VidPN has 60/1, and no samples where one is meant, every flip came
// back STATUS_GRAPHICS_PRESENT_MODE_CHANGED; viogpu3d gives the VidPN's own
// rate and one sample.
NTSTATUS ruxnwddm_describe_allocation(RUXNWDDM_ADAPTER *a, DXGKARG_DESCRIBEALLOCATION *args)
{
	const RUXNWDDM_ALLOCATION *al = args->hAllocation;

	UNREFERENCED_PARAMETER(a);
	if (!al)
		return STATUS_INVALID_PARAMETER;
	args->Width = al->info.width;
	args->Height = al->info.height;
	args->Format = ddi_format(al->info.format);
	args->MultisampleMethod.NumSamples = 1;
	args->MultisampleMethod.NumQualityLevels = 0;
	args->RefreshRate.Numerator = RUXNWDDM_REFRESH_HZ;
	args->RefreshRate.Denominator = 1;
	args->PrivateDriverFormatAttribute = 0;
	return STATUS_SUCCESS;
}

// Windows' own allocations for the desktop: a primary the size of the mode,
// and shadow and staging surfaces the CPU draws into and reads.
NTSTATUS ruxnwddm_standard_allocation(RUXNWDDM_ADAPTER *a,
				      DXGKARG_GETSTANDARDALLOCATIONDRIVERDATA *args)
{
	struct ruxnwddm_alloc_info *info = args->pAllocationPrivateDriverData;
	UINT width, height;

	UNREFERENCED_PARAMETER(a);
	// First call: sizes only.
	if (!args->pAllocationPrivateDriverData) {
		args->AllocationPrivateDriverDataSize = sizeof(*info);
		args->ResourcePrivateDriverDataSize = 0;
		return STATUS_SUCCESS;
	}
	RtlZeroMemory(info, sizeof(*info));
	info->magic = RUXNWDDM_UMD_MAGIC;
	info->kind = RUXNWDDM_ALLOC_TEXTURE;
	info->format = 87;	// DXGI_FORMAT_B8G8R8A8_UNORM

	switch (args->StandardAllocationType) {
	case D3DKMDT_STANDARDALLOCATION_SHAREDPRIMARYSURFACE:
		width = args->pCreateSharedPrimarySurfaceData->Width;
		height = args->pCreateSharedPrimarySurfaceData->Height;
		info->flags = RUXNWDDM_ALLOC_PRIMARY | RUXNWDDM_ALLOC_SHARED;
		break;
	case D3DKMDT_STANDARDALLOCATION_SHADOWSURFACE:
		width = args->pCreateShadowSurfaceData->Width;
		height = args->pCreateShadowSurfaceData->Height;
		args->pCreateShadowSurfaceData->Pitch = width * BYTES_PER_PIXEL;
		info->flags = RUXNWDDM_ALLOC_CPU_READ;
		break;
	case D3DKMDT_STANDARDALLOCATION_STAGINGSURFACE:
		width = args->pCreateStagingSurfaceData->Width;
		height = args->pCreateStagingSurfaceData->Height;
		args->pCreateStagingSurfaceData->Pitch = width * BYTES_PER_PIXEL;
		info->flags = RUXNWDDM_ALLOC_CPU_READ;
		break;
	default:
		return STATUS_NOT_SUPPORTED;
	}
	info->width = width;
	info->height = height;
	info->pitch = width * BYTES_PER_PIXEL;
	info->size = (unsigned long long)info->pitch * height;
	return STATUS_SUCCESS;
}

// A device opens an allocation (its own at creation, or another process's
// shared one). The private data replayed here is the creation's, so each
// open gets a small copy of its own; nothing needs the original.
NTSTATUS ruxnwddm_open_allocation(RUXNWDDM_DEVICE *dev, const DXGKARG_OPENALLOCATION *args)
{
	UINT i;

	for (i = 0; i < args->NumAllocations; i++) {
		DXGK_OPENALLOCATIONINFO *oi = &args->pOpenAllocation[i];
		const struct ruxnwddm_alloc_info *info = oi->pPrivateDriverData;
		RUXNWDDM_ALLOCATION *copy;

		if (!info || oi->PrivateDriverDataSize < sizeof(*info) ||
		    info->magic != RUXNWDDM_UMD_MAGIC)
			goto undo;
		copy = alloc_new(info);
		if (!copy)
			goto undo;
		copy->opened_by = dev;
		ruxnwddm_alloc_listed(dev->adapter, copy);
		oi->hDeviceSpecificAllocation = copy;
	}
	return STATUS_SUCCESS;

undo:
	while (i--) {
		ruxnwddm_alloc_unlisted(dev->adapter, args->pOpenAllocation[i].hDeviceSpecificAllocation);
		ExFreePoolWithTag(args->pOpenAllocation[i].hDeviceSpecificAllocation, POOL_TAG);
		args->pOpenAllocation[i].hDeviceSpecificAllocation = NULL;
	}
	return STATUS_INVALID_PARAMETER;
}

NTSTATUS ruxnwddm_close_allocation(RUXNWDDM_ADAPTER *a, const DXGKARG_CLOSEALLOCATION *args)
{
	UINT i;

	for (i = 0; i < args->NumAllocations; i++)
		if (args->pOpenHandleList[i]) {
			ruxnwddm_alloc_unlisted(a, args->pOpenHandleList[i]);
			ExFreePoolWithTag(args->pOpenHandleList[i], POOL_TAG);
		}
	return STATUS_SUCCESS;
}
