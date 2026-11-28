// SPDX-License-Identifier: MIT
//
// The device side of ruxnumd.dll: CreateDevice and the table of Direct3D 11.1
// device functions.
//
// Milestone 2 does not draw, so almost every entry of the table is a stub
// that does nothing and returns zero (umd_stubs.c), which is a correct
// answer for each of them: S_OK where an HRESULT is returned, FALSE where a
// BOOL is, nothing where nothing is. On arm64 a function may ignore the
// arguments it is called with. Only the entries whose answer the runtime or the
// compositor uses are real: the private sizes, resources and their maps,
// the format checks, queries, the device's end, and counting draws.

#include "umd_inner.h"

// The size of every private object but resources: they keep nothing yet.
#define PRIVATE_OBJECT_BYTES	64


static UMD_DEVICE *device_of(D3D10DDI_HDEVICE h)
{
	return h.pDrvPrivate;
}

static SIZE_T APIENTRY small_size(void)
{
	return PRIVATE_OBJECT_BYTES;
}

static void APIENTRY count_draw(D3D10DDI_HDEVICE h)
{
	device_of(h)->draws++;
}

static void APIENTRY check_multisample(D3D10DDI_HDEVICE h, DXGI_FORMAT format, UINT samples,
				       UINT flags, UINT *levels)
{
	static volatile LONG logged;

	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(flags);
	*levels = samples == 1 || umd_multisamples(format, samples) ? 1 : 0;
	// The first few questions of a process, as for formats (umd_format.c).
	if (InterlockedIncrement(&logged) <= 32)
		umd_log("CheckMultisampleQualityLevels format, samples, levels",
			(ULONG64)format << 32 | samples << 8 | *levels);
}

static void APIENTRY check_counter_info(D3D10DDI_HDEVICE h, D3D10DDI_COUNTER_INFO *info)
{
	UNREFERENCED_PARAMETER(h);
	umd_zero(info, sizeof(*info));
}

// Queries are answered at once, with zeroes: nothing is ever pending.
static void APIENTRY query_get_data(D3D10DDI_HDEVICE h, D3D10DDI_HQUERY query, void *data,
				    UINT bytes, UINT flags)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(query);
	UNREFERENCED_PARAMETER(flags);
	if (data)
		umd_zero(data, bytes);
}

static void APIENTRY check_handle_sizes(D3D10DDI_HDEVICE h, UINT *count, D3D11DDI_HANDLESIZE *sizes)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(sizes);
	*count = 0;
}

static void APIENTRY check_direct_flip(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE a,
				       D3D10DDI_HRESOURCE b, UINT flags, BOOL *supported)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(a);
	UNREFERENCED_PARAMETER(b);
	UNREFERENCED_PARAMETER(flags);
	*supported = FALSE;
}

static void APIENTRY destroy_device(D3D10DDI_HDEVICE h)
{
	UMD_DEVICE *dev = device_of(h);

	if (dev->inner)
		umd_inner_destroy(dev);
	umd_log("DestroyDevice draws", dev->draws);
	umd_note(dev, UMD_NOTE_DRAW, dev->draws);
	if (dev->context && dev->kt->pfnDestroyContextCb) {
		D3DDDICB_DESTROYCONTEXT destroy = { 0 };

		destroy.hContext = dev->context;
		dev->kt->pfnDestroyContextCb(dev->runtime.handle, &destroy);
	}
}

