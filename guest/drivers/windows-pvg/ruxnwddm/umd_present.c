// SPDX-License-Identifier: MIT
//
// Presents of ruxnumd.dll: the device's kernel context, which presents are
// submitted on, and the DXGI entries. A present hands the presented
// resource's kernel allocation to the runtime's PresentCb, which reaches
// the kernel driver as a present and, for a flip, as SetVidPnSourceAddress
// (flip.c). Rotating resource identities swaps the kernel allocations
// behind a swap chain's buffers, as a flip-model swap chain needs.
//
// A drawing device (umd_inner.h) first waits until the inner device has
// done all the work handed to it, so that what is shown is finished: the
// kernel driver shows a presented surface as it is, with no copy, so an
// unfinished frame would be shown unfinished.
//
// The DXGI tiers and the kernel context's creation follow Triton's
// tritonDxgi.c (MIT, Copyright Turing Software LLC); the code is ours.

#include "umd_inner.h"

static UMD_DEVICE *device_of(DXGI_DDI_HDEVICE h)
{
	return (UMD_DEVICE *)h;
}

// Waits until the inner device has finished everything handed to it: an
// event query, ended after it, is done only then. Also at every flush
// (umd_draw.c).
void umd_inner_wait(UMD_DEVICE *dev)
{
	D3D11_QUERY_DESC desc = { D3D11_QUERY_EVENT, 0 };
	ID3D11Query *query;
	BOOL done = FALSE;

	umd_lock(dev);
	if (SUCCEEDED(ID3D11Device1_CreateQuery(dev->inner, &desc, &query))) {
		ID3D11DeviceContext1_End(dev->context_inner, (ID3D11Asynchronous *)query);
		while (ID3D11DeviceContext1_GetData(dev->context_inner, (ID3D11Asynchronous *)query, &done,
						    sizeof(done), 0) == S_FALSE)
			SwitchToThread();
		ID3D11Query_Release(query);
	} else {
		ID3D11DeviceContext1_Flush(dev->context_inner);
	}
	umd_unlock(dev);
}

static HRESULT submit_present(UMD_DEVICE *dev, UMD_RESOURCE *src, UMD_RESOURCE *dst, void *dxgi_context)
{
	DXGIDDICB_PRESENT cb;
	HRESULT hr;

	if (umd_inner_work_lost(dev))
		return D3DDDIERR_DEVICEREMOVED;

	if (!dev->dxgi || !dev->dxgi->pfnPresentCb)
		return E_FAIL;
	if (dev->inner)
		umd_inner_wait(dev);
	umd_zero(&cb, sizeof(cb));
	cb.hSrcAllocation = src ? src->allocation : 0;
	cb.hDstAllocation = dst ? dst->allocation : 0;
	cb.pDXGIContext = dxgi_context;
	cb.hContext = dev->context;
	hr = dev->dxgi->pfnPresentCb(dev->runtime.handle, &cb);
	dev->presents++;
	// The first present and every sixty-fourth are reported, with the
	// draws counted so far on this device; a failed one is reported as a
	// failure.
	if (FAILED(hr))
		umd_note(dev, UMD_NOTE_FAILED, (unsigned int)hr);
	else if (dev->presents == 1 || (dev->presents & 63) == 0)
		umd_note(dev, UMD_NOTE_PRESENT, dev->draws);
	return hr;
}

// A windowed swap chain of the older (blt) model presents into a
// destination: the compositor's surface for the window, opened in this
// process. The kernel driver copies nothing (driver_full.c), so the copy is
// the device's, a GPU copy on the inner device (run rn41: Tutorial 7 drew
// about 1,200 frames a second into a window that stayed black). Formats are
// logged at the first such present of a device.
static void copy_to_destination(UMD_DEVICE *dev, UMD_RESOURCE *src, UINT src_sub, UMD_RESOURCE *dst, UINT dst_sub)
{
	if (!dev->inner || !src || !dst || !src->inner || !dst->inner || src == dst)
		return;
	if (!dev->blt_presents++)
		umd_log("blt present: source and destination formats", (ULONG64)src->format << 32 | dst->format);
	umd_lock(dev);
	ID3D11DeviceContext1_CopySubresourceRegion(dev->context_inner, dst->inner, dst_sub, 0, 0, 0, src->inner,
						   src_sub, NULL);
	umd_unlock(dev);
}

