// SPDX-License-Identifier: MIT
//
// Resources of the drawing device: each one an inner resource of DXMT's.
//
// A resource Windows must see in the kernel (a primary, a swap chain's back
// buffer, a resource shared with another process) also gets one kernel
// allocation, as in milestone 2 (umd_resource.c). If it is a plain 2D
// colour texture of one level, its inner texture is made over a device
// surface: winemetal is asked, just before the inner device makes the
// texture, to put it on a new surface in this process's task and export it,
// and the surface's token goes into the allocation's description, so the
// kernel driver can hold the surface and show it when Windows flips to the
// allocation (allocation.c, flip.c). Its pixels are drawn by the GPU and
// shown from where they were drawn: no copy anywhere.
//
// A resource opened from another process (OpenResource) is made the same
// way the other way round: the token in the allocation's description is
// imported into this process's task (RuxnSurfaceImportNext in winemetal)
// and the inner texture is laid over it.
//
// The translation of the creation's flags follows Triton's
// tritonTranslateUsage (tritonResource.c, MIT, Copyright Turing Software
// LLC); the code is ours.

#include "umd_inner.h"

#define BYTES_PER_PIXEL		4
#define CUBE_FACES		6

typedef BOOL(WINAPI *SURFACE_IMPORT_NEXT)(ULONG64 token, UINT width, UINT height);

static UMD_RESOURCE *resource_of(D3D10DDI_HRESOURCE h)
{
	return h.pDrvPrivate;
}

static UINT bind_flags(UINT ddi)
{
	UINT api = 0;

	if (ddi & D3D10_DDI_BIND_VERTEX_BUFFER)
		api |= D3D11_BIND_VERTEX_BUFFER;
	if (ddi & D3D10_DDI_BIND_INDEX_BUFFER)
		api |= D3D11_BIND_INDEX_BUFFER;
	if (ddi & D3D10_DDI_BIND_CONSTANT_BUFFER)
		api |= D3D11_BIND_CONSTANT_BUFFER;
	if (ddi & D3D10_DDI_BIND_SHADER_RESOURCE)
		api |= D3D11_BIND_SHADER_RESOURCE;
	if (ddi & D3D10_DDI_BIND_STREAM_OUTPUT)
		api |= D3D11_BIND_STREAM_OUTPUT;
	if (ddi & D3D10_DDI_BIND_RENDER_TARGET)
		api |= D3D11_BIND_RENDER_TARGET;
	if (ddi & D3D10_DDI_BIND_DEPTH_STENCIL)
		api |= D3D11_BIND_DEPTH_STENCIL;
	if (ddi & D3D11_DDI_BIND_UNORDERED_ACCESS)
		api |= D3D11_BIND_UNORDERED_ACCESS;
	// A swap chain's buffer is drawn into and, by the compositor, sampled.
	if (ddi & D3D10_DDI_BIND_PRESENT)
		api |= D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
	return api;
}

// Sharing goes by the device surface's token, not by the inner device's
// own shared resources, so the shared flags are left out.
static UINT misc_flags(UINT ddi)
{
	UINT api = 0;

	if (ddi & D3D10_DDI_RESOURCE_AUTO_GEN_MIP_MAP)
		api |= D3D11_RESOURCE_MISC_GENERATE_MIPS;
	if (ddi & D3D11_DDI_RESOURCE_MISC_DRAWINDIRECT_ARGS)
		api |= D3D11_RESOURCE_MISC_DRAWINDIRECT_ARGS;
	if (ddi & D3D11_DDI_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS)
		api |= D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS;
	if (ddi & D3D11_DDI_RESOURCE_MISC_BUFFER_STRUCTURED)
		api |= D3D11_RESOURCE_MISC_BUFFER_STRUCTURED;
	if (ddi & D3D11_DDI_RESOURCE_MISC_RESOURCE_CLAMP)
		api |= D3D11_RESOURCE_MISC_RESOURCE_CLAMP;
	return api;
}

static UINT cpu_flags(UINT ddi)
{
	return ((ddi & D3D10_DDI_CPU_ACCESS_READ) ? D3D11_CPU_ACCESS_READ : 0) |
	       ((ddi & D3D10_DDI_CPU_ACCESS_WRITE) ? D3D11_CPU_ACCESS_WRITE : 0);
}

