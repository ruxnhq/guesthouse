// SPDX-License-Identifier: MIT
//
// Resources of ruxnumd.dll. A resource Windows must see in the kernel (one
// that is presented, shared between processes, or scanned out) gets one
// kernel allocation through the runtime's AllocateCb, described by a
// struct ruxnwddm_alloc_info the kernel driver reads (allocation.c). Every
// resource that can be mapped gets CPU memory of its own for Map, because
// the compositor writes its constant and vertex buffers that way. Nothing
// here reaches the device yet.

#include "umd.h"

#define BYTES_PER_PIXEL		4
// D3D10_DDI_RESOURCE_MISC_SHARED and the bind flag for presentable
// resources, as d3d10umddi.h has them.
#define NEEDS_KERNEL(bind, misc, primary) \
	((primary) || ((misc) & D3D10_DDI_RESOURCE_MISC_SHARED) || ((bind) & D3D10_DDI_BIND_PRESENT))

static UMD_RESOURCE *resource_of(D3D10DDI_HRESOURCE h)
{
	return h.pDrvPrivate;
}

SIZE_T APIENTRY umd_calc_resource_size(D3D10DDI_HDEVICE h, const D3D11DDIARG_CREATERESOURCE *args)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(args);
	return sizeof(UMD_RESOURCE);
}

SIZE_T APIENTRY umd_calc_open_resource_size(D3D10DDI_HDEVICE h, const D3D10DDIARG_OPENRESOURCE *args)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(args);
	return sizeof(UMD_RESOURCE);
}

// One kernel allocation for the whole resource.
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
		     ((r->misc & D3D10_DDI_RESOURCE_MISC_SHARED) ? RUXNWDDM_ALLOC_SHARED : 0);
	info.format = r->format;
	info.width = r->width;
	info.height = r->height;
	info.pitch = RUXNWDDM_SURFACE_WIDTH(r->width) * BYTES_PER_PIXEL;
	info.size = r->bytes;

	umd_zero(&ai, sizeof(ai));
	ai.pPrivateDriverData = &info;
	ai.PrivateDriverDataSize = sizeof(info);
	if (r->primary) {
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

void APIENTRY umd_create_resource(D3D10DDI_HDEVICE h, const D3D11DDIARG_CREATERESOURCE *args,
				  D3D10DDI_HRESOURCE hr_res, D3D10DDI_HRTRESOURCE rt)
{
	UMD_DEVICE *dev = h.pDrvPrivate;
	UMD_RESOURCE *r = resource_of(hr_res);
	const D3D10DDI_MIPINFO *mip = args->pMipInfoList;
	HRESULT hr = S_OK;

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
	r->primary = args->pPrimaryDesc != NULL;
	// Every element is taken as four bytes or less, and buffers as bytes:
	// enough room for whatever the compositor maps.
	r->bytes = (SIZE_T)r->width * r->height *
		   (r->type == D3D10DDIRESOURCE_BUFFER ? 1 : BYTES_PER_PIXEL) * max(args->ArraySize, 1);
	if (!r->bytes)
		r->bytes = BYTES_PER_PIXEL;

	if (args->MapFlags || args->Usage == D3D10_DDI_USAGE_DYNAMIC ||
	    args->Usage == D3D10_DDI_USAGE_STAGING) {
		r->cpu = umd_alloc(r->bytes);
		if (!r->cpu)
			hr = E_OUTOFMEMORY;
	}
	if (SUCCEEDED(hr) && NEEDS_KERNEL(r->bind, r->misc, r->primary))
		hr = allocate(dev, r, args);
	umd_log("CreateResource bind", r->bind);
	umd_log("CreateResource result", (ULONG)hr);
	if (FAILED(hr)) {
		umd_note(dev, UMD_NOTE_FAILED, (unsigned int)hr);
		dev->core->pfnSetErrorCb(dev->core_layer, hr);
		return;
	}
	// Creates come from any thread (free threaded, umd_adapter.c).
	InterlockedIncrement((volatile LONG *)&dev->resources);
	if (r->primary || r->allocation)
		umd_note(dev, UMD_NOTE_CREATE_RESOURCE, r->primary ? 2 : 1);
}

// A resource another process shares with us: the kernel allocations are
// opened by the runtime; keep the first handle for presents.
void APIENTRY umd_open_resource(D3D10DDI_HDEVICE h, const D3D10DDIARG_OPENRESOURCE *args,
				D3D10DDI_HRESOURCE hr_res, D3D10DDI_HRTRESOURCE rt)
{
	UMD_RESOURCE *r = resource_of(hr_res);

	UNREFERENCED_PARAMETER(h);
	umd_zero(r, sizeof(*r));
	r->runtime = rt;
	r->type = D3D10DDIRESOURCE_TEXTURE2D;
	if (args->NumAllocations && args->pOpenAllocationInfo)
		r->allocation = args->pOpenAllocationInfo[0].hAllocation;
	r->kernel_resource = args->hKMResource.handle;
}

void APIENTRY umd_destroy_resource(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE hr_res)
{
	UMD_DEVICE *dev = h.pDrvPrivate;
	UMD_RESOURCE *r = resource_of(hr_res);

	if (r->allocation && r->kernel_resource == 0 && dev->kt->pfnDeallocateCb) {
		D3DDDICB_DEALLOCATE cb;

		umd_zero(&cb, sizeof(cb));
		cb.NumAllocations = 1;
		cb.HandleList = &r->allocation;
		dev->kt->pfnDeallocateCb(dev->runtime.handle, &cb);
	} else if (r->kernel_resource && dev->kt->pfnDeallocateCb) {
		D3DDDICB_DEALLOCATE cb;

		umd_zero(&cb, sizeof(cb));
		cb.hResource = r->runtime.handle;
		dev->kt->pfnDeallocateCb(dev->runtime.handle, &cb);
	}
	umd_free(r->cpu);
	r->cpu = NULL;
}

void APIENTRY umd_map(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE hr_res, UINT sub, D3D10_DDI_MAP type,
		      UINT flags, D3D10DDI_MAPPED_SUBRESOURCE *mapped)
{
	UMD_DEVICE *dev = h.pDrvPrivate;
	UMD_RESOURCE *r = resource_of(hr_res);

	UNREFERENCED_PARAMETER(sub);
	UNREFERENCED_PARAMETER(type);
	UNREFERENCED_PARAMETER(flags);
	if (!r->cpu) {
		r->cpu = umd_alloc(r->bytes);
		if (!r->cpu) {
			dev->core->pfnSetErrorCb(dev->core_layer, E_OUTOFMEMORY);
			return;
		}
	}
	mapped->pData = r->cpu;
	mapped->RowPitch = r->type == D3D10DDIRESOURCE_BUFFER ? (UINT)r->bytes : r->width * BYTES_PER_PIXEL;
	mapped->DepthPitch = (UINT)r->bytes;
}

void APIENTRY umd_unmap(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE hr_res, UINT sub)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(hr_res);
	UNREFERENCED_PARAMETER(sub);
}
