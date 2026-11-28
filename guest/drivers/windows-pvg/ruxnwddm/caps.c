// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// What the full (render and display) driver tells the graphics kernel about
// itself, in answer to DxgkDdiQueryAdapterInfo: the driver caps, one
// aperture segment of system memory, a GPU MMU whose page tables are
// bookkeeping only, one engine, and the private block the user-mode driver
// reads.
//
// Why bookkeeping only: the device does not read memory through WDDM's GPU
// virtual addresses. It has its own tasks, each with its own page table that
// ruxnpvg's task code fills. So the GPU addresses Windows hands out are kept
// and never used, as viogpu3d does for virtio-gpu.
//
// The answers, and the comments on why each one is needed by build 26100
// and later, follow viogpu3d's QueryAdapterInfo in utmapp/viogpu-wddm
// (viogpu_adapter.cpp: BSD-3-Clause, Copyright (C) 2019-2020 Red Hat,
// Inc.); the code is ours.

#include "ruxnwddm.h"

// The GPU MMU's geometry: four levels of 4 KiB tables of 512 eight-byte
// entries, 48-bit addresses. The common shape, and nothing reads it.
#define VA_BITS			48
#define PT_LEVELS		4
#define PT_INDEX_BITS		9
#define PT_BYTES		4096

// The aperture segment: GPU addresses for it are a range Windows uses as
// names only. Its pages are guest memory Windows hands over in an MDL when
// it maps an allocation (see paging.c).
#define APERTURE_BASE		0xC0000000ULL
#define APERTURE_BYTES		(1024ULL * 1024 * 1024)
#define PAGING_BUFFER_BYTES	(10 * PAGE_SIZE)

static void fill_caps(RUXNWDDM_ADAPTER *a, DXGK_DRIVERCAPS *caps, UINT size)
{
	UNREFERENCED_PARAMETER(a);

	// The structure grows with the interface version, and Windows passes
	// the one for the version this driver registered: zero what it gave.
	RtlZeroMemory(caps, size);
	caps->WDDMVersion = DXGKDDI_WDDMv2_2;
	caps->HighestAcceptableAddress.QuadPart = -1;
	caps->MemoryManagementCaps.VirtualAddressingSupported = 1;
	caps->MemoryManagementCaps.GpuMmuSupported = 1;
	caps->MemoryManagementCaps.SectionBackedPrimary = 1;
	caps->PreemptionCaps.GraphicsPreemptionGranularity = D3DKMDT_GRAPHICS_PREEMPTION_NONE;
	caps->PreemptionCaps.ComputePreemptionGranularity = D3DKMDT_COMPUTE_PREEMPTION_NONE;
	// Windows 26100 refuses a WDDM 2 render adapter without MMIO flips;
	// flips arrive as SetVidPnSourceAddress, see flip.c.
	caps->FlipCaps.FlipOnVSyncMmIo = 1;
	caps->MaxQueuedFlipOnVSync = 1;
	caps->SchedulingCaps.MultiEngineAware = 1;
	caps->SchedulingCaps.PreemptionAware = 1;
	caps->GpuEngineTopology.NbAsymetricProcessingNodes = 1;
	// Means "implements StopDeviceAndReleasePostDisplayOwnership".
	caps->SupportNonVGA = 1;
}

// Paging buffers live in system memory (segment 0). Both versions of the
// segment answer have these three fields.
#define FILL_PAGING(out)						\
	do {								\
		(out)->PagingBufferSegmentId = 0;			\
		(out)->PagingBufferSize = PAGING_BUFFER_BYTES;		\
		(out)->PagingBufferPrivateDataSize = 0;			\
	} while (0)

// Both descriptor versions start with the same fields.
#define FILL_APERTURE(d)						\
	do {								\
		(d)->BaseAddress.QuadPart = APERTURE_BASE;		\
		(d)->Size = APERTURE_BYTES;				\
		(d)->CommitLimit = APERTURE_BYTES;			\
		(d)->Flags.Aperture = 1;				\
		(d)->Flags.CacheCoherent = 1;				\
		(d)->Flags.CpuVisible = 0;				\
		(d)->Flags.DirectFlip = 1;				\
	} while (0)

static void fill_aperture(DXGK_SEGMENTDESCRIPTOR4 *d)
{
	FILL_APERTURE(d);
}

// The older form of the same answer, one call with a plain array.
static NTSTATUS query_segments3(const DXGKARG_QUERYADAPTERINFO *args)
{
	DXGK_QUERYSEGMENTOUT3 *out = args->pOutputData;

	if (args->OutputDataSize < sizeof(*out))
		return STATUS_BUFFER_TOO_SMALL;
	out->NbSegment = 1;
	if (out->pSegmentDescriptor) {
		RtlZeroMemory(out->pSegmentDescriptor, sizeof(*out->pSegmentDescriptor));
		FILL_PAGING(out);
		FILL_APERTURE(out->pSegmentDescriptor);
	}
	return STATUS_SUCCESS;
}

// The two-call contract: first the count only, then the descriptors in
// Windows' own stride, which must not be second-guessed.
static NTSTATUS query_segments(const DXGKARG_QUERYADAPTERINFO *args)
{
	DXGK_QUERYSEGMENTOUT4 *out = args->pOutputData;

	if (args->OutputDataSize < sizeof(*out))
		return STATUS_BUFFER_TOO_SMALL;
	if (out->NbSegment == 0 || !out->pSegmentDescriptor) {
		out->NbSegment = 1;
		return STATUS_SUCCESS;
	}
	if (out->SegmentDescriptorStride < sizeof(DXGK_SEGMENTDESCRIPTOR4))
		return STATUS_INVALID_PARAMETER;
	out->NbSegment = 1;
	FILL_PAGING(out);
	RtlZeroMemory(out->pSegmentDescriptor, out->SegmentDescriptorStride);
	fill_aperture((DXGK_SEGMENTDESCRIPTOR4 *)out->pSegmentDescriptor);
	return STATUS_SUCCESS;
}