static BOOL needs_kernel(const UMD_RESOURCE *r)
{
	return r->primary || (r->misc & D3D10_DDI_RESOURCE_MISC_SHARED) || (r->bind & D3D10_DDI_BIND_PRESENT);
}

// What can lie over a device surface: one level, one slice, one sample,
// four bytes a pixel in blue, green, red, alpha order, as the surface
// descriptor says (pvg_surface.c); or one byte a pixel over a surface a
// quarter as wide (winemetal's wm_surface.c), which DirectComposition's
// shared glyph masks are (run rn15: A8, in nearly every process).
static BOOL fits_surface(const UMD_RESOURCE *r, const D3D11DDIARG_CREATERESOURCE *args)
{
	return r->type == D3D10DDIRESOURCE_TEXTURE2D && r->mip_levels == 1 && r->array_size == 1 &&
	       args->SampleDesc.Count <= 1 &&
	       (r->format == DXGI_FORMAT_B8G8R8A8_UNORM || r->format == DXGI_FORMAT_B8G8R8X8_UNORM ||
		r->format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB || r->format == DXGI_FORMAT_B8G8R8A8_TYPELESS ||
		r->format == DXGI_FORMAT_A8_UNORM || r->format == DXGI_FORMAT_R8_UNORM ||
		// RGBA8 shares the same four bytes a pixel, read the same way
		// by maker and opener; never a primary, which the display would
		// show with red and blue swapped (helper scene surface-rgba; run
		// rn30: Notepad's tab row is such a shared texture).
		(!r->primary && (r->format == DXGI_FORMAT_R8G8B8A8_UNORM ||
				 r->format == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB ||
				 r->format == DXGI_FORMAT_R8G8B8A8_TYPELESS)));
}

static HRESULT make_inner(UMD_DEVICE *dev, UMD_RESOURCE *r, const D3D11DDIARG_CREATERESOURCE *args)
{
	// The runtime's initial data has the API's layout, field for field.
	const D3D11_SUBRESOURCE_DATA *data = (const D3D11_SUBRESOURCE_DATA *)args->pInitialDataUP;
	D3D11_USAGE usage = (D3D11_USAGE)args->Usage;	// the DDI's values are the API's
	UINT bind = bind_flags(args->BindFlags), misc = misc_flags(args->MiscFlags);
	UINT cpu = cpu_flags(args->MapFlags);

	switch (r->type) {
	case D3D10DDIRESOURCE_BUFFER:
	case D3D11DDIRESOURCE_BUFFEREX: {
		D3D11_BUFFER_DESC d = { r->width, usage, bind, cpu, misc, args->ByteStride };

		return ID3D11Device1_CreateBuffer(dev->inner, &d, data, (ID3D11Buffer **)&r->inner);
	}
	case D3D10DDIRESOURCE_TEXTURE1D: {
		D3D11_TEXTURE1D_DESC d = { r->width, r->mip_levels, r->array_size, (DXGI_FORMAT)r->format,
					   usage, bind, cpu, misc };

		return ID3D11Device1_CreateTexture1D(dev->inner, &d, data, (ID3D11Texture1D **)&r->inner);
	}
	case D3D10DDIRESOURCE_TEXTURE2D:
	case D3D10DDIRESOURCE_TEXTURECUBE: {
		D3D11_TEXTURE2D_DESC d = { r->width, r->height, r->mip_levels, r->array_size,
					   (DXGI_FORMAT)r->format, args->SampleDesc, usage, bind, cpu, misc };

		if (r->type == D3D10DDIRESOURCE_TEXTURECUBE) {
			d.MiscFlags |= D3D11_RESOURCE_MISC_TEXTURECUBE;
			d.ArraySize = r->array_size ? r->array_size : CUBE_FACES;
		}
		return ID3D11Device1_CreateTexture2D(dev->inner, &d, data, (ID3D11Texture2D **)&r->inner);
	}
	case D3D10DDIRESOURCE_TEXTURE3D: {
		D3D11_TEXTURE3D_DESC d = { r->width, r->height, r->depth, r->mip_levels,
					   (DXGI_FORMAT)r->format, usage, bind, cpu, misc };

		return ID3D11Device1_CreateTexture3D(dev->inner, &d, data, (ID3D11Texture3D **)&r->inner);
	}
	default:
		return E_INVALIDARG;
	}
}

