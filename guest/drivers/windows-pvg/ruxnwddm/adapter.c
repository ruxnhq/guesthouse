// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The adapter: start and stop, power, and what it tells Windows about
// itself and its one monitor. At start it checks the PCI ids, finds BAR 0,
// BAR 2 and the interrupt among the resources, maps BAR 0 and reads the
// early framebuffer's bank, takes over the display from the driver before
// it (or from the firmware) and opens the framebuffer the desktop will be
// copied to. Nothing here writes to the device.
//
// The order of steps and the handling of the POST display follow viogpudo
// in utmapp/viogpu-wddm (BSD-3-Clause, Copyright (C) 2019-2022 Red Hat,
// Inc.); the code is ours.

#include "pvg.h"

// The PCI header's first bytes: vendor and device ids.
typedef struct _PCI_IDS {
	USHORT vendor;
	USHORT device;
} PCI_IDS;

static NTSTATUS check_ids(RUXNWDDM_ADAPTER *a)
{
	PCI_IDS ids = { 0 };
	ULONG read = 0;
	NTSTATUS status;

	status = a->dxgk.DxgkCbReadDeviceSpace(a->dxgk.DeviceHandle, DXGK_WHICHSPACE_CONFIG,
					       &ids, 0, sizeof(ids), &read);
	if (!NT_SUCCESS(status))
		return status;
	ruxnwddm_record_ulong(L"PciIds", ((ULONG)ids.device << 16) | ids.vendor);
	if (read != sizeof(ids) || ids.vendor != RUXNWDDM_VENDOR_ID ||
	    ids.device != RUXNWDDM_DEVICE_ID)
		return STATUS_GRAPHICS_DRIVER_MISMATCH;
	return STATUS_SUCCESS;
}

// The memory resources come in BAR order: BAR 0 first, the mapper's BAR 2
// second. The raw interrupt descriptor carries the MSI message count.
static void find_resources(RUXNWDDM_ADAPTER *a)
{
	PCM_RESOURCE_LIST list = a->device_info.TranslatedResourceList;
	PCM_PARTIAL_RESOURCE_LIST partial;
	ULONG memory_seen = 0;
	ULONG i;

	if (!list || list->Count == 0)
		return;
	partial = &list->List[0].PartialResourceList;
	for (i = 0; i < partial->Count; i++) {
		PCM_PARTIAL_RESOURCE_DESCRIPTOR d = &partial->PartialDescriptors[i];

		switch (d->Type) {
		case CmResourceTypeMemory:
			if (memory_seen == 0) {
				a->regs_phys = d->u.Memory.Start;
				a->regs_length = d->u.Memory.Length;
			} else if (memory_seen == 1) {
				a->mapper_phys = d->u.Memory.Start;
				a->mapper_length = d->u.Memory.Length;
			}
			memory_seen++;
			break;
		case CmResourceTypeInterrupt:
			if (d->Flags & CM_RESOURCE_INTERRUPT_MESSAGE)
				a->interrupt_messages++;
			break;
		default:
			break;
		}
	}
	ruxnwddm_record_ulong(L"MemoryResources", memory_seen);
	ruxnwddm_record_ulong(L"InterruptMessages", a->interrupt_messages);
	ruxnwddm_record_ulong64(L"Bar0Address", (ULONG64)a->regs_phys.QuadPart);
	ruxnwddm_record_ulong(L"Bar0Length", a->regs_length);
	ruxnwddm_record_ulong64(L"Bar2Address", (ULONG64)a->mapper_phys.QuadPart);
	ruxnwddm_record_ulong(L"Bar2Length", a->mapper_length);
}

// Reads the bank pvgfb wrote: reads only, the same ones ruxnpvg's display
// makes before it takes the screen.
static void read_efb_bank(RUXNWDDM_ADAPTER *a)
{
	a->efb_page = ruxnwddm_read(a, REG_EFB_PAGE);
	a->efb_bytes = ruxnwddm_read(a, REG_EFB_BYTES);
	a->efb_depth = ruxnwddm_read(a, REG_EFB_DEPTH);
	a->efb_mode = ruxnwddm_read(a, REG_EFB_MODE);
	ruxnwddm_record_ulong(L"EfbPage", a->efb_page);
	ruxnwddm_record_ulong(L"EfbBytes", a->efb_bytes);
	ruxnwddm_record_ulong(L"EfbDepth", a->efb_depth);
	ruxnwddm_record_ulong(L"EfbMode", a->efb_mode);
	ruxnwddm_record_ulong(L"ModeCount", ruxnwddm_read(a, REG_MODE_COUNT));
}

