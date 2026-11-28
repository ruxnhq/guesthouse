// SPDX-License-Identifier: MIT
//
// The inner device: DXMT's Direct3D 11, loaded from beside this DLL in the
// driver store, over our winemetal.dll. One DXMT device for each device the
// runtime makes on our adapter.
//
// Every process draws through it from boot (UmdDraw 2, the default since
// run rn14), so the compositor never meets a window made before drawing
// began. The value UmdDraw under the kernel driver's Parameters key can
// still say 0 for no process or 1 for processes whose environment has
// RUXNUMD_DRAW=1, for tests. A process that does not draw gets milestone
// 2's counting device.
//
// winemetal.dll also exports two calls for this driver: one asks that the
// next texture the inner device makes lie over a device surface (made in
// this process's task and exported), the other hands back that surface's
// token, which goes into the texture's kernel allocation so the kernel
// driver can show it (umd_resource.c, ruxnwddm_umd.h).

#include "umd_inner.h"

#define PARAMETERS_KEY	L"SYSTEM\\CurrentControlSet\\Services\\ruxnwddm\\Parameters"
#define DRAW_NONE	0
#define DRAW_ASKED	1
#define DRAW_ALL	2
#define PATH_CHARS	MAX_PATH

typedef BOOL(WINAPI *SURFACE_NEXT)(UINT width, UINT height);
typedef ULONG64(WINAPI *SURFACE_TOKEN)(void);

static INIT_ONCE load_once = INIT_ONCE_STATIC_INIT;
static PFN_D3D11_CREATE_DEVICE create_device;
static SURFACE_NEXT surface_next;
typedef unsigned(WINAPI *TAKE_FAILURES)(void);
typedef void(WINAPI *CREATING)(BOOL on);
typedef unsigned(WINAPI *WORK_LOST)(void);
static TAKE_FAILURES take_failures;
static CREATING creating;
static WORK_LOST work_lost;
static SURFACE_TOKEN surface_token;

BOOL umd_inner_wanted(void)
{
	DWORD mode = DRAW_ALL, bytes = sizeof(mode);
	WCHAR value[4];

	if (RegGetValueW(HKEY_LOCAL_MACHINE, PARAMETERS_KEY, L"UmdDraw", RRF_RT_REG_DWORD, NULL, &mode,
			 &bytes) != ERROR_SUCCESS)
		mode = DRAW_ALL;
	if (mode == DRAW_ALL)
		return TRUE;
	if (mode == DRAW_ASKED)
		return GetEnvironmentVariableW(L"RUXNUMD_DRAW", value, ARRAYSIZE(value)) == 1 &&
		       value[0] == L'1';
	return FALSE;
}

// The folder this DLL was loaded from, with a backslash at its end.
static BOOL own_folder(WCHAR *path, DWORD chars)
{
	HMODULE self;
	DWORD n, i, slash = 0;

	if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
				GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
				(LPCWSTR)(void *)umd_inner_wanted, &self))
		return FALSE;
	n = GetModuleFileNameW(self, path, chars);
	if (!n || n >= chars)
		return FALSE;
	for (i = 0; i < n; i++)
		if (path[i] == L'\\')
			slash = i;
	path[slash + 1] = 0;
	return slash != 0;
}