// The entries every device table (Direct3D 11.1, WDDM 1.3, 2.0) has under
// the same names; the rest of each table stays "nothing".
#define FILL_COMMON(f)							\
	do {								\
		(f)->pfnCalcPrivateResourceSize = umd_calc_resource_size; \
		(f)->pfnCreateResource = umd_create_resource;		\
		(f)->pfnCalcPrivateOpenedResourceSize = umd_calc_open_resource_size; \
		(f)->pfnOpenResource = umd_open_resource;		\
		(f)->pfnDestroyResource = umd_destroy_resource;		\
		(f)->pfnResourceMap = umd_map;				\
		(f)->pfnStagingResourceMap = umd_map;			\
		(f)->pfnDynamicResourceMapDiscard = umd_map;		\
		(f)->pfnDynamicIABufferMapDiscard = umd_map;		\
		(f)->pfnDynamicIABufferMapNoOverwrite = umd_map;	\
		(f)->pfnDynamicConstantBufferMapDiscard = umd_map;	\
		(f)->pfnDynamicConstantBufferMapNoOverwrite = umd_map;	\
		(f)->pfnResourceUnmap = umd_unmap;			\
		(f)->pfnStagingResourceUnmap = umd_unmap;		\
		(f)->pfnDynamicResourceUnmap = umd_unmap;		\
		(f)->pfnDynamicIABufferUnmap = umd_unmap;		\
		(f)->pfnDynamicConstantBufferUnmap = umd_unmap;		\
		(f)->pfnCalcPrivateShaderResourceViewSize = (void *)small_size; \
		(f)->pfnCalcPrivateRenderTargetViewSize = (void *)small_size; \
		(f)->pfnCalcPrivateDepthStencilViewSize = (void *)small_size; \
		(f)->pfnCalcPrivateElementLayoutSize = (void *)small_size; \
		(f)->pfnCalcPrivateBlendStateSize = (void *)small_size;	\
		(f)->pfnCalcPrivateDepthStencilStateSize = (void *)small_size; \
		(f)->pfnCalcPrivateRasterizerStateSize = (void *)small_size; \
		(f)->pfnCalcPrivateShaderSize = (void *)small_size;	\
		(f)->pfnCalcPrivateGeometryShaderWithStreamOutput = (void *)small_size; \
		(f)->pfnCalcPrivateSamplerSize = (void *)small_size;	\
		(f)->pfnCalcPrivateQuerySize = (void *)small_size;	\
		(f)->pfnCalcPrivateTessellationShaderSize = (void *)small_size; \
		(f)->pfnCalcPrivateUnorderedAccessViewSize = (void *)small_size; \
		(f)->pfnCalcDeferredContextHandleSize = (void *)small_size; \
		(f)->pfnCalcPrivateDeferredContextSize = (void *)small_size; \
		(f)->pfnCalcPrivateCommandListSize = (void *)small_size; \
		(f)->pfnCheckFormatSupport = umd_check_format_support;	\
		(f)->pfnCheckMultisampleQualityLevels = (void *)check_multisample; \
		(f)->pfnCheckCounterInfo = check_counter_info;		\
		(f)->pfnQueryGetData = query_get_data;			\
		(f)->pfnCheckDeferredContextHandleSizes = check_handle_sizes; \
		(f)->pfnCheckDirectFlipSupport = check_direct_flip;	\
		(f)->pfnDestroyDevice = destroy_device;			\
		(f)->pfnDraw = (void *)count_draw;			\
		(f)->pfnDrawIndexed = (void *)count_draw;		\
		(f)->pfnDrawInstanced = (void *)count_draw;		\
		(f)->pfnDrawIndexedInstanced = (void *)count_draw;	\
		(f)->pfnDrawAuto = (void *)count_draw;			\
		(f)->pfnDispatch = (void *)count_draw;			\
		(f)->pfnClearRenderTargetView = (void *)count_draw;	\
		(f)->pfnClearView = (void *)count_draw;			\
		(f)->pfnCreateVertexShader = (void *)umd_capture_vertex;	\
		(f)->pfnCreateGeometryShader = (void *)umd_capture_geometry;	\
		(f)->pfnCreatePixelShader = (void *)umd_capture_pixel;	\
	} while (0)

// Every entry a stub that does nothing first (umd_stubs.c): the tables
// are nothing but function pointers.
static void fill_nothing(void *table, SIZE_T bytes)
{
	umd_fill_stubs(table, bytes, 0);
}

static void fill_device_funcs_11_1(D3D11_1DDI_DEVICEFUNCS *f)
{
	fill_nothing(f, sizeof(*f));
	FILL_COMMON(f);
}

static void fill_device_funcs_wddm1_3(D3DWDDM1_3DDI_DEVICEFUNCS *f)
{
	fill_nothing(f, sizeof(*f));
	FILL_COMMON(f);
}

static void fill_device_funcs_wddm2_0(D3DWDDM2_0DDI_DEVICEFUNCS *f, BOOL drawing)
{
	fill_nothing(f, sizeof(*f));
	// Same names, some with WDDM 2.0 argument types; the answers are the
	// same.
	FILL_COMMON(f);
	if (drawing)
		umd_fill_drawing(f);
}

// Tells the kernel driver what happened, for wddmtool and the registry.
void umd_note(UMD_DEVICE *dev, unsigned int what, unsigned int value)
{
	struct ruxnwddm_escape_note note;
	D3DDDICB_ESCAPE esc;

	if (!dev || !dev->kt || !dev->kt->pfnEscapeCb)
		return;
	umd_zero(&note, sizeof(note));
	note.header.magic = RUXNWDDM_ESCAPE_MAGIC;
	note.header.version = RUXNWDDM_ESCAPE_VERSION;
	note.header.command = RUXNWDDM_ESCAPE_UMD_NOTE;
	note.what = what;
	note.value = value;
	note.process = GetCurrentProcessId();
	umd_zero(&esc, sizeof(esc));
	esc.pPrivateDriverData = &note;
	esc.PrivateDriverDataSize = sizeof(note);
	dev->kt->pfnEscapeCb(dev->adapter->runtime.handle, &esc);
}

