// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The entries only the full (render and display) driver has: allocations,
// devices, contexts and processes, paging, submission and its fences,
// preemption and timeout recovery, flips and the vertical blank. Each is a
// thin entry into allocation.c, paging.c, render.c or flip.c; the entries
// both driver models share stay in driver.c, which fills this table's
// common part and calls ruxnwddm_fill_full for the rest.
//
// The set of entries, and the interface version 2.2 (2.3 would route flips
// through a multiplane-overlay entry this driver does not have), follow
// viogpu3d in utmapp/viogpu-wddm (driver.cpp: BSD-3-Clause, Copyright (C)
// 2019-2020 Red Hat, Inc.); the code is ours.

#include "ruxnwddm.h"

static NTSTATUS create_device(const HANDLE adapter, DXGKARG_CREATEDEVICE *args)
{
	NTSTATUS status = ruxnwddm_note(L"FailCreateDevice", ruxnwddm_create_device(adapter, args));

	// Item B's probe: device creation in the lab log, before its first present.
	ruxnwddm_klog_event(" ddi CreateDevice flags", args->Flags.Value, 0, 0, status);
	return status;
}

static NTSTATUS destroy_device(const HANDLE device)
{
	return ruxnwddm_destroy_device(device);
}

static NTSTATUS create_allocation(const HANDLE adapter, DXGKARG_CREATEALLOCATION *args)
{
	return ruxnwddm_note(L"FailCreateAllocation", ruxnwddm_create_allocation(adapter, args));
}

static NTSTATUS destroy_allocation(const HANDLE adapter, const DXGKARG_DESTROYALLOCATION *args)
{
	return ruxnwddm_destroy_allocation(adapter, args);
}

static NTSTATUS describe_allocation(const HANDLE adapter, DXGKARG_DESCRIBEALLOCATION *args)
{
	return ruxnwddm_note(L"FailDescribeAllocation", ruxnwddm_describe_allocation(adapter, args));
}

static NTSTATUS standard_allocation(const HANDLE adapter, DXGKARG_GETSTANDARDALLOCATIONDRIVERDATA *args)
{
	return ruxnwddm_note(L"FailStandardAllocation", ruxnwddm_standard_allocation(adapter, args));
}

static NTSTATUS open_allocation(const HANDLE device, const DXGKARG_OPENALLOCATION *args)
{
	return ruxnwddm_note(L"FailOpenAllocation", ruxnwddm_open_allocation(device, args));
}

static NTSTATUS close_allocation(const HANDLE device, const DXGKARG_CLOSEALLOCATION *args)
{
	return ruxnwddm_close_allocation(((RUXNWDDM_DEVICE *)device)->adapter, args);
}

static NTSTATUS build_paging_buffer(const HANDLE adapter, DXGKARG_BUILDPAGINGBUFFER *args)
{
	return ruxnwddm_note(L"FailPaging", ruxnwddm_build_paging_buffer(adapter, args));
}

static NTSTATUS create_context(const HANDLE device, DXGKARG_CREATECONTEXT *args)
{
	return ruxnwddm_note(L"FailCreateContext", ruxnwddm_create_context(device, args));
}

static NTSTATUS destroy_context(const HANDLE context)
{
	return ruxnwddm_destroy_context(context);
}

static NTSTATUS create_process(PVOID adapter, DXGKARG_CREATEPROCESS *args)
{
	return ruxnwddm_create_process(adapter, args);
}

static NTSTATUS destroy_process(PVOID adapter, HANDLE process)
{
	UNREFERENCED_PARAMETER(adapter);
	UNREFERENCED_PARAMETER(process);
	return STATUS_SUCCESS;
}

// The page tables are bookkeeping (caps.c): a context's root is noted by
// nobody.
static void set_root_page_table(const HANDLE adapter, const DXGKARG_SETROOTPAGETABLE *args)
{
	UNREFERENCED_PARAMETER(adapter);
	UNREFERENCED_PARAMETER(args);
}

static SIZE_T root_page_table_size(const HANDLE adapter, DXGKARG_GETROOTPAGETABLESIZE *args)
{
	UNREFERENCED_PARAMETER(adapter);
	return ruxnwddm_root_page_table_size(args);
}

// Command buffers of the older path: nothing to translate or patch, as
// nothing in them runs yet.
static NTSTATUS render(const HANDLE context, DXGKARG_RENDER *args)
{
	UNREFERENCED_PARAMETER(context);
	UNREFERENCED_PARAMETER(args);
	return STATUS_SUCCESS;
}