static NTSTATUS query_gpummu(const DXGKARG_QUERYADAPTERINFO *args)
{
	DXGK_GPUMMUCAPS *caps = args->pOutputData;

	if (args->OutputDataSize < sizeof(*caps))
		return STATUS_BUFFER_TOO_SMALL;
	RtlZeroMemory(caps, sizeof(*caps));
	// Windows writes the tables through CPU pointers and says when.
	caps->ExplicitPageTableInvalidation = 1;
	caps->CacheCoherentMemorySupported = 1;
	caps->ZeroInPteSupported = 1;
	caps->PageTableUpdateMode = DXGK_PAGETABLEUPDATE_CPU_VIRTUAL;
	caps->VirtualAddressBitCount = VA_BITS;
	caps->PageTableLevelCount = PT_LEVELS;
	return STATUS_SUCCESS;
}

static NTSTATUS query_level(const DXGKARG_QUERYADAPTERINFO *args)
{
	const DXGK_QUERYPAGETABLELEVELDESCIN *in = args->pInputData;
	DXGK_PAGE_TABLE_LEVEL_DESC *desc = args->pOutputData;

	if (args->InputDataSize < sizeof(*in) || args->OutputDataSize < sizeof(*desc))
		return STATUS_BUFFER_TOO_SMALL;
	if (in->LevelIndex >= PT_LEVELS)
		return STATUS_INVALID_PARAMETER;
	RtlZeroMemory(desc, sizeof(*desc));
	desc->PageTableIndexBitCount = PT_INDEX_BITS;
	desc->PageTableSegmentId = 0;
	desc->PagingProcessPageTableSegmentId = 0;
	desc->PageTableSizeInBytes = PT_BYTES;
	desc->PageTableAlignmentInBytes = PT_BYTES;
	return STATUS_SUCCESS;
}

static NTSTATUS query_physical(RUXNWDDM_ADAPTER *a, const DXGKARG_QUERYADAPTERINFO *args)
{
	DXGK_PHYSICALADAPTERCAPS *caps = args->pOutputData;

	if (args->InputDataSize < sizeof(DXGK_QUERYPHYSICALADAPTERCAPSIN) ||
	    args->OutputDataSize < RTL_SIZEOF_THROUGH_FIELD(DXGK_PHYSICALADAPTERCAPS, Flags))
		return STATUS_BUFFER_TOO_SMALL;
	RtlZeroMemory(caps, args->OutputDataSize);
	caps->NumExecutionNodes = 1;
	caps->PagingNodeIndex = 0;
	// The graphics kernel's own handle for this adapter, nothing else.
	caps->DxgkPhysicalAdapterHandle = a->dxgk.DeviceHandle;
	caps->Flags.GpuMmuSupported = 1;
	return STATUS_SUCCESS;
}

// The block the user-mode driver reads with its adapter's query: enough to
// know it is talking to this kernel driver and which version.
static NTSTATUS query_umd_private(const DXGKARG_QUERYADAPTERINFO *args)
{
	struct ruxnwddm_umd_adapter_info *info = args->pOutputData;

	if (args->OutputDataSize < sizeof(*info))
		return STATUS_BUFFER_TOO_SMALL;
	RtlZeroMemory(info, sizeof(*info));
	info->magic = RUXNWDDM_ESCAPE_MAGIC;
	info->version = RUXNWDDM_DRIVER_VERSION;
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_query_adapter_info_full(RUXNWDDM_ADAPTER *a, const DXGKARG_QUERYADAPTERINFO *args)
{
	switch (args->Type) {
	case DXGKQAITYPE_DRIVERCAPS:
		if (args->OutputDataSize < RTL_SIZEOF_THROUGH_FIELD(DXGK_DRIVERCAPS, SupportNonVGA))
			return STATUS_BUFFER_TOO_SMALL;
		fill_caps(a, args->pOutputData, args->OutputDataSize);
		return STATUS_SUCCESS;
	case DXGKQAITYPE_QUERYSEGMENT3:
		return query_segments3(args);
	case DXGKQAITYPE_QUERYSEGMENT4:
		return query_segments(args);
	case DXGKQAITYPE_GPUMMUCAPS:
		return query_gpummu(args);
	case DXGKQAITYPE_PAGETABLELEVELDESC:
		return query_level(args);
	case DXGKQAITYPE_PHYSICALADAPTERCAPS:
		return query_physical(a, args);
	case DXGKQAITYPE_UMDRIVERPRIVATE:
		return query_umd_private(args);
	case DXGKQAITYPE_HISTORYBUFFERPRECISION: {
		DXGKARG_HISTORYBUFFERPRECISION *p = args->pOutputData;

		if (args->OutputDataSize < sizeof(*p))
			return STATUS_BUFFER_TOO_SMALL;
		RtlZeroMemory(p, sizeof(*p));
		p->PrecisionBits = 64;
		return STATUS_SUCCESS;
	}
	case DXGKQAITYPE_64BITONLYCAPS:
	case DXGKQAITYPE_DISPLAY_DRIVERCAPS_EXTENSION:
		// All zero: not 64-bit only, no display extensions.
		if (args->OutputDataSize < sizeof(UINT))
			return STATUS_BUFFER_TOO_SMALL;
		RtlZeroMemory(args->pOutputData, args->OutputDataSize);
		return STATUS_SUCCESS;
	default:
		return STATUS_NOT_SUPPORTED;
	}
}
