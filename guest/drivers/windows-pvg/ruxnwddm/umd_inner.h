// SPDX-License-Identifier: MIT
//
// The drawing half of ruxnumd.dll (milestone 3): every Direct3D device the
// runtime makes on our adapter gets an inner Direct3D 11 device from DXMT
// (ruxnd3d11.dll, ruxndxgi.dll and our winemetal.dll, beside this DLL in the
// driver store), and the device-driver-interface calls become the same
// calls on it. DXMT turns them into Metal-shaped commands, winemetal into
// the device's wire, and ruxnwddm's escapes carry them to the device. This
// is Triton's arrangement (utmapp/mesa, branch neptune, MIT, Copyright Turing
// Software LLC) with DXMT as the inner device; the code is ours, written
// against the same interface, and smaller: only what the compositor and
// ordinary Direct3D 11 programs at feature level 10.1 use.
//
// Each driver object below lies in the memory the runtime gives it (its
// CalcPrivate*Size) and holds the inner device's object.

#ifndef RUXNWDDM_UMD_INNER_H
#define RUXNWDDM_UMD_INNER_H

#define COBJMACROS
#include "umd.h"
#include <d3d11_1.h>

// The stages a shader object can be.
#define UMD_STAGE_VERTEX	0
#define UMD_STAGE_GEOMETRY	1
#define UMD_STAGE_PIXEL		2
#define UMD_STAGE_COMPUTE	3

// What a view is a view of, for the view objects' common part.
#define UMD_VIEW_RENDER_TARGET	1
#define UMD_VIEW_DEPTH_STENCIL	2
#define UMD_VIEW_SHADER_RESOURCE 3
#define UMD_VIEW_UNORDERED_ACCESS 4

#define UMD_MOST_ELEMENTS	D3D11_IA_VERTEX_INPUT_STRUCTURE_ELEMENT_COUNT

struct umd_view;

// A view: the API description it was made with, kept so that it can be
// made again on the inner resource a rotation hands its resource.
typedef struct umd_view {
	UINT kind;			// UMD_VIEW_*
	UMD_RESOURCE *resource;
	struct umd_view *next;		// among its resource's views
	union {
		D3D11_RENDER_TARGET_VIEW_DESC rtv;
		D3D11_DEPTH_STENCIL_VIEW_DESC dsv;
		D3D11_SHADER_RESOURCE_VIEW_DESC srv;
		D3D11_UNORDERED_ACCESS_VIEW_DESC uav;
	} desc;
	ID3D11View *inner;
} UMD_VIEW;

typedef struct umd_shader {
	UINT stage;			// UMD_STAGE_*
	ID3D11DeviceChild *inner;
} UMD_SHADER;

// An element layout: the API's description, named as umd_dxbc.c names
// vertex shader inputs, and the inner input layout made from it.
typedef struct umd_layout {
	D3D11_INPUT_ELEMENT_DESC elements[UMD_MOST_ELEMENTS];
	UINT count;
	ID3D11InputLayout *inner;
} UMD_LAYOUT;

// Blend, depth-stencil, rasterizer and sampler states.
typedef struct umd_state {
	IUnknown *inner;
} UMD_STATE;

// Queries are answered by the inner device when it has them, else at
// once (umd_draw.c).
typedef struct umd_query {
	D3D10DDI_QUERY type;
	ID3D11Query *inner;
} UMD_QUERY;

// umd_inner.c: the inner device.
HRESULT umd_inner_create(UMD_DEVICE *dev);
void umd_inner_destroy(UMD_DEVICE *dev);
BOOL umd_inner_wanted(void);
BOOL umd_inner_surface_next(UINT width, UINT height);
ULONG64 umd_inner_surface_token(void);
void umd_lock(UMD_DEVICE *dev);
void umd_unlock(UMD_DEVICE *dev);
void umd_error(UMD_DEVICE *dev, HRESULT hr, const char *what);

// umd_resource_inner.c
void umd_fill_resources(D3DWDDM2_0DDI_DEVICEFUNCS *f);

// umd_view.c
void umd_fill_views(D3DWDDM2_0DDI_DEVICEFUNCS *f);
HRESULT umd_view_remake(UMD_DEVICE *dev, UMD_VIEW *v);
void umd_view_rebind(UMD_DEVICE *dev);
void umd_inner_wait(UMD_DEVICE *dev);
HRESULT umd_inner_make(ID3D11Device1 **device, ID3D11DeviceContext1 **context);
// umd_blt.c
void umd_blt_copy(UMD_DEVICE *dev, UMD_RESOURCE *src);
void umd_blt_destroy(UMD_DEVICE *dev);

// umd_state.c
void umd_fill_states(D3DWDDM2_0DDI_DEVICEFUNCS *f);

// umd_shader.c
void umd_fill_shaders(D3DWDDM2_0DDI_DEVICEFUNCS *f);

// umd_draw.c
void umd_fill_draws(D3DWDDM2_0DDI_DEVICEFUNCS *f);
void umd_flush(UMD_DEVICE *dev);

// umd_device.c: the drawing device's table, over the counting one.
void umd_fill_drawing(D3DWDDM2_0DDI_DEVICEFUNCS *f);

// umd_dxbc.c
void *umd_dxbc_build(const UINT *code, const D3D11_1DDIARG_STAGE_IO_SIGNATURES *sig, BOOL pixel,
		     SIZE_T *bytes);
void *umd_dxbc_input_signature(const D3D11_1DDIARG_SIGNATURE_ENTRY2 *entries, UINT count, SIZE_T *bytes);

// umd_md5.c
void umd_dxbc_checksum(const UCHAR *container, SIZE_T bytes, UCHAR out[16]);

static __forceinline UMD_DEVICE *umd_device(D3D10DDI_HDEVICE h)
{
	return h.pDrvPrivate;
}

void umd_fill_compute(D3DWDDM2_0DDI_DEVICEFUNCS *f);

// Failures winemetal said on this thread: cleared before an inner create,
// and asked after it, which turns a create it could not make into
// E_OUTOFMEMORY and lets go of what DXMT made.
void umd_inner_clear_failures(void);
HRESULT umd_inner_checked(HRESULT hr, void **made);
BOOL umd_inner_work_lost(UMD_DEVICE *dev);

#endif
