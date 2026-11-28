// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// DriverEntry and the table of DDIs the graphics kernel calls. Each DDI here
// is a thin entry that finds the adapter and hands the call to the file that
// does the work: adapter.c (start, power, queries), vidpn.c (modes),
// present.c (pixels), interrupt.c and escape.c. The driver registers either
// as display-only (milestone 1) or as a full render and display driver
// (milestone 2, whose own entries are in driver_full.c), as the INF says.
//
// The table's shape follows viogpudo's driver.cpp in utmapp/viogpu-wddm
// (BSD-3-Clause, Copyright (C) 2019-2022 Red Hat, Inc.); the code is ours.

#include "pvg.h"

#define POOL_TAG	'dwxR'

static DXGKDDI_ADD_DEVICE add_device;
static DXGKDDI_START_DEVICE start_device;
static DXGKDDI_STOP_DEVICE stop_device;
static DXGKDDI_RESET_DEVICE reset_device;
static DXGKDDI_REMOVE_DEVICE remove_device;
static DXGKDDI_DISPATCH_IO_REQUEST dispatch_io_request;
static DXGKDDI_INTERRUPT_ROUTINE interrupt_routine;
static DXGKDDI_DPC_ROUTINE dpc_routine;
static DXGKDDI_QUERY_CHILD_RELATIONS query_child_relations;
static DXGKDDI_QUERY_CHILD_STATUS query_child_status;
static DXGKDDI_QUERY_DEVICE_DESCRIPTOR query_device_descriptor;
static DXGKDDI_SET_POWER_STATE set_power_state;
static DXGKDDI_UNLOAD unload;
static DXGKDDI_QUERY_INTERFACE query_interface;
static DXGKDDI_QUERYADAPTERINFO query_adapter_info;
static DXGKDDI_SETPOINTERPOSITION set_pointer_position;
static DXGKDDI_SETPOINTERSHAPE set_pointer_shape;
static DXGKDDI_ESCAPE escape;
static DXGKDDI_ISSUPPORTEDVIDPN is_supported_vidpn;
static DXGKDDI_RECOMMENDFUNCTIONALVIDPN recommend_functional_vidpn;
static DXGKDDI_ENUMVIDPNCOFUNCMODALITY enum_cofunc_modality;
static DXGKDDI_SETVIDPNSOURCEVISIBILITY set_source_visibility;
static DXGKDDI_COMMITVIDPN commit_vidpn;
static DXGKDDI_UPDATEACTIVEVIDPNPRESENTPATH update_active_path;
static DXGKDDI_RECOMMENDMONITORMODES recommend_monitor_modes;
static DXGKDDI_QUERYVIDPNHWCAPABILITY query_vidpn_hw_capability;
static DXGKDDI_PRESENTDISPLAYONLY present_display_only;
static DXGKDDI_STOP_DEVICE_AND_RELEASE_POST_DISPLAY_OWNERSHIP release_post_display;
static DXGKDDI_SYSTEM_DISPLAY_ENABLE system_display_enable;
static DXGKDDI_SYSTEM_DISPLAY_WRITE system_display_write;

DRIVER_INITIALIZE DriverEntry;

// Which model this load runs: FALSE for display-only, TRUE for full.
static BOOLEAN full_driver;

// The entries both models share, by the same names in both tables.
#define FILL_COMMON(init)							\
	do {								\
		(init).DxgkDdiAddDevice = add_device;			\
		(init).DxgkDdiStartDevice = start_device;		\
		(init).DxgkDdiStopDevice = stop_device;			\
		(init).DxgkDdiResetDevice = reset_device;		\
		(init).DxgkDdiRemoveDevice = remove_device;		\
		(init).DxgkDdiDispatchIoRequest = dispatch_io_request;	\
		(init).DxgkDdiInterruptRoutine = interrupt_routine;	\
		(init).DxgkDdiDpcRoutine = dpc_routine;			\
		(init).DxgkDdiQueryChildRelations = query_child_relations; \
		(init).DxgkDdiQueryChildStatus = query_child_status;	\
		(init).DxgkDdiQueryDeviceDescriptor = query_device_descriptor; \
		(init).DxgkDdiSetPowerState = set_power_state;		\
		(init).DxgkDdiUnload = unload;				\
		(init).DxgkDdiQueryInterface = query_interface;		\
		(init).DxgkDdiQueryAdapterInfo = query_adapter_info;	\
		(init).DxgkDdiSetPointerPosition = set_pointer_position; \
		(init).DxgkDdiSetPointerShape = set_pointer_shape;	\
		(init).DxgkDdiEscape = escape;				\
		(init).DxgkDdiIsSupportedVidPn = is_supported_vidpn;	\
		(init).DxgkDdiRecommendFunctionalVidPn = recommend_functional_vidpn; \
		(init).DxgkDdiEnumVidPnCofuncModality = enum_cofunc_modality; \
		(init).DxgkDdiSetVidPnSourceVisibility = set_source_visibility; \
		(init).DxgkDdiCommitVidPn = commit_vidpn;		\
		(init).DxgkDdiUpdateActiveVidPnPresentPath = update_active_path; \
		(init).DxgkDdiRecommendMonitorModes = recommend_monitor_modes; \
		(init).DxgkDdiQueryVidPnHWCapability = query_vidpn_hw_capability; \
		(init).DxgkDdiStopDeviceAndReleasePostDisplayOwnership = release_post_display; \
		(init).DxgkDdiSystemDisplayEnable = system_display_enable; \
		(init).DxgkDdiSystemDisplayWrite = system_display_write; \
	} while (0)

