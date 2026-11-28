// SPDX-License-Identifier: MIT
//
// ruxnumd.dll, the user-mode half of the full driver (milestone 2): the
// Direct3D 10/11 device driver interface Windows' Direct3D runtime loads for
// our adapter, in every process that makes a device on it, the compositor
// included.
//
// Two ways of being a device. Counting (milestone 2): accepted by the
// runtime and the compositor as a real device, making the allocations the
// compositor presents from and handing its presents to the kernel driver,
// but drawing nothing (draws are counted and dropped). Drawing (milestone 3,
// umd_inner.h): an inner Direct3D 11 device from DXMT behind every call,
// as Triton puts its inner device behind the same interface. Which way a
// process gets is the lab's choice (umd_inner_wanted).
//
// The adapter's entry points and the caps answers follow Triton's
// tritonDDI.c in utmapp/mesa, branch neptune (MIT, Copyright Turing
// Software LLC); the code is ours.

#ifndef RUXNWDDM_UMD_INTERNAL_H
#define RUXNWDDM_UMD_INTERNAL_H

#include <windows.h>

typedef LONG NTSTATUS;
#include <d3d10umddi.h>

#include "ruxnwddm_umd.h"
#include "ruxnwddm_escape.h"

// What the user-mode driver tells the kernel driver, through the
// UMD_NOTE escape, so a run can see which processes made devices.
#define UMD_NOTE_OPEN_ADAPTER	1
#define UMD_NOTE_CREATE_DEVICE	2
#define UMD_NOTE_CREATE_RESOURCE	3
#define UMD_NOTE_PRESENT	4
#define UMD_NOTE_DRAW		5
#define UMD_NOTE_FAILED		6

typedef struct umd_adapter {
	D3D10DDI_HRTADAPTER runtime;
	D3DDDI_ADAPTERCALLBACKS callbacks;
} UMD_ADAPTER;

typedef struct umd_device {
	UMD_ADAPTER *adapter;
	D3D10DDI_HRTDEVICE runtime;
	D3D10DDI_HRTCORELAYER core_layer;
	const D3DDDI_DEVICECALLBACKS *kt;	// the kernel thunks
	const D3D11DDI_CORELAYER_DEVICECALLBACKS *core;
	const DXGI_DDI_BASE_CALLBACKS *dxgi;
	HANDLE context;				// the kernel context
	D3DKMT_HANDLE sync_object;
	UINT interface_version;
	ULONG draws, presents, resources, blt_presents;

	// The drawing way (umd_inner.h); all NULL while counting.
	struct ID3D11Device1 *inner;
	struct ID3D11DeviceContext1 *context_inner;
	CRITICAL_SECTION lock;		// the inner context is used from one thread at a time
	BOOL lock_ready;
	D3DKMT_HANDLE paging_queue;	// for residency requests (umd_residency.c)
	const void *paging_fence;
	BOOL paging_queue_failed;
	BOOL removed;		// winemetal lost work (umd_inner_work_lost)
	// The render targets bound, to bind their remade inner views again
	// after a rotation (umd_view_rebind).
	struct umd_view *bound_rtvs[8];
	UINT bound_rtv_count;
	struct umd_view *bound_dsv;
	struct umd_blt *blt;		// the copier of blt presents (umd_blt.c)
	struct umd_shader *vs;		// bound vertex shader and element layout
	struct umd_layout *layout;
} UMD_DEVICE;

// A resource: its description and, when Windows needs one, its kernel
// allocation. CPU-visible bytes for Map live in memory of our own.
typedef struct umd_resource {
	D3D10DDIRESOURCE_TYPE type;
	UINT bind, misc, usage, cpu_access;
	UINT width, height, format;
	SIZE_T bytes;
	void *cpu;				// for Map
	D3DKMT_HANDLE allocation;		// 0 if none
	D3DKMT_HANDLE kernel_resource;
	BOOL primary;
	D3D10DDI_HRTRESOURCE runtime;

	// The drawing way: the inner resource, its shape, and its views.
	struct ID3D11Resource *inner;
	UINT mip_levels, array_size, depth;
	ULONG64 surface_token;		// its pixels are a device surface (ruxnwddm_umd.h)
	BOOL opened;			// another process made it (OpenResource)
	struct umd_view *views;
} UMD_RESOURCE;