HRESULT umd_create_device(UMD_ADAPTER *adapter, D3D10DDIARG_CREATEDEVICE *args)
{
	UMD_DEVICE *dev = args->hDrvDevice.pDrvPrivate;
	HRESULT hr;

	umd_log("CreateDevice interface", args->Interface);
	umd_log("CreateDevice version", args->Version);
	umd_log("CreateDevice flags", args->Flags);
	if (!dev)
		return E_INVALIDARG;
	umd_zero(dev, sizeof(*dev));
	dev->adapter = adapter;
	dev->runtime = args->hRTDevice;
	dev->core_layer = args->hRTCoreLayer;
	dev->kt = args->pKTCallbacks;
	dev->core = args->p11UMCallbacks;
	dev->interface_version = args->Interface;
	if (args->DXGIBaseDDI.pDXGIBaseCallbacks)
		dev->dxgi = args->DXGIBaseDDI.pDXGIBaseCallbacks;

	switch (args->Interface) {
	case D3D11_0_DDI_INTERFACE_VERSION:
	case D3D11_1_DDI_INTERFACE_VERSION:
		// The 11.0 table is the 11.1 table's first part.
		fill_device_funcs_11_1(args->p11_1DeviceFuncs);
		break;
	case D3DWDDM1_3_DDI_INTERFACE_VERSION:
		fill_device_funcs_wddm1_3(args->pWDDM1_3DeviceFuncs);
		break;
	case D3DWDDM2_0_DDI_INTERFACE_VERSION:
		// Only the WDDM 2.0 table, which the runtime asks for on this
		// build, has the drawing way; a process that does not get it,
		// or whose inner device cannot be made, counts.
		if (umd_inner_wanted() && SUCCEEDED(umd_inner_create(dev)))
			umd_log("CreateDevice drawing", 1);
		fill_device_funcs_wddm2_0(args->pWDDM2_0DeviceFuncs, dev->inner != NULL);
		break;
	default:
		umd_log("CreateDevice refused interface", args->Interface);
		umd_note(dev, UMD_NOTE_FAILED, args->Interface);
		return E_FAIL;
	}
	// The video tables, asked for later through the sub-object entry.
	umd_video_init(args);
	// The DXGI table's tier follows the interface asked for.
	if (IS_DXGI1_4_BASE_FUNCTIONS(args->Interface, args->Version))
		umd_fill_dxgi(args->DXGIBaseDDI.pDXGIDDIBaseFunctions5,
			      sizeof(*args->DXGIBaseDDI.pDXGIDDIBaseFunctions5));
	else if (IS_DXGI1_3_BASE_FUNCTIONS(args->Interface, args->Version))
		umd_fill_dxgi(args->DXGIBaseDDI.pDXGIDDIBaseFunctions4,
			      sizeof(*args->DXGIBaseDDI.pDXGIDDIBaseFunctions4));
	else if (IS_DXGI1_2_BASE_FUNCTIONS(args->Interface, args->Version))
		umd_fill_dxgi(args->DXGIBaseDDI.pDXGIDDIBaseFunctions3,
			      sizeof(*args->DXGIBaseDDI.pDXGIDDIBaseFunctions3));
	else if (IS_DXGI1_1_BASE_FUNCTIONS(args->Interface, args->Version))
		umd_fill_dxgi(args->DXGIBaseDDI.pDXGIDDIBaseFunctions2,
			      sizeof(*args->DXGIBaseDDI.pDXGIDDIBaseFunctions2));
	else
		umd_fill_dxgi(args->DXGIBaseDDI.pDXGIDDIBaseFunctions,
			      sizeof(*args->DXGIBaseDDI.pDXGIDDIBaseFunctions));

	hr = umd_create_kernel_context(dev);
	umd_log("CreateDevice kernel context", (ULONG)hr);
	umd_note(dev, UMD_NOTE_CREATE_DEVICE, (unsigned int)hr);
	return hr;
}

// The drawing device: every entry the inner device answers, over the
// counting device's table, whose other answers (format checks and the like)
// stay.
void umd_fill_drawing(D3DWDDM2_0DDI_DEVICEFUNCS *f)
{
	umd_fill_resources(f);
	umd_fill_views(f);
	umd_fill_states(f);
	umd_fill_shaders(f);
	umd_fill_draws(f);
	umd_fill_compute(f);
}