// Kernel presents are counted by kind, with the flags of the first ones
// recorded, so a run shows how the compositor hands its frames over: flips
// of a primary, or copies (blts) into one. A blt's two ends are noted for
// the user-mode driver's copy (blt_present.c).
static NTSTATUS present(const HANDLE context, DXGKARG_PRESENT *args)
{
	RUXNWDDM_CONTEXT *ctx = context;
	RUXNWDDM_ADAPTER *a;
	const RUXNWDDM_ALLOCATION *src = NULL, *dst = NULL;
	LONG64 n;

	if (!ctx || !ctx->device || !(a = ctx->device->adapter))
		return STATUS_SUCCESS;
	n = InterlockedIncrement64(&a->counts.presents);
	if (args->Flags.Flip)
		InterlockedIncrement64(&a->counts.present_flips);
	if (args->Flags.Blt)
		InterlockedIncrement64(&a->counts.present_blts);
	// A WDDM 2 driver with a GPU MMU is handed pAllocationInfo, not the
	// older pAllocationList, whose entries are of another size (runs rn41
	// to rn49: no blt destination was ever found). Its element 0 is always
	// empty, the source is element 1 and the destination element 2
	// (d3dkmddi.h's DXGK_PRESENT_SOURCE_INDEX and _DESTINATION_INDEX), each
	// NULL when there is none; NumSrcAllocations and NumDstAllocations are
	// for linked adapters and are 0 here (run rn50 read element 0 and only
	// when they were not, so it found nothing).
	if (args->Flags.Blt && args->pAllocationInfo) {
		src = args->pAllocationInfo[DXGK_PRESENT_SOURCE_INDEX].hDeviceSpecificAllocation;
		dst = args->pAllocationInfo[DXGK_PRESENT_DESTINATION_INDEX].hDeviceSpecificAllocation;
		if (src && src->info.magic != RUXNWDDM_UMD_MAGIC)
			src = NULL;
		if (dst && dst->info.magic != RUXNWDDM_UMD_MAGIC)
			dst = NULL;
	}
	if (dst && (dst->info.flags & RUXNWDDM_ALLOC_PRIMARY))
		InterlockedIncrement64(&a->counts.present_to_primary);
	// What the first blts carry.
	if (args->Flags.Blt && InterlockedIncrement(&a->blt_dests.seen) <= 4) {
		ruxnwddm_record_ulong(L"BltSources", args->NumSrcAllocations);
		ruxnwddm_record_ulong(L"BltDestinations", args->NumDstAllocations);
		ruxnwddm_record_ulong(L"BltHasSrc", src != NULL);
		ruxnwddm_record_ulong(L"BltHasDst", dst != NULL);
		ruxnwddm_record_ulong(L"BltDstRects", args->SubRectCnt);
		ruxnwddm_record_ulong64(L"BltSrcRect", (ULONG64)(args->SrcRect.right - args->SrcRect.left) << 32 |
						       (ULONG)(args->SrcRect.bottom - args->SrcRect.top));
		ruxnwddm_record_ulong64(L"BltDstRect", (ULONG64)(args->DstRect.right - args->DstRect.left) << 32 |
						       (ULONG)(args->DstRect.bottom - args->DstRect.top));
	}
	// Where a blt went, for the user-mode driver's copy (blt_present.c).
	if (src && dst)
		ruxnwddm_blt_note(a, src, dst);
	if (n <= 4)
		ruxnwddm_record_ulong(n == 1 ? L"PresentFlags1" : L"PresentFlagsLater", args->Flags.Value);
	return STATUS_SUCCESS;
}

static NTSTATUS patch(const HANDLE adapter, const DXGKARG_PATCH *args)
{
	UNREFERENCED_PARAMETER(adapter);
	UNREFERENCED_PARAMETER(args);
	return STATUS_SUCCESS;
}

static NTSTATUS submit_command(const HANDLE adapter, const DXGKARG_SUBMITCOMMAND *args)
{
	return ruxnwddm_submit(adapter, args);
}

static NTSTATUS submit_command_virtual(const HANDLE adapter, const DXGKARG_SUBMITCOMMANDVIRTUAL *args)
{
	return ruxnwddm_submit_virtual(adapter, args);
}

static NTSTATUS preempt_command(const HANDLE adapter, const DXGKARG_PREEMPTCOMMAND *args)
{
	return ruxnwddm_preempt(adapter, args);
}

static NTSTATUS query_current_fence(const HANDLE adapter, DXGKARG_QUERYCURRENTFENCE *args)
{
	return ruxnwddm_query_current_fence(adapter, args);
}

// Nothing is ever stuck, so recovery has nothing to reset.
static NTSTATUS reset_from_timeout(const HANDLE adapter)
{
	ruxnwddm_record_count(L"ResetFromTimeout");
	UNREFERENCED_PARAMETER(adapter);
	return STATUS_SUCCESS;
}

static NTSTATUS restart_from_timeout(const HANDLE adapter)
{
	UNREFERENCED_PARAMETER(adapter);
	return STATUS_SUCCESS;
}