// umd_adapter.c
__declspec(dllexport) HRESULT APIENTRY OpenAdapter10_2(D3D10DDIARG_OPENADAPTER *args);

// umd_device.c
HRESULT umd_create_device(UMD_ADAPTER *adapter, D3D10DDIARG_CREATEDEVICE *args);
void umd_note(UMD_DEVICE *dev, unsigned int what, unsigned int value);

// umd_resource.c
SIZE_T APIENTRY umd_calc_resource_size(D3D10DDI_HDEVICE, const D3D11DDIARG_CREATERESOURCE *);
void APIENTRY umd_create_resource(D3D10DDI_HDEVICE, const D3D11DDIARG_CREATERESOURCE *,
				  D3D10DDI_HRESOURCE, D3D10DDI_HRTRESOURCE);
SIZE_T APIENTRY umd_calc_open_resource_size(D3D10DDI_HDEVICE, const D3D10DDIARG_OPENRESOURCE *);
void APIENTRY umd_open_resource(D3D10DDI_HDEVICE, const D3D10DDIARG_OPENRESOURCE *,
				D3D10DDI_HRESOURCE, D3D10DDI_HRTRESOURCE);
void APIENTRY umd_destroy_resource(D3D10DDI_HDEVICE, D3D10DDI_HRESOURCE);
void APIENTRY umd_map(D3D10DDI_HDEVICE, D3D10DDI_HRESOURCE, UINT, D3D10_DDI_MAP, UINT,
		      D3D10DDI_MAPPED_SUBRESOURCE *);
void APIENTRY umd_unmap(D3D10DDI_HDEVICE, D3D10DDI_HRESOURCE, UINT);

// umd_present.c
void umd_fill_dxgi(void *table, SIZE_T bytes);
HRESULT umd_create_kernel_context(UMD_DEVICE *dev);

// umd_capture.c: the lab's copy of each shader the runtime hands over.
void APIENTRY umd_capture_vertex(D3D10DDI_HDEVICE, const UINT *, D3D10DDI_HSHADER, D3D10DDI_HRTSHADER,
				 const D3D11_1DDIARG_STAGE_IO_SIGNATURES *);
void APIENTRY umd_capture_geometry(D3D10DDI_HDEVICE, const UINT *, D3D10DDI_HSHADER, D3D10DDI_HRTSHADER,
				   const D3D11_1DDIARG_STAGE_IO_SIGNATURES *);
void APIENTRY umd_capture_pixel(D3D10DDI_HDEVICE, const UINT *, D3D10DDI_HSHADER, D3D10DDI_HRTSHADER,
				const D3D11_1DDIARG_STAGE_IO_SIGNATURES *);

// umd_residency.c
void umd_make_resident(UMD_DEVICE *dev, D3DKMT_HANDLE allocation);

// umd_format.c
void APIENTRY umd_check_format_support(D3D10DDI_HDEVICE h, DXGI_FORMAT format, UINT *caps);

// umd_stubs.c: device tables start at slot 0, DXGI tables at this one.
#define UMD_DXGI_FIRST_SLOT	192
#define UMD_VIDEO_FIRST_SLOT	224	// the video table's stubs log from here, wrapping
void umd_fill_stubs(void *table, SIZE_T bytes, ULONG first);
void umd_fill_dxgi_stubs(void *table, SIZE_T bytes);

// umd_video.c
void umd_video_init(D3D10DDIARG_CREATEDEVICE *args);

// umd_log.c
void umd_log(const char *what, ULONG64 value);
// The sample counts a format is drawn into (umd_format.c).
BOOL umd_multisamples(DXGI_FORMAT format, UINT samples);

// umd_util.c
void *umd_alloc(SIZE_T bytes);
void umd_free(void *p);
void umd_zero(void *p, SIZE_T bytes);
void umd_copy(void *to, const void *from, SIZE_T bytes);

#endif
