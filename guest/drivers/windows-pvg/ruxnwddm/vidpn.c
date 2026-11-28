// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The video present network for one source, one target and one mode: the
// mode of the framebuffer the device already shows (scanout.c). Windows
// proposes configurations; the driver says which are possible, fills in
// the mode sets it left open, and is told when one is committed. The
// device cannot change mode in milestone 1, so there is exactly one mode
// everywhere and nothing to set when it is committed.
//
// The steps follow viogpudo's EnumVidPnCofuncModality and CommitVidPn in
// utmapp/viogpu-wddm (BSD-3-Clause, Copyright (C) 2019-2022 Red Hat, Inc.),
// cut down to a single mode; the code is ours.

#include "ruxnwddm.h"

static BOOLEAN pivot_is_source(const DXGKARG_ENUMVIDPNCOFUNCMODALITY *args,
			       D3DDDI_VIDEO_PRESENT_SOURCE_ID source)
{
	return args->EnumPivotType == D3DKMDT_EPT_VIDPNSOURCE &&
	       args->EnumPivot.VidPnSourceId == source;
}

static BOOLEAN pivot_is_target(const DXGKARG_ENUMVIDPNCOFUNCMODALITY *args,
			       D3DDDI_VIDEO_PRESENT_TARGET_ID target)
{
	return args->EnumPivotType == D3DKMDT_EPT_VIDPNTARGET &&
	       args->EnumPivot.VidPnTargetId == target;
}

#define REFRESH_HZ	RUXNWDDM_REFRESH_HZ

// The display-only driver may leave the timings open; the full one must
// name them, as it reports a vertical blank (the graphics kernel refuses
// an unspecified frequency with STATUS_GRAPHICS_INVALID_FREQUENCY, run
// rm5). The device has no timings of its own: 60 Hz, no blanking.
static void fill_signal(RUXNWDDM_ADAPTER *a, D3DKMDT_VIDEO_SIGNAL_INFO *signal)
{
	ULONG w = a->scanout.width, h = a->scanout.height;

	signal->VideoStandard = D3DKMDT_VSS_OTHER;
	signal->TotalSize.cx = w;
	signal->TotalSize.cy = h;
	signal->ActiveSize = signal->TotalSize;
	signal->ScanLineOrdering = D3DDDI_VSSLO_PROGRESSIVE;
	if (!a->full) {
		signal->VSyncFreq.Numerator = D3DKMDT_FREQUENCY_NOTSPECIFIED;
		signal->VSyncFreq.Denominator = D3DKMDT_FREQUENCY_NOTSPECIFIED;
		signal->HSyncFreq.Numerator = D3DKMDT_FREQUENCY_NOTSPECIFIED;
		signal->HSyncFreq.Denominator = D3DKMDT_FREQUENCY_NOTSPECIFIED;
		signal->PixelRate = D3DKMDT_FREQUENCY_NOTSPECIFIED;
		return;
	}
	signal->VSyncFreq.Numerator = REFRESH_HZ;
	signal->VSyncFreq.Denominator = 1;
	signal->HSyncFreq.Numerator = REFRESH_HZ * h;
	signal->HSyncFreq.Denominator = 1;
	signal->PixelRate = (SIZE_T)REFRESH_HZ * w * h;
}

static NTSTATUS add_source_mode(RUXNWDDM_ADAPTER *a, const DXGK_VIDPNSOURCEMODESET_INTERFACE *set_if,
				D3DKMDT_HVIDPNSOURCEMODESET set)
{
	D3DKMDT_VIDPN_SOURCE_MODE *mode;
	NTSTATUS status;

	status = set_if->pfnCreateNewModeInfo(set, &mode);
	if (!NT_SUCCESS(status))
		return status;
	mode->Type = D3DKMDT_RMT_GRAPHICS;
	mode->Format.Graphics.PrimSurfSize.cx = a->scanout.width;
	mode->Format.Graphics.PrimSurfSize.cy = a->scanout.height;
	mode->Format.Graphics.VisibleRegionSize = mode->Format.Graphics.PrimSurfSize;
	mode->Format.Graphics.Stride = a->scanout.pitch;
	mode->Format.Graphics.PixelFormat = D3DDDIFMT_A8R8G8B8;
	mode->Format.Graphics.ColorBasis = D3DKMDT_CB_SCRGB;
	mode->Format.Graphics.PixelValueAccessMode = D3DKMDT_PVAM_DIRECT;
	status = set_if->pfnAddMode(set, mode);
	if (!NT_SUCCESS(status)) {
		set_if->pfnReleaseModeInfo(set, mode);
		if (status == STATUS_GRAPHICS_MODE_ALREADY_IN_MODESET)
			status = STATUS_SUCCESS;
	}
	return status;
}

