// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The render side of the full driver: devices, contexts and processes, the
// submission of command buffers, and the fence of each submission.
//
// Milestone 2 runs nothing on the device. Every submission is reported done
// at once, in order, through the graphics kernel's interrupt callback, so
// Windows' scheduler sees a GPU that never falls behind. The command
// buffers themselves are only counted: the user-mode driver of milestone 2
// does not draw. Milestone 3 hands them to the ring as command 67.
//
// The DDIs that a WDDM 2 driver must have for preemption, timeout recovery,
// engine status and the GPU clock answer as for an engine that is always
// idle. The set, and why each is needed on build 26100, follows viogpu3d in
// utmapp/viogpu-wddm (driver.cpp: BSD-3-Clause, Copyright (C) 2019-2020 Red
// Hat, Inc.); the code is ours.

#include "pvg.h"

#define POOL_TAG		'cwxR'
// What a context asks of the graphics kernel for its command buffers.
#define DMA_BUFFER_BYTES	(1024 * 1024)
#define ALLOCATION_LIST_SIZE	256
#define PATCH_LIST_SIZE		256

NTSTATUS ruxnwddm_create_device(RUXNWDDM_ADAPTER *a, DXGKARG_CREATEDEVICE *args)
{
	RUXNWDDM_DEVICE *dev = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*dev), POOL_TAG);

	if (!dev)
		return STATUS_NO_MEMORY;
	dev->adapter = a;
	dev->runtime_handle = args->hDevice;
	args->hDevice = dev;
	InterlockedIncrement64(&a->counts.devices);
	return STATUS_SUCCESS;
}

// In the closing process's context when a program closes the device, and
// when the graphics kernel cleans up after a process that exits; the task
// goes with it (pvg_client.c).
NTSTATUS ruxnwddm_destroy_device(RUXNWDDM_DEVICE *dev)
{
	if (!dev)
		return STATUS_SUCCESS;
	if (dev->client)
		pvg_client_close(dev->client);
	ExFreePoolWithTag(dev, POOL_TAG);
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_create_context(RUXNWDDM_DEVICE *dev, DXGKARG_CREATECONTEXT *args)
{
	RUXNWDDM_CONTEXT *ctx = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*ctx), POOL_TAG);

	if (!ctx)
		return STATUS_NO_MEMORY;
	ctx->device = dev;
	ctx->node = args->NodeOrdinal;
	ctx->system = args->Flags.SystemContext;
	args->hContext = ctx;
	RtlZeroMemory(&args->ContextInfo, sizeof(args->ContextInfo));
	args->ContextInfo.DmaBufferSize = DMA_BUFFER_BYTES;
	// Command buffers in the aperture segment. Left at 0 (system memory),
	// build 26100 takes an older path and faults while it builds the
	// system context's buffer pool, as viogpu3d found (bugcheck 0x3B).
	args->ContextInfo.DmaBufferSegmentSet = 0x1;
	args->ContextInfo.DmaBufferPrivateDataSize = 0;
	if (args->Flags.GdiContext || args->Flags.SystemContext) {
		args->ContextInfo.AllocationListSize = DXGK_ALLOCATION_LIST_SIZE_GDICONTEXT;
		args->ContextInfo.PatchLocationListSize = DXGK_ALLOCATION_LIST_SIZE_GDICONTEXT;
	} else {
		args->ContextInfo.AllocationListSize = ALLOCATION_LIST_SIZE;
		args->ContextInfo.PatchLocationListSize = PATCH_LIST_SIZE;
	}
	if (dev && dev->adapter)
		InterlockedIncrement64(&dev->adapter->counts.contexts);
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_destroy_context(RUXNWDDM_CONTEXT *ctx)
{
	if (ctx)
		ExFreePoolWithTag(ctx, POOL_TAG);
	return STATUS_SUCCESS;
}

// A process gets nothing of its own yet; milestone 3 gives it a task.
NTSTATUS ruxnwddm_create_process(RUXNWDDM_ADAPTER *a, DXGKARG_CREATEPROCESS *args)
{
	args->hKmdProcess = args->hDxgkProcess;
	InterlockedIncrement64(&a->counts.processes);
	return STATUS_SUCCESS;
}

// Runs at the interrupt's level, under the graphics kernel's lock: says the
// newest fence submitted is done and asks for the DPC that wakes waiters.
static BOOLEAN report_done(PVOID context)
{
	RUXNWDDM_ADAPTER *a = context;
	DXGKARGCB_NOTIFY_INTERRUPT_DATA note;
	UINT fence = a->fence.pending;

	if (fence == a->fence.completed)
		return TRUE;
	RtlZeroMemory(&note, sizeof(note));
	note.InterruptType = DXGK_INTERRUPT_DMA_COMPLETED;
	note.DmaCompleted.SubmissionFenceId = fence;
	note.DmaCompleted.NodeOrdinal = 0;
	note.DmaCompleted.EngineOrdinal = 0;
	a->dxgk.DxgkCbNotifyInterrupt(a->dxgk.DeviceHandle, &note);
	a->fence.completed = fence;
	a->dxgk.DxgkCbQueueDpc(a->dxgk.DeviceHandle);
	return TRUE;
}