static HRESULT APIENTRY present(DXGI_DDI_ARG_PRESENT *args)
{
	UMD_DEVICE *dev = device_of(args->hDevice);

	if (!dev->presents)
		umd_log("first present: a destination given", args->hDstResource != 0);
	copy_to_destination(dev, (UMD_RESOURCE *)args->hSurfaceToPresent, args->SrcSubResourceIndex,
			    (UMD_RESOURCE *)args->hDstResource, args->DstSubResourceIndex);
	return submit_present(dev, (UMD_RESOURCE *)args->hSurfaceToPresent, (UMD_RESOURCE *)args->hDstResource,
			      args->pDXGIContext);
}

// The newer present: several surfaces, of which the first is shown.
static HRESULT APIENTRY present1(DXGI_DDI_ARG_PRESENT1 *args)
{
	UMD_RESOURCE *src = NULL;

	if (!device_of(args->hDevice)->presents) {
		const UMD_RESOURCE *d = (const UMD_RESOURCE *)args->hDstResource;

		// Whether there is a destination, and whether it has an inner
		// resource (bit 1) and a surface (bit 2); the flags at bit 8.
		umd_log("first present1: destination, inner, surface; flags",
			(ULONG64)(d != NULL) | (ULONG64)(d && d->inner) << 1 | (ULONG64)(d && d->surface_token) << 2 |
				(ULONG64)args->Flags.Value << 8);
	}
	if (args->SurfacesToPresent && args->phSurfacesToPresent) {
		UMD_DEVICE *dev = device_of(args->hDevice);

		src = (UMD_RESOURCE *)args->phSurfacesToPresent[0].hSurface;
		// A blt with no destination given: the kernel's, copied here
		// on the device once the program's frame is done (umd_blt.c).
		if (args->Flags.Blt && !args->hDstResource && dev->inner) {
			umd_inner_wait(dev);
			umd_blt_copy(dev, src);
		}
		copy_to_destination(device_of(args->hDevice), src, args->phSurfacesToPresent[0].SubResourceIndex,
				    (UMD_RESOURCE *)args->hDstResource, args->DstSubResourceIndex);
	}
	return submit_present(device_of(args->hDevice), src, (UMD_RESOURCE *)args->hDstResource,
			      args->pDXGIContext);
}

// Resource i takes the identity of resource i + 1, the last that of the
// first; the runtime's own handle of each stays where it is.
static HRESULT APIENTRY rotate_identities(DXGI_DDI_ARG_ROTATE_RESOURCE_IDENTITIES *args)
{
	UMD_RESOURCE first;
	UINT i;

	UMD_DEVICE *dev = device_of(args->hDevice);

	if (args->Resources < 2)
		return S_OK;
	first = *(UMD_RESOURCE *)args->pResources[0];
	for (i = 0; i + 1 < args->Resources; i++) {
		UMD_RESOURCE *to = (UMD_RESOURCE *)args->pResources[i];
		const UMD_RESOURCE *from = (UMD_RESOURCE *)args->pResources[i + 1];

		to->allocation = from->allocation;
		to->kernel_resource = from->kernel_resource;
		to->cpu = from->cpu;
		to->inner = from->inner;
		to->surface_token = from->surface_token;
	}
	((UMD_RESOURCE *)args->pResources[i])->allocation = first.allocation;
	((UMD_RESOURCE *)args->pResources[i])->kernel_resource = first.kernel_resource;
	((UMD_RESOURCE *)args->pResources[i])->cpu = first.cpu;
	((UMD_RESOURCE *)args->pResources[i])->inner = first.inner;
	((UMD_RESOURCE *)args->pResources[i])->surface_token = first.surface_token;
	// Each resource's views follow its new inner resource.
	if (dev->inner) {
		umd_lock(dev);
		for (i = 0; i < args->Resources; i++) {
			UMD_VIEW *v;

			for (v = ((UMD_RESOURCE *)args->pResources[i])->views; v; v = v->next)
				umd_view_remake(dev, v);
		}
		umd_view_rebind(dev);
		umd_unlock(dev);
	}
	return S_OK;
}