static void take_post_display(RUXNWDDM_ADAPTER *a)
{
	RtlZeroMemory(&a->post, sizeof(a->post));
	a->post.TargetId = D3DDDI_ID_UNINITIALIZED;
	a->post_status = STATUS_NOT_SUPPORTED;
	if (a->dxgk.DxgkCbAcquirePostDisplayOwnership)
		a->post_status = a->dxgk.DxgkCbAcquirePostDisplayOwnership(a->dxgk.DeviceHandle,
									   &a->post);
	ruxnwddm_record_ulong(L"PostStatus", (ULONG)a->post_status);
	ruxnwddm_record_ulong(L"PostWidth", a->post.Width);
	ruxnwddm_record_ulong(L"PostHeight", a->post.Height);
	ruxnwddm_record_ulong(L"PostPitch", a->post.Pitch);
	ruxnwddm_record_ulong(L"PostFormat", (ULONG)a->post.ColorFormat);
	ruxnwddm_record_ulong64(L"PostAddress", (ULONG64)a->post.PhysicAddress.QuadPart);
}

static void unmap_regs(RUXNWDDM_ADAPTER *a)
{
	if (a->regs) {
		MmUnmapIoSpace((PVOID)a->regs, a->regs_length);
		a->regs = NULL;
	}
}

NTSTATUS ruxnwddm_start(RUXNWDDM_ADAPTER *a, DXGK_START_INFO *start_info,
			DXGKRNL_INTERFACE *dxgk, ULONG *views, ULONG *children)
{
	NTSTATUS status;

	UNREFERENCED_PARAMETER(start_info);

	ruxnwddm_record_count(L"StartCount");
	// Keep no more of the interface than this driver was built against.
	RtlCopyMemory(&a->dxgk, dxgk, min(dxgk->Size, sizeof(a->dxgk)));

	status = a->dxgk.DxgkCbGetDeviceInformation(a->dxgk.DeviceHandle, &a->device_info);
	if (NT_SUCCESS(status))
		status = check_ids(a);
	if (!NT_SUCCESS(status))
		goto fail;

	find_resources(a);
	// Anything but the expected size means this is not the BAR we think it
	// is, and nothing gets read.
	if (a->regs_length != RUXNWDDM_REGS_LENGTH) {
		status = STATUS_DEVICE_CONFIGURATION_ERROR;
		goto fail;
	}
	a->regs = MmMapIoSpaceEx(a->regs_phys, a->regs_length, PAGE_READWRITE | PAGE_NOCACHE);
	if (!a->regs) {
		status = STATUS_INSUFFICIENT_RESOURCES;
		goto fail;
	}
	read_efb_bank(a);
	take_post_display(a);

	status = ruxnwddm_scanout_open(a);
	if (!NT_SUCCESS(status))
		goto fail;
	if (a->full) {
		KeInitializeSpinLock(&a->va_map.lock);
		ruxnwddm_fence_init(a);
		status = ruxnwddm_flip_start(a);
		if (NT_SUCCESS(status))
			status = pvg_start(a->pvg);
		if (!NT_SUCCESS(status))
			goto fail;
	}

	*views = RUXNWDDM_SOURCES;
	*children = RUXNWDDM_CHILDREN;
	a->started = TRUE;
	ruxnwddm_record_ulong(L"StartStatus", 0);
	return STATUS_SUCCESS;

fail:
	ruxnwddm_record_ulong(L"StartStatus", (ULONG)status);
	unmap_regs(a);
	return status;
}

NTSTATUS ruxnwddm_stop(RUXNWDDM_ADAPTER *a)
{
	a->started = FALSE;
	if (a->full) {
		ruxnwddm_flip_stop(a);
		// The screen and the tasks go while the ring still runs.
		pvg_power_down(a->pvg);
	}
	ruxnwddm_ring_stop(a);
	if (a->full)
		pvg_stop(a->pvg);
	ruxnwddm_scanout_close(a);
	unmap_regs(a);
	ruxnwddm_record_count(L"StopCount");
	return STATUS_SUCCESS;
}