// One kernel allocation for the whole resource, naming its surface if it
// has one.
static HRESULT allocate(UMD_DEVICE *dev, UMD_RESOURCE *r, const D3D11DDIARG_CREATERESOURCE *args)
{
	struct ruxnwddm_alloc_info info;
	D3DDDI_ALLOCATIONINFO ai;
	D3DDDICB_ALLOCATE cb;
	HRESULT hr;

	umd_zero(&info, sizeof(info));
	info.magic = RUXNWDDM_UMD_MAGIC;
	info.kind = r->type == D3D10DDIRESOURCE_BUFFER ? RUXNWDDM_ALLOC_BUFFER : RUXNWDDM_ALLOC_TEXTURE;
	info.flags = (r->primary ? RUXNWDDM_ALLOC_PRIMARY : 0) |
		     ((r->misc & D3D10_DDI_RESOURCE_MISC_SHARED) ? RUXNWDDM_ALLOC_SHARED : 0) |
		     (r->surface_token ? RUXNWDDM_ALLOC_SURFACE : 0);
	info.format = r->format;
	info.width = r->width;
	info.height = r->height;
	info.pitch = RUXNWDDM_SURFACE_WIDTH(r->width) * BYTES_PER_PIXEL;
	info.size = r->bytes;
	info.surface_token = r->surface_token;

	umd_zero(&ai, sizeof(ai));
	ai.pPrivateDriverData = &info;
	ai.PrivateDriverDataSize = sizeof(info);
	if (r->primary && args->pPrimaryDesc) {
		ai.Flags.Primary = 1;
		ai.VidPnSourceId = args->pPrimaryDesc->VidPnSourceId;
	}
	umd_zero(&cb, sizeof(cb));
	cb.hResource = r->runtime.handle;
	cb.NumAllocations = 1;
	cb.pAllocationInfo = &ai;
	hr = dev->kt->pfnAllocateCb(dev->runtime.handle, &cb);
	if (SUCCEEDED(hr)) {
		r->allocation = ai.hAllocation;
		r->kernel_resource = cb.hKMResource;
		umd_make_resident(dev, r->allocation);
	}
	return hr;
}

static void APIENTRY create_resource(D3D10DDI_HDEVICE h, const D3D11DDIARG_CREATERESOURCE *args,
				     D3D10DDI_HRESOURCE hres, D3D10DDI_HRTRESOURCE rt)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_RESOURCE *r = resource_of(hres);
	const D3D10DDI_MIPINFO *mip = args->pMipInfoList;
	HRESULT hr;

	umd_zero(r, sizeof(*r));
	r->type = args->ResourceDimension;
	r->bind = args->BindFlags;
	r->misc = args->MiscFlags;
	r->usage = args->Usage;
	r->cpu_access = args->MapFlags;
	r->format = args->Format;
	r->runtime = rt;
	r->width = mip ? mip[0].TexelWidth : 0;
	r->height = mip && mip[0].TexelHeight ? mip[0].TexelHeight : 1;
	r->depth = mip && mip[0].TexelDepth ? mip[0].TexelDepth : 1;
	r->mip_levels = args->MipLevels ? args->MipLevels : 1;
	r->array_size = args->ArraySize ? args->ArraySize : 1;
	r->primary = args->pPrimaryDesc != NULL;
	r->bytes = (SIZE_T)r->width * r->height *
		   (r->type == D3D10DDIRESOURCE_BUFFER ? 1 : BYTES_PER_PIXEL);

	umd_lock(dev);
	// Asked just before the texture is made, in this thread: winemetal
	// takes the request with the next texture of this size.
	if (needs_kernel(r) && fits_surface(r, args) && umd_inner_surface_next(r->width, r->height)) {
		umd_inner_clear_failures();
		hr = umd_inner_checked(make_inner(dev, r, args), (void **)&r->inner);
		r->surface_token = umd_inner_surface_token();
	} else {
		umd_inner_clear_failures();
		hr = umd_inner_checked(make_inner(dev, r, args), (void **)&r->inner);
	}
	umd_unlock(dev);
	if (SUCCEEDED(hr) && needs_kernel(r))
		hr = allocate(dev, r, args);
	umd_log("CreateResource bind", r->bind);
	umd_log("CreateResource surface token", r->surface_token);
	// What another process will open without its pixels: format, levels,
	// size (run rn14: twelve such in dwm, made by whom not known).
	if (needs_kernel(r) && !r->surface_token)
		umd_log("CreateResource: needs the kernel, no surface: format, levels, width, height",
			(ULONG64)r->format << 48 | (ULONG64)r->mip_levels << 40 | (ULONG64)r->width << 20 | r->height);
	if (FAILED(hr)) {
		umd_error(dev, hr, "CreateResource failed");
		return;
	}
	// Creates come from any thread (free threaded, umd_adapter.c).
	InterlockedIncrement((volatile LONG *)&dev->resources);
	if (r->allocation)
		umd_note(dev, UMD_NOTE_CREATE_RESOURCE, (r->primary ? 2 : 1) | (r->surface_token ? 4 : 0));
}