// Hands the new primary to the graphics kernel. Without it the kernel never
// takes the resource's allocation as the source's primary, and every flip
// to it comes back STATUS_GRAPHICS_PRESENT_MODE_CHANGED (runs rn3 to rn5),
// as Triton's notes say of theirs (tritonDxgi.c).
static HRESULT APIENTRY set_display_mode(DXGI_DDI_ARG_SETDISPLAYMODE *args)
{
	UMD_DEVICE *dev = device_of(args->hDevice);
	UMD_RESOURCE *r = (UMD_RESOURCE *)args->hResource;
	D3DDDICB_SETDISPLAYMODE cb;
	HRESULT hr;

	if (!r || !r->allocation || !dev->kt->pfnSetDisplayModeCb)
		return E_INVALIDARG;
	umd_zero(&cb, sizeof(cb));
	cb.hPrimaryAllocation = r->allocation;
	hr = dev->kt->pfnSetDisplayModeCb(dev->runtime.handle, &cb);
	umd_log("SetDisplayMode", (ULONG)hr);
	return hr;
}

// Every allocation of ours is resident for as long as it lives
// (umd_residency.c).
static HRESULT APIENTRY query_residency(DXGI_DDI_ARG_QUERYRESOURCERESIDENCY *args)
{
	UINT i;

	for (i = 0; i < args->Resources; i++)
		args->pStatus[i] = DXGI_DDI_RESIDENCY_FULLY_RESIDENT;
	return S_OK;
}

// No gamma ramp: the identity.
static HRESULT APIENTRY gamma_caps(DXGI_DDI_ARG_GET_GAMMA_CONTROL_CAPS *args)
{
	DXGI_GAMMA_CONTROL_CAPABILITIES *c = args->pGammaCapabilities;

	umd_zero(c, sizeof(*c));
	c->MaxConvertedValue = 1.0f;
	return S_OK;
}

// The first entries of every DXGI tier are the same; later tiers only add.
void umd_fill_dxgi(void *table, SIZE_T bytes)
{
	DXGI_DDI_BASE_FUNCTIONS *f = table;

	if (!table)
		return;
	umd_fill_dxgi_stubs(table, bytes);
	f->pfnPresent = present;
	f->pfnRotateResourceIdentities = rotate_identities;
	f->pfnSetDisplayMode = set_display_mode;
	f->pfnQueryResourceResidency = query_residency;
	f->pfnGetGammaCaps = gamma_caps;
	// Present1 came with DXGI 1.3, at the same place in every later tier.
	if (bytes >= RTL_SIZEOF_THROUGH_FIELD(DXGI1_3_DDI_BASE_FUNCTIONS, pfnPresent1))
		((DXGI1_3_DDI_BASE_FUNCTIONS *)table)->pfnPresent1 = present1;
}

// One kernel context for the device, on the one engine, made the way the
// kernel driver's memory model asks for (virtual addressing).
HRESULT umd_create_kernel_context(UMD_DEVICE *dev)
{
	HRESULT hr;

	if (dev->kt->pfnCreateContextVirtualCb) {
		D3DDDICB_CREATECONTEXTVIRTUAL cb;

		umd_zero(&cb, sizeof(cb));
		cb.NodeOrdinal = 0;
		cb.EngineAffinity = 1;
		hr = dev->kt->pfnCreateContextVirtualCb(dev->runtime.handle, &cb);
		if (SUCCEEDED(hr))
			dev->context = cb.hContext;
	} else {
		D3DDDICB_CREATECONTEXT cb;

		umd_zero(&cb, sizeof(cb));
		cb.NodeOrdinal = 0;
		cb.EngineAffinity = 1;
		hr = dev->kt->pfnCreateContextCb(dev->runtime.handle, &cb);
		if (SUCCEEDED(hr))
			dev->context = cb.hContext;
	}
	return hr;
}
