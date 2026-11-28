// SPDX-License-Identifier: MIT
//
// The adapter side of ruxnumd.dll: OpenAdapter10_2, the one export the
// Direct3D runtime looks for, the interface versions offered, and the caps
// the runtime and the compositor ask before they make a device.
//
// Every answer is a plain "not supported" except what a device needs to
// exist: the pipeline levels (they must come as a superset, or the runtime
// refuses the adapter), compute, and the GPU address width the kernel
// driver reports.
// The cases and their reasons follow Triton's tritonGetCaps (MIT, Copyright
// Turing Software LLC).
//
// Feature level 11.0 for every process. The runtime takes compute at 11.0
// from the shader caps' "compute plus raw and structured buffers in shader
// 4.x" bit: without it, its trace said "Driver doesn't support compute on
// FL11" (run rn35), D3D11CreateDevice failed with 0x887a0020 and the
// compositor failed over and over (rn32, rn33); with it, a device at 11.0
// ran d3d11tri's compute test right (rn36).
//
// Lab switches, per process: RUXNUMD_PIPELINE=101 gives 10.1 as before,
// =111 adds 11.1 (not yet proven: rn35 tried it before the shader bit was
// found); RUXNUMD_THREADS=1 adds free-threaded creates (every create and
// destroy holds the device's lock, umd_view.c).

#include "umd.h"

// The kernel driver's GPU MMU width (caps.c).
#define GPU_VA_BITS	48

// True when the variable is set to exactly this value.
static BOOL lab_switch(const WCHAR *name, const WCHAR *on)
{
	WCHAR value[8];
	DWORD n = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
	DWORD i;

	if (!n || n >= ARRAYSIZE(value))
		return FALSE;
	for (i = 0; on[i] || value[i]; i++)
		if (on[i] != value[i])
			return FALSE;
	return TRUE;
}

enum pipeline { PIPELINE_10_1, PIPELINE_11_0, PIPELINE_11_1 };

static enum pipeline pipeline_level(void)
{
	if (lab_switch(L"RUXNUMD_PIPELINE", L"101"))
		return PIPELINE_10_1;
	if (lab_switch(L"RUXNUMD_PIPELINE", L"111"))
		return PIPELINE_11_1;
	return PIPELINE_11_0;
}

static UINT64 offered_versions[] = {
	D3D11_0_DDI_SUPPORTED,
	D3D11_1_DDI_SUPPORTED,
	D3DWDDM1_3_DDI_SUPPORTED,
	D3DWDDM2_0_DDI_SUPPORTED,
};

static HRESULT APIENTRY get_supported_versions(D3D10DDI_HADAPTER adapter, UINT32 *entries,
					       UINT64 *versions)
{
	const UINT32 count = ARRAYSIZE(offered_versions);
	UINT32 i;

	UNREFERENCED_PARAMETER(adapter);
	umd_log("GetSupportedVersions entries", entries ? *entries : 0);
	if (!entries)
		return E_INVALIDARG;
	if (!versions) {
		*entries = count;
		return S_OK;
	}
	for (i = 0; i < count && i < *entries; i++)
		versions[i] = offered_versions[i];
	*entries = i;
	umd_log("GetSupportedVersions gave", i);
	return S_OK;
}