static NTSTATUS initialize_display_only(PDRIVER_OBJECT driver, PUNICODE_STRING registry_path)
{
	KMDDOD_INITIALIZATION_DATA init;

	RtlZeroMemory(&init, sizeof(init));
	// The display-only model came with WDDM 1.2 (Windows 8), and every
	// DDI below is in that version's table.
	init.Version = DXGKDDI_INTERFACE_VERSION_WIN8;
	FILL_COMMON(init);
	init.DxgkDdiPresentDisplayOnly = present_display_only;
	return DxgkInitializeDisplayOnlyDriver(driver, registry_path, &init);
}

static NTSTATUS initialize_full(PDRIVER_OBJECT driver, PUNICODE_STRING registry_path)
{
	DRIVER_INITIALIZATION_DATA init;

	RtlZeroMemory(&init, sizeof(init));
	FILL_COMMON(init);
	ruxnwddm_fill_full(&init);
	return DxgkInitialize(driver, registry_path, &init);
}

NTSTATUS DriverEntry(PDRIVER_OBJECT driver, PUNICODE_STRING registry_path)
{
	NTSTATUS status;

	status = ruxnwddm_record_init(registry_path);
	if (!NT_SUCCESS(status))
		return status;

	// The INF sets FullDriver under Parameters; without it, display-only.
	full_driver = ruxnwddm_record_read(L"FullDriver") == 1;
	ruxnwddm_record_ulong(L"Model", full_driver ? 2 : 1);
	if (full_driver)
		status = initialize_full(driver, registry_path);
	else
		status = initialize_display_only(driver, registry_path);
	ruxnwddm_record_ulong(L"InitializeStatus", (ULONG)status);
	if (!NT_SUCCESS(status))
		ruxnwddm_record_free();
	return status;
}

static void unload(void)
{
	ruxnwddm_record_free();
}

static NTSTATUS add_device(PDEVICE_OBJECT pdo, PVOID *context)
{
	RUXNWDDM_ADAPTER *a;

	if (!pdo || !context)
		return STATUS_INVALID_PARAMETER;
	a = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*a), POOL_TAG);
	if (!a)
		return STATUS_NO_MEMORY;
	a->pdo = pdo;
	a->adapter_power = PowerDeviceD0;
	a->monitor_power = PowerDeviceD0;
	a->source_visible = TRUE;
	a->full = full_driver;
	ruxnwddm_ring_init(a);
	ruxnwddm_blt_init(a);
	ruxnwddm_alloc_list_init(a);
	if (a->full) {
		a->pvg = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*a->pvg), POOL_TAG);
		if (!a->pvg) {
			ExFreePoolWithTag(a, POOL_TAG);
			return STATUS_NO_MEMORY;
		}
		pvg_init(a->pvg, a);
	}
	*context = a;
	ruxnwddm_record_count(L"AddDeviceCount");
	return STATUS_SUCCESS;
}

static NTSTATUS remove_device(PVOID context)
{
	RUXNWDDM_ADAPTER *a = context;

	if (a) {
		ruxnwddm_ring_free(a);
		// The display's client goes with the adapter; every program's
		// client went with its kernel device.
		if (a->pvg && a->pvg->display_client)
			pvg_client_close(a->pvg->display_client);
		if (a->pvg)
			ExFreePoolWithTag(a->pvg, POOL_TAG);
		ExFreePoolWithTag(a, POOL_TAG);
	}
	return STATUS_SUCCESS;
}

static NTSTATUS start_device(PVOID context, PDXGK_START_INFO start_info,
			     PDXGKRNL_INTERFACE dxgk, PULONG views, PULONG children)
{
	return ruxnwddm_start(context, start_info, dxgk, views, children);
}

static NTSTATUS stop_device(PVOID context)
{
	return ruxnwddm_stop(context);
}