static NTSTATUS add_target_mode(RUXNWDDM_ADAPTER *a, const DXGK_VIDPNTARGETMODESET_INTERFACE *set_if,
				D3DKMDT_HVIDPNTARGETMODESET set)
{
	D3DKMDT_VIDPN_TARGET_MODE *mode;
	NTSTATUS status;

	status = set_if->pfnCreateNewModeInfo(set, &mode);
	if (!NT_SUCCESS(status))
		return status;
	fill_signal(a, &mode->VideoSignalInfo);
	mode->Preference = D3DKMDT_MP_PREFERRED;
	status = set_if->pfnAddMode(set, mode);
	if (!NT_SUCCESS(status)) {
		set_if->pfnReleaseModeInfo(set, mode);
		if (status == STATUS_GRAPHICS_MODE_ALREADY_IN_MODESET)
			status = STATUS_SUCCESS;
	}
	return status;
}

// A configuration is possible if it uses only our source and target.
NTSTATUS ruxnwddm_is_supported_vidpn(RUXNWDDM_ADAPTER *a, DXGKARG_ISSUPPORTEDVIDPN *args)
{
	const DXGK_VIDPN_INTERFACE *vidpn_if;
	const DXGK_VIDPNTOPOLOGY_INTERFACE *topology_if;
	D3DKMDT_HVIDPNTOPOLOGY topology;
	SIZE_T paths = 0;
	NTSTATUS status;

	args->IsVidPnSupported = FALSE;
	if (!args->hDesiredVidPn) {
		// An empty configuration: everything off. Always possible.
		args->IsVidPnSupported = TRUE;
		return STATUS_SUCCESS;
	}
	status = a->dxgk.DxgkCbQueryVidPnInterface(args->hDesiredVidPn, DXGK_VIDPN_INTERFACE_VERSION_V1,
						   &vidpn_if);
	if (NT_SUCCESS(status))
		status = vidpn_if->pfnGetTopology(args->hDesiredVidPn, &topology, &topology_if);
	if (NT_SUCCESS(status))
		status = topology_if->pfnGetNumPaths(topology, &paths);
	if (!NT_SUCCESS(status))
		return status;
	if (paths == 0) {
		args->IsVidPnSupported = TRUE;
		return STATUS_SUCCESS;
	}
	if (paths == 1) {
		const D3DKMDT_VIDPN_PRESENT_PATH *path;

		status = topology_if->pfnAcquireFirstPathInfo(topology, &path);
		if (!NT_SUCCESS(status))
			return status;
		args->IsVidPnSupported = path->VidPnSourceId == RUXNWDDM_SOURCE_ID &&
					 path->VidPnTargetId == RUXNWDDM_TARGET_ID;
		topology_if->pfnReleasePathInfo(topology, path);
	}
	return STATUS_SUCCESS;
}