static NTSTATUS collect_debug_info(const HANDLE adapter, const DXGKARG_COLLECTDBGINFO *args)
{
	UNREFERENCED_PARAMETER(adapter);
	UNREFERENCED_PARAMETER(args);
	return STATUS_SUCCESS;
}

static NTSTATUS cancel_command(const HANDLE adapter, const DXGKARG_CANCELCOMMAND *args)
{
	UNREFERENCED_PARAMETER(adapter);
	UNREFERENCED_PARAMETER(args);
	return STATUS_SUCCESS;
}

static NTSTATUS query_engine_status(const HANDLE adapter, DXGKARG_QUERYENGINESTATUS *args)
{
	return ruxnwddm_query_engine_status(adapter, args);
}

static NTSTATUS reset_engine(const HANDLE adapter, DXGKARG_RESETENGINE *args)
{
	return ruxnwddm_reset_engine(adapter, args);
}

static NTSTATUS node_metadata(const HANDLE adapter, UINT node, DXGKARG_GETNODEMETADATA *args)
{
	UNREFERENCED_PARAMETER(adapter);
	return ruxnwddm_node_metadata(node, args);
}

static NTSTATUS calibrate_gpu_clock(const HANDLE adapter, UINT32 node, UINT32 engine,
				    DXGKARG_CALIBRATEGPUCLOCK *args)
{
	UNREFERENCED_PARAMETER(adapter);
	UNREFERENCED_PARAMETER(node);
	UNREFERENCED_PARAMETER(engine);
	return ruxnwddm_calibrate_clock(args);
}

static void set_stable_power_state(const HANDLE adapter, const DXGKARG_SETSTABLEPOWERSTATE *args)
{
	UNREFERENCED_PARAMETER(adapter);
	UNREFERENCED_PARAMETER(args);
}

static NTSTATUS set_source_address(const HANDLE adapter, const DXGKARG_SETVIDPNSOURCEADDRESS *args)
{
	return ruxnwddm_set_source_address(adapter, args);
}

static NTSTATUS control_interrupt(const HANDLE adapter, const DXGK_INTERRUPT_TYPE type, BOOLEAN enable)
{
	NTSTATUS status = ruxnwddm_control_interrupt(adapter, type, enable);

	ruxnwddm_klog_event(" ddi ControlInterrupt type,enable", type, enable, 0, status);
	return status;
}

static NTSTATUS get_scan_line(const HANDLE adapter, DXGKARG_GETSCANLINE *args)
{
	return ruxnwddm_get_scan_line(adapter, args);
}

void ruxnwddm_fill_full(DRIVER_INITIALIZATION_DATA *init)
{
	init->Version = DXGKDDI_INTERFACE_VERSION_WDDM2_2;
	init->DxgkDdiCreateDevice = create_device;
	init->DxgkDdiDestroyDevice = destroy_device;
	init->DxgkDdiCreateAllocation = create_allocation;
	init->DxgkDdiDestroyAllocation = destroy_allocation;
	init->DxgkDdiDescribeAllocation = describe_allocation;
	init->DxgkDdiGetStandardAllocationDriverData = standard_allocation;
	init->DxgkDdiOpenAllocation = open_allocation;
	init->DxgkDdiCloseAllocation = close_allocation;
	init->DxgkDdiBuildPagingBuffer = build_paging_buffer;
	init->DxgkDdiCreateContext = create_context;
	init->DxgkDdiDestroyContext = destroy_context;
	init->DxgkDdiCreateProcess = create_process;
	init->DxgkDdiDestroyProcess = destroy_process;
	init->DxgkDdiSetRootPageTable = set_root_page_table;
	init->DxgkDdiGetRootPageTableSize = root_page_table_size;
	init->DxgkDdiRender = render;
	init->DxgkDdiRenderKm = render;
	init->DxgkDdiPresent = present;
	init->DxgkDdiPatch = patch;
	init->DxgkDdiSubmitCommand = submit_command;
	init->DxgkDdiSubmitCommandVirtual = submit_command_virtual;
	init->DxgkDdiPreemptCommand = preempt_command;
	init->DxgkDdiQueryCurrentFence = query_current_fence;
	init->DxgkDdiResetFromTimeout = reset_from_timeout;
	init->DxgkDdiRestartFromTimeout = restart_from_timeout;
	init->DxgkDdiCollectDbgInfo = collect_debug_info;
	init->DxgkDdiCancelCommand = cancel_command;
	init->DxgkDdiQueryEngineStatus = query_engine_status;
	init->DxgkDdiResetEngine = reset_engine;
	init->DxgkDdiGetNodeMetadata = node_metadata;
	init->DxgkDdiCalibrateGpuClock = calibrate_gpu_clock;
	init->DxgkDdiSetStablePowerState = set_stable_power_state;
	init->DxgkDdiSetVidPnSourceAddress = set_source_address;
	init->DxgkDdiControlInterrupt = control_interrupt;
	init->DxgkDdiGetScanLine = get_scan_line;
}