// Called at bugcheck time to put the display in a known state. There is
// nothing to put back: the early framebuffer is on and stays on.
static void reset_device(PVOID context)
{
	UNREFERENCED_PARAMETER(context);
}

static NTSTATUS dispatch_io_request(PVOID context, ULONG source, PVIDEO_REQUEST_PACKET packet)
{
	UNREFERENCED_PARAMETER(context);
	UNREFERENCED_PARAMETER(source);
	UNREFERENCED_PARAMETER(packet);
	return STATUS_NOT_SUPPORTED;
}

static BOOLEAN interrupt_routine(PVOID context, ULONG message)
{
	return ruxnwddm_interrupt(context, message);
}

static void dpc_routine(PVOID context)
{
	ruxnwddm_dpc(context);
}

static NTSTATUS query_child_relations(PVOID context, PDXGK_CHILD_DESCRIPTOR children, ULONG size)
{
	return ruxnwddm_note(L"FailChildRelations",
			     ruxnwddm_query_child_relations(context, children, size));
}

static NTSTATUS query_child_status(PVOID context, PDXGK_CHILD_STATUS status, BOOLEAN non_destructive)
{
	UNREFERENCED_PARAMETER(non_destructive);
	return ruxnwddm_note(L"FailChildStatus", ruxnwddm_query_child_status(context, status));
}

// No EDID: the monitor's modes come from recommend_monitor_modes instead.
static NTSTATUS query_device_descriptor(PVOID context, ULONG child, PDXGK_DEVICE_DESCRIPTOR descriptor)
{
	UNREFERENCED_PARAMETER(context);
	UNREFERENCED_PARAMETER(child);
	UNREFERENCED_PARAMETER(descriptor);
	return STATUS_GRAPHICS_CHILD_DESCRIPTOR_NOT_SUPPORTED;
}

static NTSTATUS set_power_state(PVOID context, ULONG uid, DEVICE_POWER_STATE state, POWER_ACTION action)
{
	return ruxnwddm_set_power(context, uid, state, action);
}

static NTSTATUS query_interface(PVOID context, PQUERY_INTERFACE query)
{
	UNREFERENCED_PARAMETER(context);
	UNREFERENCED_PARAMETER(query);
	return STATUS_NOT_SUPPORTED;
}

static NTSTATUS query_adapter_info(const HANDLE context, const DXGKARG_QUERYADAPTERINFO *args)
{
	RUXNWDDM_ADAPTER *a = (RUXNWDDM_ADAPTER *)context;

	NTSTATUS status;

	ruxnwddm_record_ulong(L"LastQueryType", (ULONG)args->Type);
	if (a->full)
		status = ruxnwddm_note(L"FailQueryAdapterInfo", ruxnwddm_query_adapter_info_full(a, args));
	else
		status = ruxnwddm_note(L"FailQueryAdapterInfo", ruxnwddm_query_adapter_info(a, args));
	// Item B's probe: each query's type, sizes and answer in the lab log.
	ruxnwddm_klog_event(" ddi QueryAdapterInfo type,in,out", (ULONG)args->Type, args->InputDataSize,
			    args->OutputDataSize, status);
	return status;
}

// The cursor is drawn into the desktop by Windows: no pointer caps are
// reported, so these are not expected to be called.
static NTSTATUS set_pointer_position(const HANDLE context, const DXGKARG_SETPOINTERPOSITION *args)
{
	UNREFERENCED_PARAMETER(context);
	UNREFERENCED_PARAMETER(args);
	return STATUS_NOT_SUPPORTED;
}

static NTSTATUS set_pointer_shape(const HANDLE context, const DXGKARG_SETPOINTERSHAPE *args)
{
	UNREFERENCED_PARAMETER(context);
	UNREFERENCED_PARAMETER(args);
	return STATUS_NOT_SUPPORTED;
}

static NTSTATUS escape(const HANDLE context, const DXGKARG_ESCAPE *args)
{
	NTSTATUS status = ruxnwddm_escape((RUXNWDDM_ADAPTER *)context, args);

	// Item B's probe: escapes that fail (ours succeed by the thousand).
	if (!NT_SUCCESS(status))
		ruxnwddm_klog_event(" ddi Escape failed size,flags", args->PrivateDriverDataSize, args->Flags.Value, 0,
				    status);
	return status;
}

static NTSTATUS is_supported_vidpn(const HANDLE context, DXGKARG_ISSUPPORTEDVIDPN *args)
{
	NTSTATUS status = ruxnwddm_note(L"FailIsSupportedVidPn", ruxnwddm_is_supported_vidpn((RUXNWDDM_ADAPTER *)context, args));

	ruxnwddm_klog_event(" ddi IsSupportedVidPn", 0, 0, 0, status);
	return status;
}