// Fills the source's mode set if it is not the pivot and has no pinned
// mode. Our one mode is the only answer either way.
static NTSTATUS cofunc_source(RUXNWDDM_ADAPTER *a, const DXGKARG_ENUMVIDPNCOFUNCMODALITY *args,
			      const DXGK_VIDPN_INTERFACE *vidpn_if,
			      D3DDDI_VIDEO_PRESENT_SOURCE_ID source)
{
	const DXGK_VIDPNSOURCEMODESET_INTERFACE *set_if;
	const D3DKMDT_VIDPN_SOURCE_MODE *pinned = NULL;
	D3DKMDT_HVIDPNSOURCEMODESET set;
	NTSTATUS status;

	if (pivot_is_source(args, source))
		return STATUS_SUCCESS;
	status = vidpn_if->pfnAcquireSourceModeSet(args->hConstrainingVidPn, source, &set, &set_if);
	if (!NT_SUCCESS(status))
		return status;
	status = set_if->pfnAcquirePinnedModeInfo(set, &pinned);
	if (NT_SUCCESS(status) && pinned) {
		set_if->pfnReleaseModeInfo(set, pinned);
		return vidpn_if->pfnReleaseSourceModeSet(args->hConstrainingVidPn, set);
	}
	vidpn_if->pfnReleaseSourceModeSet(args->hConstrainingVidPn, set);
	if (!NT_SUCCESS(status))
		return status;

	status = vidpn_if->pfnCreateNewSourceModeSet(args->hConstrainingVidPn, source, &set, &set_if);
	if (!NT_SUCCESS(status))
		return status;
	status = add_source_mode(a, set_if, set);
	if (NT_SUCCESS(status))
		status = vidpn_if->pfnAssignSourceModeSet(args->hConstrainingVidPn, source, set);
	if (!NT_SUCCESS(status))
		vidpn_if->pfnReleaseSourceModeSet(args->hConstrainingVidPn, set);
	return status;
}

static NTSTATUS cofunc_target(RUXNWDDM_ADAPTER *a, const DXGKARG_ENUMVIDPNCOFUNCMODALITY *args,
			      const DXGK_VIDPN_INTERFACE *vidpn_if,
			      D3DDDI_VIDEO_PRESENT_TARGET_ID target)
{
	const DXGK_VIDPNTARGETMODESET_INTERFACE *set_if;
	const D3DKMDT_VIDPN_TARGET_MODE *pinned = NULL;
	D3DKMDT_HVIDPNTARGETMODESET set;
	NTSTATUS status;

	if (pivot_is_target(args, target))
		return STATUS_SUCCESS;
	status = vidpn_if->pfnAcquireTargetModeSet(args->hConstrainingVidPn, target, &set, &set_if);
	if (!NT_SUCCESS(status))
		return status;
	status = set_if->pfnAcquirePinnedModeInfo(set, &pinned);
	if (NT_SUCCESS(status) && pinned) {
		set_if->pfnReleaseModeInfo(set, pinned);
		return vidpn_if->pfnReleaseTargetModeSet(args->hConstrainingVidPn, set);
	}
	vidpn_if->pfnReleaseTargetModeSet(args->hConstrainingVidPn, set);
	if (!NT_SUCCESS(status))
		return status;

	status = vidpn_if->pfnCreateNewTargetModeSet(args->hConstrainingVidPn, target, &set, &set_if);
	if (!NT_SUCCESS(status))
		return status;
	status = add_target_mode(a, set_if, set);
	if (NT_SUCCESS(status))
		status = vidpn_if->pfnAssignTargetModeSet(args->hConstrainingVidPn, target, set);
	if (!NT_SUCCESS(status))
		vidpn_if->pfnReleaseTargetModeSet(args->hConstrainingVidPn, set);
	return status;
}