static BOOLEAN report_preempted(PVOID context);
static KDEFERRED_ROUTINE fence_dpc;

static void fence_dpc(PKDPC dpc, PVOID context, PVOID arg1, PVOID arg2)
{
	RUXNWDDM_ADAPTER *a = context;
	BOOLEAN ignored;

	UNREFERENCED_PARAMETER(dpc);
	UNREFERENCED_PARAMETER(arg1);
	UNREFERENCED_PARAMETER(arg2);
	a->dxgk.DxgkCbSynchronizeExecution(a->dxgk.DeviceHandle, report_done, a, 0, &ignored);
	if (InterlockedExchange(&a->fence.preempt_pending, 0))
		a->dxgk.DxgkCbSynchronizeExecution(a->dxgk.DeviceHandle, report_preempted, a, 0,
						   &ignored);
}

void ruxnwddm_fence_init(RUXNWDDM_ADAPTER *a)
{
	KeInitializeDpc(&a->fence.dpc, fence_dpc, a);
}

// A submission is reported done from a DPC, after the submit call has
// returned, as viogpu3d reports from its worker: the graphics kernel
// records a packet as submitted when the call returns. Fences only grow,
// so the newest one stands for all before it.
static void complete(RUXNWDDM_ADAPTER *a, UINT fence)
{
	a->fence.pending = fence;
	KeInsertQueueDpc(&a->fence.dpc, NULL, NULL);
}

NTSTATUS ruxnwddm_submit_virtual(RUXNWDDM_ADAPTER *a, const DXGKARG_SUBMITCOMMANDVIRTUAL *args)
{
	InterlockedIncrement64(&a->counts.submits);
	complete(a, args->SubmissionFenceId);
	return STATUS_SUCCESS;
}

// Paging buffers and kernel-built command buffers come this way.
NTSTATUS ruxnwddm_submit(RUXNWDDM_ADAPTER *a, const DXGKARG_SUBMITCOMMAND *args)
{
	InterlockedIncrement64(&a->counts.paging_submits);
	complete(a, args->SubmissionFenceId);
	return STATUS_SUCCESS;
}

// Nothing is ever running, so a preemption is acknowledged with the last
// fence done.
static BOOLEAN report_preempted(PVOID context)
{
	RUXNWDDM_ADAPTER *a = context;
	DXGKARGCB_NOTIFY_INTERRUPT_DATA note;

	RtlZeroMemory(&note, sizeof(note));
	note.InterruptType = DXGK_INTERRUPT_DMA_PREEMPTED;
	note.DmaPreempted.PreemptionFenceId = a->fence.preempt;
	note.DmaPreempted.LastCompletedFenceId = a->fence.completed;
	note.DmaPreempted.NodeOrdinal = 0;
	note.DmaPreempted.EngineOrdinal = 0;
	a->dxgk.DxgkCbNotifyInterrupt(a->dxgk.DeviceHandle, &note);
	a->dxgk.DxgkCbQueueDpc(a->dxgk.DeviceHandle);
	return TRUE;
}

// Acknowledged from the same DPC, after the call returns, for the same
// reason as completions.
NTSTATUS ruxnwddm_preempt(RUXNWDDM_ADAPTER *a, const DXGKARG_PREEMPTCOMMAND *args)
{
	a->fence.preempt = args->PreemptionFenceId;
	InterlockedExchange(&a->fence.preempt_pending, 1);
	KeInsertQueueDpc(&a->fence.dpc, NULL, NULL);
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_query_current_fence(RUXNWDDM_ADAPTER *a, DXGKARG_QUERYCURRENTFENCE *args)
{
	args->CurrentFence = a->fence.completed;
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_query_engine_status(RUXNWDDM_ADAPTER *a, DXGKARG_QUERYENGINESTATUS *args)
{
	UNREFERENCED_PARAMETER(a);
	args->EngineStatus.Responsive = 1;
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_reset_engine(RUXNWDDM_ADAPTER *a, DXGKARG_RESETENGINE *args)
{
	args->LastAbortedFenceId = a->fence.completed;
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_node_metadata(UINT node, DXGKARG_GETNODEMETADATA *args)
{
	// The high word is the physical adapter's index; there is one node.
	if (DXGKNODEMETADATA_GETNODEORDINAL(node) >= 1)
		return STATUS_INVALID_PARAMETER;
	// All of it zeroed: Windows appends the friendly name to the node's.
	RtlZeroMemory(args, sizeof(*args));
	args->EngineType = DXGK_ENGINE_TYPE_3D;
	// The one node must say what the adapter says (caps.c).
	args->GpuMmuSupported = TRUE;
	args->IoMmuSupported = FALSE;
	return STATUS_SUCCESS;
}

// The GPU's clock is the CPU's: both read from the same counter.
NTSTATUS ruxnwddm_calibrate_clock(DXGKARG_CALIBRATEGPUCLOCK *args)
{
	LARGE_INTEGER frequency;
	LARGE_INTEGER now = KeQueryPerformanceCounter(&frequency);

	args->GpuClockCounter = (UINT64)now.QuadPart;
	args->CpuClockCounter = (UINT64)now.QuadPart;
	args->GpuFrequency = (UINT64)frequency.QuadPart;
	return STATUS_SUCCESS;
}