// Windows asks this only when it has no configuration of its own; with one
// source and one target it builds the obvious one itself.
static NTSTATUS recommend_functional_vidpn(const HANDLE context,
					   const DXGKARG_RECOMMENDFUNCTIONALVIDPN *const args)
{
	UNREFERENCED_PARAMETER(context);
	UNREFERENCED_PARAMETER(args);
	return STATUS_GRAPHICS_NO_RECOMMENDED_FUNCTIONAL_VIDPN;
}

static NTSTATUS enum_cofunc_modality(const HANDLE context,
				     const DXGKARG_ENUMVIDPNCOFUNCMODALITY *const args)
{
	NTSTATUS status = ruxnwddm_note(L"FailCofuncModality", ruxnwddm_enum_cofunc_modality((RUXNWDDM_ADAPTER *)context, args));

	ruxnwddm_klog_event(" ddi EnumCofuncModality", 0, 0, 0, status);
	return status;
}

static NTSTATUS set_source_visibility(const HANDLE context, const DXGKARG_SETVIDPNSOURCEVISIBILITY *args)
{
	RUXNWDDM_ADAPTER *a = (RUXNWDDM_ADAPTER *)context;

	if (args->VidPnSourceId != RUXNWDDM_SOURCE_ID && args->VidPnSourceId != D3DDDI_ID_ALL)
		return STATUS_INVALID_PARAMETER;
	a->source_visible = args->Visible;
	ruxnwddm_klog_event(" ddi SetVidPnSourceVisibility source,visible", args->VidPnSourceId, args->Visible, 0, 0);
	if (!args->Visible)
		ruxnwddm_blank(a);
	return STATUS_SUCCESS;
}

static NTSTATUS commit_vidpn(const HANDLE context, const DXGKARG_COMMITVIDPN *const args)
{
	NTSTATUS status = ruxnwddm_note(L"FailCommitVidPn", ruxnwddm_commit_vidpn((RUXNWDDM_ADAPTER *)context, args));

	ruxnwddm_klog_event(" ddi CommitVidPn", 0, 0, 0, status);
	return status;
}

// Rotation and scaling stay identity, so a changed path needs nothing.
static NTSTATUS update_active_path(const HANDLE context,
				   const DXGKARG_UPDATEACTIVEVIDPNPRESENTPATH *const args)
{
	UNREFERENCED_PARAMETER(context);
	UNREFERENCED_PARAMETER(args);
	return STATUS_SUCCESS;
}

static NTSTATUS recommend_monitor_modes(const HANDLE context,
					const DXGKARG_RECOMMENDMONITORMODES *const args)
{
	NTSTATUS status = ruxnwddm_note(L"FailMonitorModes", ruxnwddm_recommend_monitor_modes((RUXNWDDM_ADAPTER *)context, args));

	ruxnwddm_klog_event(" ddi RecommendMonitorModes", 0, 0, 0, status);
	return status;
}

// The driver neither rotates, scales, clones nor converts colours.
static NTSTATUS query_vidpn_hw_capability(const HANDLE context, DXGKARG_QUERYVIDPNHWCAPABILITY *args)
{
	UNREFERENCED_PARAMETER(context);
	RtlZeroMemory(&args->VidPnHWCaps, sizeof(args->VidPnHWCaps));
	ruxnwddm_klog_event(" ddi QueryVidPnHWCapability source,target", args->SourceId, args->TargetId, 0, 0);
	return STATUS_SUCCESS;
}

static NTSTATUS present_display_only(const HANDLE context, const DXGKARG_PRESENT_DISPLAYONLY *args)
{
	return ruxnwddm_note(L"FailPresent", ruxnwddm_present((RUXNWDDM_ADAPTER *)context, args));
}

static NTSTATUS release_post_display(PVOID context, D3DDDI_VIDEO_PRESENT_TARGET_ID target,
				     PDXGK_DISPLAY_INFORMATION info)
{
	return ruxnwddm_release_post_display(context, target, info);
}

static NTSTATUS system_display_enable(PVOID context, D3DDDI_VIDEO_PRESENT_TARGET_ID target,
				      PDXGKARG_SYSTEM_DISPLAY_ENABLE_FLAGS flags, UINT *width,
				      UINT *height, D3DDDIFORMAT *format)
{
	UNREFERENCED_PARAMETER(target);
	UNREFERENCED_PARAMETER(flags);
	return ruxnwddm_system_display_enable(context, width, height, format);
}

static void system_display_write(PVOID context, PVOID source, UINT width, UINT height,
				 UINT stride, UINT x, UINT y)
{
	ruxnwddm_system_display_write(context, source, width, height, stride, x, y);
}