// The path's scaling and rotation, where Windows left them open: identity
// only, since the driver copies pixels as they come.
static NTSTATUS cofunc_path(RUXNWDDM_ADAPTER *a, const DXGKARG_ENUMVIDPNCOFUNCMODALITY *args,
			    const DXGK_VIDPNTOPOLOGY_INTERFACE *topology_if,
			    D3DKMDT_HVIDPNTOPOLOGY topology, const D3DKMDT_VIDPN_PRESENT_PATH *path)
{
	D3DKMDT_VIDPN_PRESENT_PATH local = *path;
	BOOLEAN changed = FALSE;
	BOOLEAN pivot_path = args->EnumPivot.VidPnSourceId == path->VidPnSourceId &&
			     args->EnumPivot.VidPnTargetId == path->VidPnTargetId;

	if (!(args->EnumPivotType == D3DKMDT_EPT_SCALING && pivot_path) &&
	    path->ContentTransformation.Scaling == D3DKMDT_VPPS_UNPINNED) {
		RtlZeroMemory(&local.ContentTransformation.ScalingSupport,
			      sizeof(local.ContentTransformation.ScalingSupport));
		local.ContentTransformation.ScalingSupport.Identity = 1;
		changed = TRUE;
	}
	if (!(args->EnumPivotType == D3DKMDT_EPT_ROTATION && pivot_path) &&
	    path->ContentTransformation.Rotation == D3DKMDT_VPPR_UNPINNED) {
		RtlZeroMemory(&local.ContentTransformation.RotationSupport,
			      sizeof(local.ContentTransformation.RotationSupport));
		local.ContentTransformation.RotationSupport.Identity = 1;
		// From the path-independent rotation interface on (the full
		// driver's 2.2), a path must also say it takes no offset; without
		// it the graphics kernel stops the system inside
		// UpdatePathSupportInfo (bugcheck 0x113, argument 0x1C, runs rm7
		// and rm8, found on the crash dump's stack). viogpu3d sets it too.
		if (a->full)
			local.ContentTransformation.RotationSupport.Offset0 = 1;
		changed = TRUE;
	}
	if (!changed)
		return STATUS_SUCCESS;
	return topology_if->pfnUpdatePathSupportInfo(topology, &local);
}

NTSTATUS ruxnwddm_enum_cofunc_modality(RUXNWDDM_ADAPTER *a,
				       const DXGKARG_ENUMVIDPNCOFUNCMODALITY *args)
{
	const DXGK_VIDPN_INTERFACE *vidpn_if;
	const DXGK_VIDPNTOPOLOGY_INTERFACE *topology_if;
	const D3DKMDT_VIDPN_PRESENT_PATH *path, *next;
	D3DKMDT_HVIDPNTOPOLOGY topology;
	NTSTATUS status;

	status = a->dxgk.DxgkCbQueryVidPnInterface(args->hConstrainingVidPn,
						   DXGK_VIDPN_INTERFACE_VERSION_V1, &vidpn_if);
	if (NT_SUCCESS(status))
		status = vidpn_if->pfnGetTopology(args->hConstrainingVidPn, &topology, &topology_if);
	if (NT_SUCCESS(status))
		status = topology_if->pfnAcquireFirstPathInfo(topology, &path);
	if (status == STATUS_GRAPHICS_DATASET_IS_EMPTY)
		return STATUS_SUCCESS;
	if (!NT_SUCCESS(status))
		return status;

	while (path) {
		status = cofunc_source(a, args, vidpn_if, path->VidPnSourceId);
		if (NT_SUCCESS(status))
			status = cofunc_target(a, args, vidpn_if, path->VidPnTargetId);
		if (NT_SUCCESS(status))
			status = cofunc_path(a, args, topology_if, topology, path);
		if (!NT_SUCCESS(status)) {
			topology_if->pfnReleasePathInfo(topology, path);
			return status;
		}
		status = topology_if->pfnAcquireNextPathInfo(topology, path, &next);
		topology_if->pfnReleasePathInfo(topology, path);
		if (status == STATUS_GRAPHICS_NO_MORE_ELEMENTS_IN_DATASET)
			return STATUS_SUCCESS;
		if (!NT_SUCCESS(status))
			return status;
		path = next;
	}
	return STATUS_SUCCESS;
}