static HRESULT APIENTRY get_caps(D3D10DDI_HADAPTER adapter, const D3D10_2DDIARG_GETCAPS *args)
{
	UNREFERENCED_PARAMETER(adapter);
	// The size too: a question asked with a size this driver does not
	// expect would show here.
	umd_log("GetCaps type and size", (ULONG64)args->DataSize << 32 | (ULONG)args->Type);
	if (!args->pData || !args->DataSize)
		return S_OK;
	umd_zero(args->pData, args->DataSize);

	switch (args->Type) {
	case D3D11DDICAPS_3DPIPELINESUPPORT: {
		D3D11DDI_3DPIPELINESUPPORT_CAPS *caps = args->pData;
		enum pipeline level = pipeline_level();

		// Compute is carried (umd_compute.c), tessellation is refused at
		// the shader.
		caps->Caps = D3D11DDI_ENCODE_3DPIPELINESUPPORT_CAP(D3D11DDI_3DPIPELINELEVEL_10_0) |
			     D3D11DDI_ENCODE_3DPIPELINESUPPORT_CAP(D3D11DDI_3DPIPELINELEVEL_10_1);
		if (level >= PIPELINE_11_0)
			caps->Caps |= D3D11DDI_ENCODE_3DPIPELINESUPPORT_CAP(D3D11DDI_3DPIPELINELEVEL_11_0);
		if (level >= PIPELINE_11_1)
			caps->Caps |= D3D11DDI_ENCODE_3DPIPELINESUPPORT_CAP(D3D11_1DDI_3DPIPELINELEVEL_11_1);
		umd_log("GetCaps pipeline levels", caps->Caps);
		break;
	}
	case D3D11DDICAPS_THREADING: {
		D3D11DDI_THREADING_CAPS *caps = args->pData;

		// Optional, says the DDI; command lists stay the runtime's own.
		if (lab_switch(L"RUXNUMD_THREADS", L"1"))
			caps->Caps = D3D11DDICAPS_FREETHREADED;
		umd_log("GetCaps threading", caps->Caps);
		break;
	}
	case D3D11DDICAPS_SHADER: {
		D3D11DDI_SHADER_CAPS *caps = args->pData;

		// Compute, as the runtime reads it at 11.0 (head of this file).
		if (pipeline_level() >= PIPELINE_11_0)
			caps->Caps = D3D11DDICAPS_SHADER_COMPUTE_PLUS_RAW_AND_STRUCTURED_BUFFERS_IN_SHADER_4_X;
		umd_log("GetCaps shader", caps->Caps);
		break;
	}
	case D3D11_1DDICAPS_D3D11_OPTIONS: {
		D3D11_1DDI_D3D11_OPTIONS_DATA *caps = args->pData;

		// Asked of 11.1; DXMT's device answers it (it reports 11.1).
		if (pipeline_level() >= PIPELINE_11_1)
			caps->OutputMergerLogicOp = TRUE;
		umd_log("GetCaps logic op", caps->OutputMergerLogicOp);
		break;
	}
	case D3DWDDM2_0DDICAPS_GPUVA_CAPS: {
		D3DWDDM2_0DDI_GPUVA_CAPS_DATA *caps = args->pData;

		caps->MaxGPUVirtualAddressBitsPerResource = GPU_VA_BITS;
		break;
	}
	case D3DWDDM2_0DDICAPS_MEMORY_ARCHITECTURE: {
		D3DWDDM2_0DDI_MEMORY_ARCHITECTURE_CAPS *caps = args->pData;

		// Guest memory is the GPU's memory.
		caps->UMA = TRUE;
		break;
	}
	default:
		umd_log("GetCaps answered as none", (ULONG64)args->Type);
		// Threading, shader extras, the 11.1 and later options, the
		// architecture (not a tile-based renderer, which would change how
		// the compositor draws): all off, as zeroed above.
		break;
	}
	return S_OK;
}

static SIZE_T APIENTRY calc_device_size(D3D10DDI_HADAPTER adapter,
					const D3D10DDIARG_CALCPRIVATEDEVICESIZE *args)
{
	UNREFERENCED_PARAMETER(adapter);
	umd_log("CalcPrivateDeviceSize interface", args->Interface);
	return sizeof(UMD_DEVICE);
}

static HRESULT APIENTRY create_device(D3D10DDI_HADAPTER adapter, D3D10DDIARG_CREATEDEVICE *args)
{
	return umd_create_device(adapter.pDrvPrivate, args);
}

static HRESULT APIENTRY close_adapter(D3D10DDI_HADAPTER adapter)
{
	umd_free(adapter.pDrvPrivate);
	return S_OK;
}

HRESULT APIENTRY OpenAdapter10_2(D3D10DDIARG_OPENADAPTER *args)
{
	UMD_ADAPTER *a = umd_alloc(sizeof(*a));
	D3D10_2DDI_ADAPTERFUNCS *funcs = args->pAdapterFuncs_2;

	umd_log("OpenAdapter10_2 interface", args->Interface);
	umd_log("OpenAdapter10_2 version", args->Version);
	if (!a)
		return E_OUTOFMEMORY;
	a->runtime = args->hRTAdapter;
	if (args->pAdapterCallbacks)
		a->callbacks = *args->pAdapterCallbacks;
	args->hAdapter.pDrvPrivate = a;
	funcs->pfnCalcPrivateDeviceSize = calc_device_size;
	funcs->pfnCreateDevice = create_device;
	funcs->pfnCloseAdapter = close_adapter;
	funcs->pfnGetSupportedVersions = get_supported_versions;
	funcs->pfnGetCaps = get_caps;
	return S_OK;
}