// A resource another process made and shares: its allocation's description
// names the surface its pixels are, which this process imports.
static void APIENTRY open_resource(D3D10DDI_HDEVICE h, const D3D10DDIARG_OPENRESOURCE *args,
				   D3D10DDI_HRESOURCE hres, D3D10DDI_HRTRESOURCE rt)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_RESOURCE *r = resource_of(hres);
	const struct ruxnwddm_alloc_info *info = NULL;
	SURFACE_IMPORT_NEXT import_next;
	D3D11_TEXTURE2D_DESC d;
	HRESULT hr = E_INVALIDARG;

	umd_zero(r, sizeof(*r));
	r->runtime = rt;
	r->type = D3D10DDIRESOURCE_TEXTURE2D;
	r->mip_levels = r->array_size = r->depth = 1;
	if (args->NumAllocations && args->pOpenAllocationInfo) {
		r->allocation = args->pOpenAllocationInfo[0].hAllocation;
		if (args->pOpenAllocationInfo[0].PrivateDriverDataSize >= sizeof(*info))
			info = args->pOpenAllocationInfo[0].pPrivateDriverData;
	}
	r->kernel_resource = args->hKMResource.handle;
	r->opened = TRUE;
	if (!info || info->magic != RUXNWDDM_UMD_MAGIC) {
		umd_error(dev, hr, "OpenResource: not one of ours");
		return;
	}
	r->width = info->width;
	r->height = info->height;
	r->format = info->format;
	r->surface_token = (info->flags & RUXNWDDM_ALLOC_SURFACE) ? info->surface_token : 0;
	import_next = (SURFACE_IMPORT_NEXT)(void *)GetProcAddress(GetModuleHandleW(L"winemetal.dll"),
								 "RuxnSurfaceImportNext");
	umd_zero(&d, sizeof(d));
	d.Width = r->width;
	d.Height = r->height;
	d.MipLevels = d.ArraySize = d.SampleDesc.Count = 1;
	d.Format = (DXGI_FORMAT)r->format;
	d.Usage = D3D11_USAGE_DEFAULT;
	d.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
	// A resource with no surface (made by a process that does not draw
	// through the inner device, or in a form no surface takes) gets a
	// texture of its own: the opener goes on, but sees none of the
	// maker's pixels (run rn8: the compositor failed on such opens of
	// windows made before it drew this way).
	umd_lock(dev);
	umd_inner_clear_failures();
	if (!r->surface_token)
		hr = ID3D11Device1_CreateTexture2D(dev->inner, &d, NULL, (ID3D11Texture2D **)&r->inner);
	else if (import_next && import_next(r->surface_token, r->width, r->height))
		hr = ID3D11Device1_CreateTexture2D(dev->inner, &d, NULL, (ID3D11Texture2D **)&r->inner);
	hr = umd_inner_checked(hr, (void **)&r->inner);
	umd_unlock(dev);
	umd_log(r->surface_token ? "OpenResource surface token" : "OpenResource: no surface, a texture of its own",
		r->surface_token ? r->surface_token :
				   ((ULONG64)r->format << 48 | (ULONG64)r->width << 24 | r->height));
	if (FAILED(hr))
		umd_error(dev, hr, "OpenResource failed");
}