// A committed configuration either shows our source in our one mode, or
// shows nothing on it. There is no mode to set on the device.
NTSTATUS ruxnwddm_commit_vidpn(RUXNWDDM_ADAPTER *a, const DXGKARG_COMMITVIDPN *args)
{
	const DXGK_VIDPN_INTERFACE *vidpn_if;
	const DXGK_VIDPNSOURCEMODESET_INTERFACE *set_if;
	const D3DKMDT_VIDPN_SOURCE_MODE *pinned = NULL;
	D3DKMDT_HVIDPNSOURCEMODESET set;
	NTSTATUS status;

	if (args->AffectedVidPnSourceId != RUXNWDDM_SOURCE_ID &&
	    args->AffectedVidPnSourceId != D3DDDI_ID_ALL)
		return STATUS_SUCCESS;

	status = a->dxgk.DxgkCbQueryVidPnInterface(args->hFunctionalVidPn,
						   DXGK_VIDPN_INTERFACE_VERSION_V1, &vidpn_if);
	if (!NT_SUCCESS(status))
		return status;
	status = vidpn_if->pfnAcquireSourceModeSet(args->hFunctionalVidPn, RUXNWDDM_SOURCE_ID, &set,
						   &set_if);
	if (status == STATUS_GRAPHICS_SOURCE_NOT_IN_TOPOLOGY ||
	    status == STATUS_GRAPHICS_INVALID_VIDEO_PRESENT_SOURCE) {
		// Our source is off: blank it and wait for the next commit.
		a->mode_set = FALSE;
		ruxnwddm_blank(a);
		return STATUS_SUCCESS;
	}
	if (!NT_SUCCESS(status))
		return status;
	status = set_if->pfnAcquirePinnedModeInfo(set, &pinned);
	if (NT_SUCCESS(status) && pinned) {
		const D3DKMDT_GRAPHICS_RENDERING_FORMAT *g = &pinned->Format.Graphics;

		if (pinned->Type != D3DKMDT_RMT_GRAPHICS ||
		    g->PrimSurfSize.cx != a->scanout.width || g->PrimSurfSize.cy != a->scanout.height)
			status = STATUS_GRAPHICS_INVALID_VIDEO_PRESENT_SOURCE_MODE;
		else
			a->mode_set = TRUE;
		set_if->pfnReleaseModeInfo(set, pinned);
	} else if (NT_SUCCESS(status)) {
		a->mode_set = FALSE;
		ruxnwddm_blank(a);
	}
	vidpn_if->pfnReleaseSourceModeSet(args->hFunctionalVidPn, set);
	if (NT_SUCCESS(status)) {
		a->commits++;
		ruxnwddm_record_ulong(L"Commits", a->commits);
		ruxnwddm_record_ulong(L"ModeSet", a->mode_set);
	} else {
		ruxnwddm_record_ulong(L"CommitRefused", (ULONG)status);
	}
	return status;
}

// The monitor's one mode, the same as the source's.
NTSTATUS ruxnwddm_recommend_monitor_modes(RUXNWDDM_ADAPTER *a,
					  const DXGKARG_RECOMMENDMONITORMODES *args)
{
	const DXGK_MONITORSOURCEMODESET_INTERFACE *set_if = args->pMonitorSourceModeSetInterface;
	D3DKMDT_MONITOR_SOURCE_MODE *mode;
	NTSTATUS status;

	status = set_if->pfnCreateNewModeInfo(args->hMonitorSourceModeSet, &mode);
	if (!NT_SUCCESS(status))
		return status;
	fill_signal(a, &mode->VideoSignalInfo);
	mode->Origin = D3DKMDT_MCO_DRIVER;
	mode->Preference = D3DKMDT_MP_PREFERRED;
	mode->ColorBasis = D3DKMDT_CB_SRGB;
	mode->ColorCoeffDynamicRanges.FirstChannel = 8;
	mode->ColorCoeffDynamicRanges.SecondChannel = 8;
	mode->ColorCoeffDynamicRanges.ThirdChannel = 8;
	mode->ColorCoeffDynamicRanges.FourthChannel = 8;
	status = set_if->pfnAddMode(args->hMonitorSourceModeSet, mode);
	if (!NT_SUCCESS(status)) {
		set_if->pfnReleaseModeInfo(args->hMonitorSourceModeSet, mode);
		if (status == STATUS_GRAPHICS_MODE_ALREADY_IN_MODESET)
			status = STATUS_SUCCESS;
	}
	return status;
}