static HMODULE load_beside(const WCHAR *folder, const WCHAR *name)
{
	WCHAR path[PATH_CHARS];
	DWORD i = 0, j = 0;

	while (folder[i] && i < PATH_CHARS - 1)
		path[j++] = folder[i++];
	for (i = 0; name[i] && j < PATH_CHARS - 1; i++)
		path[j++] = name[i];
	path[j] = 0;
	return LoadLibraryExW(path, NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
}

// winemetal before ruxndxgi before ruxnd3d11, so each one's imports bind to
// the one already loaded from the same folder.
static BOOL CALLBACK load(PINIT_ONCE once, PVOID param, PVOID *context)
{
	WCHAR folder[PATH_CHARS];
	HMODULE winemetal, dxgi, d3d11;

	UNREFERENCED_PARAMETER(once);
	UNREFERENCED_PARAMETER(param);
	UNREFERENCED_PARAMETER(context);
	if (!own_folder(folder, PATH_CHARS))
		return TRUE;
	winemetal = load_beside(folder, L"winemetal.dll");
	dxgi = winemetal ? load_beside(folder, L"ruxndxgi.dll") : NULL;
	d3d11 = dxgi ? load_beside(folder, L"ruxnd3d11.dll") : NULL;
	umd_log("inner: DXMT loaded", d3d11 != NULL);
	if (!d3d11)
		return TRUE;
	create_device = (PFN_D3D11_CREATE_DEVICE)(void *)GetProcAddress(d3d11, "D3D11CreateDevice");
	surface_next = (SURFACE_NEXT)(void *)GetProcAddress(winemetal, "RuxnSurfaceNext");
	take_failures = (TAKE_FAILURES)(void *)GetProcAddress(winemetal, "RuxnTakeFailures");
	creating = (CREATING)(void *)GetProcAddress(winemetal, "RuxnCreating");
	work_lost = (WORK_LOST)(void *)GetProcAddress(winemetal, "RuxnWorkLost");
	surface_token = (SURFACE_TOKEN)(void *)GetProcAddress(winemetal, "RuxnSurfaceToken");
	return TRUE;
}

// A second device of DXMT's in this process, with its own context, for
// work that must not touch the program's state (umd_blt.c).
HRESULT umd_inner_make(ID3D11Device1 **device1, ID3D11DeviceContext1 **context1)
{
	static const D3D_FEATURE_LEVEL level = D3D_FEATURE_LEVEL_11_0;
	ID3D11Device *device = NULL;
	ID3D11DeviceContext *context = NULL;
	HRESULT hr;

	*device1 = NULL;
	*context1 = NULL;
	InitOnceExecuteOnce(&load_once, load, NULL, NULL);
	if (!create_device)
		return E_FAIL;
	hr = create_device(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, &level, 1, D3D11_SDK_VERSION, &device, NULL,
			   &context);
	if (FAILED(hr))
		return hr;
	hr = ID3D11Device_QueryInterface(device, &IID_ID3D11Device1, (void **)device1);
	if (SUCCEEDED(hr))
		hr = ID3D11DeviceContext_QueryInterface(context, &IID_ID3D11DeviceContext1, (void **)context1);
	ID3D11DeviceContext_Release(context);
	ID3D11Device_Release(device);
	return hr;
}

HRESULT umd_inner_create(UMD_DEVICE *dev)
{
	static const D3D_FEATURE_LEVEL level = D3D_FEATURE_LEVEL_11_0;
	ID3D11Device *device = NULL;
	ID3D11DeviceContext *context = NULL;
	HRESULT hr;

	InitOnceExecuteOnce(&load_once, load, NULL, NULL);
	if (!create_device)
		return E_FAIL;
	hr = create_device(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, &level, 1, D3D11_SDK_VERSION, &device, NULL,
			   &context);
	umd_log("inner: D3D11CreateDevice", (ULONG)hr);
	if (FAILED(hr))
		return hr;
	hr = ID3D11Device_QueryInterface(device, &IID_ID3D11Device1, (void **)&dev->inner);
	if (SUCCEEDED(hr))
		hr = ID3D11DeviceContext_QueryInterface(context, &IID_ID3D11DeviceContext1,
							(void **)&dev->context_inner);
	ID3D11DeviceContext_Release(context);
	ID3D11Device_Release(device);
	if (FAILED(hr)) {
		umd_inner_destroy(dev);
		return hr;
	}
	InitializeCriticalSection(&dev->lock);
	dev->lock_ready = TRUE;
	return S_OK;
}

void umd_inner_destroy(UMD_DEVICE *dev)
{
	umd_blt_destroy(dev);
	if (dev->context_inner) {
		ID3D11DeviceContext1_ClearState(dev->context_inner);
		ID3D11DeviceContext1_Flush(dev->context_inner);
		ID3D11DeviceContext1_Release(dev->context_inner);
		dev->context_inner = NULL;
	}
	if (dev->inner) {
		ID3D11Device1_Release(dev->inner);
		dev->inner = NULL;
	}
	if (dev->lock_ready) {
		DeleteCriticalSection(&dev->lock);
		dev->lock_ready = FALSE;
	}
}

BOOL umd_inner_surface_next(UINT width, UINT height)
{
	return surface_next && surface_next(width, height);
}

ULONG64 umd_inner_surface_token(void)
{
	return surface_token ? surface_token() : 0;
}

void umd_lock(UMD_DEVICE *dev)
{
	EnterCriticalSection(&dev->lock);
}

void umd_unlock(UMD_DEVICE *dev)
{
	LeaveCriticalSection(&dev->lock);
}

// Failures winemetal said on this thread since the last ask: cleared
// before an inner create, asked after it. A create DXMT reports as made
// but winemetal could not make fails with E_OUTOFMEMORY, and what DXMT made
// is let go (run rn80).
void umd_inner_clear_failures(void)
{
	if (creating)
		creating(TRUE);
	if (take_failures)
		take_failures();
}

HRESULT umd_inner_checked(HRESULT hr, void **made)
{
	unsigned failed = take_failures ? take_failures() : 0;

	if (creating)
		creating(FALSE);
	if (FAILED(hr) || !failed)
		return hr;
	if (made && *made) {
		IUnknown_Release((IUnknown *)*made);
		*made = NULL;
	}
	return E_OUTOFMEMORY;
}

// Whether winemetal lost work it could not return to any call (RuxnWorkLost):
// the device then counts as removed. Said once a device, through the
// runtime, as D3DDDIERR_DEVICEREMOVED, at every call that asks; present
// returns it too.
BOOL umd_inner_work_lost(UMD_DEVICE *dev)
{
	if (!work_lost || !work_lost())
		return FALSE;
	if (!dev->removed) {
		dev->removed = TRUE;
		umd_log("work lost in winemetal: the device is reported removed", work_lost());
	}
	if (dev->core && dev->core->pfnSetErrorCb)
		dev->core->pfnSetErrorCb(dev->core_layer, D3DDDIERR_DEVICEREMOVED);
	return TRUE;
}

// A call that returns nothing reports its failure through the runtime.
void umd_error(UMD_DEVICE *dev, HRESULT hr, const char *what)
{
	umd_log(what, (ULONG)hr);
	umd_note(dev, UMD_NOTE_FAILED, (unsigned int)hr);
	if (dev->core && dev->core->pfnSetErrorCb)
		dev->core->pfnSetErrorCb(dev->core_layer, hr);
}