static void APIENTRY destroy_resource(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE hres)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_RESOURCE *r = resource_of(hres);
	D3DDDICB_DEALLOCATE cb;

	if (r->inner) {
		umd_lock(dev);
		ID3D11Resource_Release(r->inner);
		umd_unlock(dev);
		r->inner = NULL;
	}
	// Made or opened, the resource's allocations are let go here, by the
	// runtime's handle, as the milestone 2 path does (umd_resource.c).
	// Opened ones were once left to the runtime, which never closed them:
	// the compositor kept every window's shared allocations open after the
	// window's program had ended, each holding its surface's memory, and
	// after half an hour of use no new program could draw (runs rn63 to
	// rn65).
	if (r->kernel_resource && r->allocation && dev->kt->pfnDeallocateCb) {
		HRESULT hr;

		umd_zero(&cb, sizeof(cb));
		cb.hResource = r->runtime.handle;
		hr = dev->kt->pfnDeallocateCb(dev->runtime.handle, &cb);
		if (r->opened && FAILED(hr))
			umd_log("DestroyResource: an opened resource's deallocation failed", (ULONG64)(LONG)hr);
	}
}

// Map, in every flavour the runtime has: the inner context's own, whose
// memory is the buffer object the device reads, so nothing is copied.
static void APIENTRY map(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE hres, UINT sub, D3D10_DDI_MAP type,
			 UINT flags, D3D10DDI_MAPPED_SUBRESOURCE *mapped)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_RESOURCE *r = resource_of(hres);
	D3D11_MAPPED_SUBRESOURCE m;
	HRESULT hr;

	if (!r->inner) {
		umd_error(dev, E_INVALIDARG, "Map: no inner resource");
		return;
	}
	umd_lock(dev);
	// The DDI's map types and the do-not-wait flag have the API's values.
	hr = ID3D11DeviceContext1_Map(dev->context_inner, r->inner, sub, (D3D11_MAP)type, flags, &m);
	umd_unlock(dev);
	if (SUCCEEDED(hr) && umd_inner_work_lost(dev)) {
		umd_lock(dev);
		ID3D11DeviceContext1_Unmap(dev->context_inner, r->inner, sub);
		umd_unlock(dev);
		mapped->pData = NULL;
		return;
	}
	if (FAILED(hr)) {
		if (hr != DXGI_ERROR_WAS_STILL_DRAWING)
			umd_log("Map failed", (ULONG)hr);
		dev->core->pfnSetErrorCb(dev->core_layer, hr);
		return;
	}
	mapped->pData = m.pData;
	mapped->RowPitch = m.RowPitch;
	mapped->DepthPitch = m.DepthPitch;
}

static void APIENTRY unmap(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE hres, UINT sub)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_RESOURCE *r = resource_of(hres);

	if (!r->inner)
		return;
	umd_lock(dev);
	ID3D11DeviceContext1_Unmap(dev->context_inner, r->inner, sub);
	umd_unlock(dev);
}

// Whether a staging resource's copy is still under way: the inner device
// is asked without waiting.
static BOOL APIENTRY is_staging_busy(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE hres)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(hres);
	return FALSE;	// every submit is waited for in winemetal
}

void umd_fill_resources(D3DWDDM2_0DDI_DEVICEFUNCS *f)
{
	f->pfnCreateResource = create_resource;
	f->pfnOpenResource = open_resource;
	f->pfnDestroyResource = destroy_resource;
	f->pfnResourceMap = map;
	f->pfnStagingResourceMap = map;
	f->pfnDynamicResourceMapDiscard = map;
	f->pfnDynamicIABufferMapDiscard = map;
	f->pfnDynamicIABufferMapNoOverwrite = map;
	f->pfnDynamicConstantBufferMapDiscard = map;
	f->pfnDynamicConstantBufferMapNoOverwrite = map;
	f->pfnResourceUnmap = unmap;
	f->pfnStagingResourceUnmap = unmap;
	f->pfnDynamicResourceUnmap = unmap;
	f->pfnDynamicIABufferUnmap = unmap;
	f->pfnDynamicConstantBufferUnmap = unmap;
	f->pfnResourceIsStagingBusy = is_staging_busy;
}