// Hands the display back, for the driver that comes next (the basic display
// driver, or this one again after an update), in the shape it is in now.
NTSTATUS ruxnwddm_release_post_display(RUXNWDDM_ADAPTER *a, D3DDDI_VIDEO_PRESENT_TARGET_ID target,
				       DXGK_DISPLAY_INFORMATION *info)
{
	RtlZeroMemory(info, sizeof(*info));
	info->Width = a->scanout.width;
	info->Height = a->scanout.height;
	info->Pitch = a->scanout.pitch;
	info->ColorFormat = a->scanout.format;
	info->PhysicAddress = a->scanout.phys;
	info->TargetId = target;
	info->AcpiId = a->post.AcpiId;
	ruxnwddm_record_count(L"ReleasePostCount");
	return ruxnwddm_stop(a);
}

NTSTATUS ruxnwddm_set_power(RUXNWDDM_ADAPTER *a, ULONG uid, DEVICE_POWER_STATE state,
			    POWER_ACTION action)
{
	UNREFERENCED_PARAMETER(action);

	if (uid != RUXNWDDM_ADAPTER_UID) {
		// The monitor: off means the desktop need not be copied.
		a->monitor_power = state;
		return STATUS_SUCCESS;
	}
	if (state != PowerDeviceD0) {
		// The screen and the tasks go first, while the ring still
		// runs; then the ring goes off before the device sleeps.
		if (a->full) {
			ruxnwddm_flip_power(a, FALSE);
			pvg_power_down(a->pvg);
		}
		ruxnwddm_ring_stop(a);
	} else if (a->full && a->started) {
		pvg_power_up(a->pvg);
		ruxnwddm_flip_power(a, TRUE);
	}
	a->adapter_power = state;
	ruxnwddm_record_ulong(L"AdapterPower", (ULONG)state);
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_query_adapter_info(RUXNWDDM_ADAPTER *a, const DXGKARG_QUERYADAPTERINFO *args)
{
	DXGK_DRIVERCAPS *caps;

	UNREFERENCED_PARAMETER(a);

	ruxnwddm_record_ulong(L"LastQueryType", (ULONG)args->Type);
	ruxnwddm_record_ulong(L"LastQuerySize", args->OutputDataSize);
	if (args->Type != DXGKQAITYPE_DRIVERCAPS)
		return STATUS_NOT_SUPPORTED;
	// Windows passes the caps of the interface version the driver claims
	// (WIN8), which is shorter than this header's. Every field set below
	// lies in the first part of the structure.
	if (args->OutputDataSize < FIELD_OFFSET(DXGK_DRIVERCAPS, SupportNonVGA) + sizeof(BOOLEAN))
		return STATUS_BUFFER_TOO_SMALL;
	caps = args->pOutputData;
	RtlZeroMemory(caps, min(args->OutputDataSize, sizeof(*caps)));
	caps->WDDMVersion = DXGKDDI_WDDMv1_2;
	caps->HighestAcceptableAddress.QuadPart = -1;
	// The device is no VGA: no legacy ranges, the framebuffer is memory.
	caps->SupportNonVGA = TRUE;
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_query_child_relations(RUXNWDDM_ADAPTER *a, DXGK_CHILD_DESCRIPTOR *children,
					ULONG size)
{
	UNREFERENCED_PARAMETER(a);

	// The array has room for one more entry than there are children,
	// which stays zeroed as its end. Only our one entry is written.
	if (size < sizeof(*children) * RUXNWDDM_CHILDREN)
		return STATUS_BUFFER_TOO_SMALL;
	children[0].ChildDeviceType = TypeVideoOutput;
	// An external monitor, as viogpudo reports for a device that is not
	// the VGA one. Run rm2 showed Windows keeping an "internal" output
	// off while the basic display driver's own output was on.
	children[0].ChildCapabilities.HpdAwareness = HpdAwarenessInterruptible;
	children[0].ChildCapabilities.Type.VideoOutput.InterfaceTechnology = D3DKMDT_VOT_HD15;
	children[0].ChildCapabilities.Type.VideoOutput.MonitorOrientationAwareness = D3DKMDT_MOA_NONE;
	children[0].ChildCapabilities.Type.VideoOutput.SupportsSdtvModes = FALSE;
	children[0].AcpiUid = 0;
	children[0].ChildUid = RUXNWDDM_TARGET_ID;
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_query_child_status(RUXNWDDM_ADAPTER *a, DXGK_CHILD_STATUS *status)
{
	if (status->ChildUid != RUXNWDDM_TARGET_ID)
		return STATUS_INVALID_PARAMETER;
	switch (status->Type) {
	case StatusConnection:
		status->HotPlug.Connected = a->started;
		return STATUS_SUCCESS;
	case StatusRotation:
		status->Rotation.Angle = 0;
		return STATUS_SUCCESS;
	default:
		return STATUS_NOT_SUPPORTED;
	}
}
