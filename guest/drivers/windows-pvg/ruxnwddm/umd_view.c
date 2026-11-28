// SPDX-License-Identifier: MIT
//
// Views of the drawing device: render target, depth-stencil and shader
// resource views as inner views, their binding, and the clears that name
// them. Unordered access views are not offered at feature level 10.1.
//
// Each view keeps the API description it was made with and is listed on its
// resource, because a swap chain's rotation hands a resource another inner
// resource, and the view must then be made again on that one
// (umd_present.c), as Triton does (tritonDxgi.c, MIT, Copyright Turing
// Software LLC).

#include "umd_inner.h"

static UMD_VIEW *view_of(void *h)
{
	return h;
}

static UMD_RESOURCE *resource_of(D3D10DDI_HRESOURCE h)
{
	return h.pDrvPrivate;
}

static BOOL is_array(const UMD_RESOURCE *r, UINT first, UINT size)
{
	return r->array_size > 1 || first || size > 1;
}

static void link_view(UMD_VIEW *v, UMD_RESOURCE *r, UINT kind)
{
	v->kind = kind;
	v->resource = r;
	v->next = r->views;
	r->views = v;
}

static void unlink_view(UMD_VIEW *v)
{
	UMD_VIEW **at;

	if (!v->resource)
		return;
	for (at = &v->resource->views; *at; at = &(*at)->next) {
		if (*at == v) {
			*at = v->next;
			break;
		}
	}
	v->resource = NULL;
}

// Makes the inner view from the description kept; also after a rotation.
HRESULT umd_view_remake(UMD_DEVICE *dev, UMD_VIEW *v)
{
	ID3D11Resource *inner = v->resource ? v->resource->inner : NULL;
	HRESULT hr;

	if (v->inner) {
		ID3D11View_Release(v->inner);
		v->inner = NULL;
	}
	if (!inner)
		return E_INVALIDARG;
	umd_inner_clear_failures();
	switch (v->kind) {
	case UMD_VIEW_RENDER_TARGET:
		hr = ID3D11Device1_CreateRenderTargetView(dev->inner, inner, &v->desc.rtv,
							  (ID3D11RenderTargetView **)&v->inner);
		break;
	case UMD_VIEW_DEPTH_STENCIL:
		hr = ID3D11Device1_CreateDepthStencilView(dev->inner, inner, &v->desc.dsv,
							  (ID3D11DepthStencilView **)&v->inner);
		break;
	case UMD_VIEW_UNORDERED_ACCESS:
		hr = ID3D11Device1_CreateUnorderedAccessView(dev->inner, inner, &v->desc.uav,
							     (ID3D11UnorderedAccessView **)&v->inner);
		break;
	default:
		hr = ID3D11Device1_CreateShaderResourceView(dev->inner, inner, &v->desc.srv,
							    (ID3D11ShaderResourceView **)&v->inner);
		break;
	}
	return umd_inner_checked(hr, (void **)&v->inner);
}

static SIZE_T APIENTRY view_size(D3D10DDI_HDEVICE h, const void *args)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(args);
	return sizeof(UMD_VIEW);
}

static void make(UMD_DEVICE *dev, UMD_VIEW *v, UMD_RESOURCE *r, UINT kind, const char *what)
{
	HRESULT hr;

	// Under the lock: creates come from any thread (free threaded,
	// umd_adapter.c) while a present walks the resource's views.
	umd_lock(dev);
	link_view(v, r, kind);
	hr = umd_view_remake(dev, v);
	umd_unlock(dev);
	if (FAILED(hr))
		umd_error(dev, hr, what);
}

static void APIENTRY create_rtv(D3D10DDI_HDEVICE h, const D3DWDDM2_0DDIARG_CREATERENDERTARGETVIEW *args,
			       D3D10DDI_HRENDERTARGETVIEW hv, D3D10DDI_HRTRENDERTARGETVIEW rt)
{
	UMD_VIEW *v = view_of(hv.pDrvPrivate);
	UMD_RESOURCE *r = resource_of(args->hDrvResource);
	D3D11_RENDER_TARGET_VIEW_DESC *d = &v->desc.rtv;

	UNREFERENCED_PARAMETER(rt);
	umd_zero(v, sizeof(*v));
	d->Format = args->Format;
	switch (args->ResourceDimension) {
	case D3D10DDIRESOURCE_BUFFER:
		d->ViewDimension = D3D11_RTV_DIMENSION_BUFFER;
		d->Buffer.FirstElement = args->Buffer.FirstElement;
		d->Buffer.NumElements = args->Buffer.NumElements;
		break;
	case D3D10DDIRESOURCE_TEXTURE2D:
	case D3D10DDIRESOURCE_TEXTURECUBE:
		if (is_array(r, args->Tex2D.FirstArraySlice, args->Tex2D.ArraySize)) {
			d->ViewDimension = D3D11_RTV_DIMENSION_TEXTURE2DARRAY;
			d->Texture2DArray.MipSlice = args->Tex2D.MipSlice;
			d->Texture2DArray.FirstArraySlice = args->Tex2D.FirstArraySlice;
			d->Texture2DArray.ArraySize = args->Tex2D.ArraySize;
		} else {
			d->ViewDimension = D3D11_RTV_DIMENSION_TEXTURE2D;
			d->Texture2D.MipSlice = args->Tex2D.MipSlice;
		}
		break;
	case D3D10DDIRESOURCE_TEXTURE1D:
		d->ViewDimension = D3D11_RTV_DIMENSION_TEXTURE1DARRAY;
		d->Texture1DArray.MipSlice = args->Tex1D.MipSlice;
		d->Texture1DArray.FirstArraySlice = args->Tex1D.FirstArraySlice;
		d->Texture1DArray.ArraySize = args->Tex1D.ArraySize;
		break;
	case D3D10DDIRESOURCE_TEXTURE3D:
		d->ViewDimension = D3D11_RTV_DIMENSION_TEXTURE3D;
		d->Texture3D.MipSlice = args->Tex3D.MipSlice;
		d->Texture3D.FirstWSlice = args->Tex3D.FirstW;
		d->Texture3D.WSize = args->Tex3D.WSize;
		break;
	default:
		break;
	}
	make(umd_device(h), v, r, UMD_VIEW_RENDER_TARGET, "CreateRenderTargetView failed");
}

static void APIENTRY create_dsv(D3D10DDI_HDEVICE h, const D3D11DDIARG_CREATEDEPTHSTENCILVIEW *args,
			       D3D10DDI_HDEPTHSTENCILVIEW hv, D3D10DDI_HRTDEPTHSTENCILVIEW rt)
{
	UMD_VIEW *v = view_of(hv.pDrvPrivate);
	UMD_RESOURCE *r = resource_of(args->hDrvResource);
	D3D11_DEPTH_STENCIL_VIEW_DESC *d = &v->desc.dsv;

	UNREFERENCED_PARAMETER(rt);
	umd_zero(v, sizeof(*v));
	d->Format = args->Format;
	d->Flags = args->Flags;	// read-only depth and stencil: the API's bits
	if (is_array(r, args->Tex2D.FirstArraySlice, args->Tex2D.ArraySize)) {
		d->ViewDimension = D3D11_DSV_DIMENSION_TEXTURE2DARRAY;
		d->Texture2DArray.MipSlice = args->Tex2D.MipSlice;
		d->Texture2DArray.FirstArraySlice = args->Tex2D.FirstArraySlice;
		d->Texture2DArray.ArraySize = args->Tex2D.ArraySize;
	} else {
		d->ViewDimension = D3D11_DSV_DIMENSION_TEXTURE2D;
		d->Texture2D.MipSlice = args->Tex2D.MipSlice;
	}
	make(umd_device(h), v, r, UMD_VIEW_DEPTH_STENCIL, "CreateDepthStencilView failed");
}

static void APIENTRY create_srv(D3D10DDI_HDEVICE h, const D3DWDDM2_0DDIARG_CREATESHADERRESOURCEVIEW *args,
			       D3D10DDI_HSHADERRESOURCEVIEW hv, D3D10DDI_HRTSHADERRESOURCEVIEW rt)
{
	UMD_VIEW *v = view_of(hv.pDrvPrivate);
	UMD_RESOURCE *r = resource_of(args->hDrvResource);
	D3D11_SHADER_RESOURCE_VIEW_DESC *d = &v->desc.srv;

	UNREFERENCED_PARAMETER(rt);
	umd_zero(v, sizeof(*v));
	d->Format = args->Format;
	switch (args->ResourceDimension) {
	case D3D10DDIRESOURCE_BUFFER:
		d->ViewDimension = D3D11_SRV_DIMENSION_BUFFER;
		d->Buffer.FirstElement = args->Buffer.FirstElement;
		d->Buffer.NumElements = args->Buffer.NumElements;
		break;
	case D3D11DDIRESOURCE_BUFFEREX:
		d->ViewDimension = D3D11_SRV_DIMENSION_BUFFEREX;
		d->BufferEx.FirstElement = args->BufferEx.FirstElement;
		d->BufferEx.NumElements = args->BufferEx.NumElements;
		d->BufferEx.Flags = args->BufferEx.Flags;
		break;
	case D3D10DDIRESOURCE_TEXTURE1D:
		d->ViewDimension = D3D11_SRV_DIMENSION_TEXTURE1DARRAY;
		d->Texture1DArray.MostDetailedMip = args->Tex1D.MostDetailedMip;
		d->Texture1DArray.MipLevels = args->Tex1D.MipLevels;
		d->Texture1DArray.FirstArraySlice = args->Tex1D.FirstArraySlice;
		d->Texture1DArray.ArraySize = args->Tex1D.ArraySize;
		break;
	case D3D10DDIRESOURCE_TEXTURE2D:
		if (is_array(r, args->Tex2D.FirstArraySlice, args->Tex2D.ArraySize)) {
			d->ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2DARRAY;
			d->Texture2DArray.MostDetailedMip = args->Tex2D.MostDetailedMip;
			d->Texture2DArray.MipLevels = args->Tex2D.MipLevels;
			d->Texture2DArray.FirstArraySlice = args->Tex2D.FirstArraySlice;
			d->Texture2DArray.ArraySize = args->Tex2D.ArraySize;
		} else {
			d->ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2D;
			d->Texture2D.MostDetailedMip = args->Tex2D.MostDetailedMip;
			d->Texture2D.MipLevels = args->Tex2D.MipLevels;
		}
		break;
	case D3D10DDIRESOURCE_TEXTURE3D:
		d->ViewDimension = D3D11_SRV_DIMENSION_TEXTURE3D;
		d->Texture3D.MostDetailedMip = args->Tex3D.MostDetailedMip;
		d->Texture3D.MipLevels = args->Tex3D.MipLevels;
		break;
	case D3D10DDIRESOURCE_TEXTURECUBE:
		d->ViewDimension = D3D11_SRV_DIMENSION_TEXTURECUBE;
		d->TextureCube.MostDetailedMip = args->TexCube.MostDetailedMip;
		d->TextureCube.MipLevels = args->TexCube.MipLevels;
		break;
	}
	make(umd_device(h), v, r, UMD_VIEW_SHADER_RESOURCE, "CreateShaderResourceView failed");
}

static void destroy(D3D10DDI_HDEVICE h, UMD_VIEW *v)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	for (UINT i = 0; i < dev->bound_rtv_count; i++)
		if (dev->bound_rtvs[i] == v)
			dev->bound_rtvs[i] = NULL;
	if (dev->bound_dsv == v)
		dev->bound_dsv = NULL;
	unlink_view(v);
	if (v->inner) {
		ID3D11View_Release(v->inner);
		v->inner = NULL;
	}
	umd_unlock(dev);
}

static void APIENTRY destroy_rtv(D3D10DDI_HDEVICE h, D3D10DDI_HRENDERTARGETVIEW hv)
{
	destroy(h, view_of(hv.pDrvPrivate));
}

static void APIENTRY destroy_dsv(D3D10DDI_HDEVICE h, D3D10DDI_HDEPTHSTENCILVIEW hv)
{
	destroy(h, view_of(hv.pDrvPrivate));
}

static void APIENTRY destroy_srv(D3D10DDI_HDEVICE h, D3D10DDI_HSHADERRESOURCEVIEW hv)
{
	destroy(h, view_of(hv.pDrvPrivate));
}

static ID3D11View *inner_of(const void *h)
{
	const UMD_VIEW *v = h;

	return v ? v->inner : NULL;
}

static void APIENTRY set_render_targets(D3D10DDI_HDEVICE h, const D3D10DDI_HRENDERTARGETVIEW *rtvs, UINT count,
				       UINT clear_slots, D3D10DDI_HDEPTHSTENCILVIEW dsv,
				       const D3D11DDI_HUNORDEREDACCESSVIEW *uavs, const UINT *counts,
				       UINT uav_start, UINT uav_count, UINT range_start, UINT range_size)
{
	UMD_DEVICE *dev = umd_device(h);
	ID3D11RenderTargetView *views[D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT];
	UINT i;

	UNREFERENCED_PARAMETER(clear_slots);
	UNREFERENCED_PARAMETER(uavs);
	UNREFERENCED_PARAMETER(counts);
	UNREFERENCED_PARAMETER(uav_start);
	UNREFERENCED_PARAMETER(uav_count);
	UNREFERENCED_PARAMETER(range_start);
	UNREFERENCED_PARAMETER(range_size);
	if (count > D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT)
		count = D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT;
	umd_lock(dev);
	for (i = 0; i < count; i++)
		dev->bound_rtvs[i] = view_of(rtvs[i].pDrvPrivate);
	dev->bound_rtv_count = count;
	dev->bound_dsv = view_of(dsv.pDrvPrivate);
	for (i = 0; i < count; i++)
		views[i] = (ID3D11RenderTargetView *)inner_of(rtvs[i].pDrvPrivate);
	ID3D11DeviceContext1_OMSetRenderTargets(dev->context_inner, count, views,
						(ID3D11DepthStencilView *)inner_of(dsv.pDrvPrivate));
	umd_unlock(dev);
}

// After a rotation the bound views have new inner views (umd_view_remake)
// and the inner context still holds the old ones, which name the textures
// before the rotation (run rn44: a flip-model swap chain's passes lost
// their colour target after the first present). Under the device's lock.
void umd_view_rebind(UMD_DEVICE *dev)
{
	ID3D11RenderTargetView *views[D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT];
	UINT i;

	for (i = 0; i < dev->bound_rtv_count; i++)
		views[i] = (ID3D11RenderTargetView *)inner_of(dev->bound_rtvs[i]);
	ID3D11DeviceContext1_OMSetRenderTargets(dev->context_inner, dev->bound_rtv_count, views,
						(ID3D11DepthStencilView *)inner_of(dev->bound_dsv));
}

// The shader resources of one stage.
static void set_srvs(D3D10DDI_HDEVICE h, UINT stage, UINT start, UINT count,
		     const D3D10DDI_HSHADERRESOURCEVIEW *srvs)
{
	UMD_DEVICE *dev = umd_device(h);
	ID3D11ShaderResourceView *views[D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT];
	UINT i;

	if (start >= D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT)
		return;
	if (count > D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT - start)
		count = D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT - start;
	for (i = 0; i < count; i++)
		views[i] = (ID3D11ShaderResourceView *)inner_of(srvs[i].pDrvPrivate);
	umd_lock(dev);
	if (stage == UMD_STAGE_VERTEX)
		ID3D11DeviceContext1_VSSetShaderResources(dev->context_inner, start, count, views);
	else if (stage == UMD_STAGE_GEOMETRY)
		ID3D11DeviceContext1_GSSetShaderResources(dev->context_inner, start, count, views);
	else if (stage == UMD_STAGE_COMPUTE)
		ID3D11DeviceContext1_CSSetShaderResources(dev->context_inner, start, count, views);
	else
		ID3D11DeviceContext1_PSSetShaderResources(dev->context_inner, start, count, views);
	umd_unlock(dev);
}

static void APIENTRY vs_srvs(D3D10DDI_HDEVICE h, UINT start, UINT count, const D3D10DDI_HSHADERRESOURCEVIEW *v)
{
	set_srvs(h, UMD_STAGE_VERTEX, start, count, v);
}

static void APIENTRY gs_srvs(D3D10DDI_HDEVICE h, UINT start, UINT count, const D3D10DDI_HSHADERRESOURCEVIEW *v)
{
	set_srvs(h, UMD_STAGE_GEOMETRY, start, count, v);
}

static void APIENTRY ps_srvs(D3D10DDI_HDEVICE h, UINT start, UINT count, const D3D10DDI_HSHADERRESOURCEVIEW *v)
{
	set_srvs(h, UMD_STAGE_PIXEL, start, count, v);
}

static void APIENTRY clear_rtv(D3D10DDI_HDEVICE h, D3D10DDI_HRENDERTARGETVIEW hv, FLOAT colour[4])
{
	UMD_DEVICE *dev = umd_device(h);
	ID3D11View *v = inner_of(hv.pDrvPrivate);

	if (!v)
		return;
	umd_lock(dev);
	ID3D11DeviceContext1_ClearRenderTargetView(dev->context_inner, (ID3D11RenderTargetView *)v, colour);
	umd_unlock(dev);
}

static void APIENTRY clear_dsv(D3D10DDI_HDEVICE h, D3D10DDI_HDEPTHSTENCILVIEW hv, UINT flags, FLOAT depth,
			      UINT8 stencil)
{
	UMD_DEVICE *dev = umd_device(h);
	ID3D11View *v = inner_of(hv.pDrvPrivate);

	if (!v)
		return;
	umd_lock(dev);
	// D3D10_DDI_CLEAR_DEPTH and _STENCIL have the API's values.
	ID3D11DeviceContext1_ClearDepthStencilView(dev->context_inner, (ID3D11DepthStencilView *)v, flags, depth,
						   stencil);
	umd_unlock(dev);
}

// ClearView names a view of any kind by its handle; a render target view
// is the kind the compositor clears.
static void APIENTRY clear_view(D3D10DDI_HDEVICE h, D3D11DDI_HANDLETYPE type, void *hv, const FLOAT colour[4],
			       const D3D10_DDI_RECT *rects, UINT count)
{
	UMD_DEVICE *dev = umd_device(h);
	ID3D11View *v = inner_of(hv);

	UNREFERENCED_PARAMETER(type);
	if (!v)
		return;
	umd_lock(dev);
	ID3D11DeviceContext1_ClearView(dev->context_inner, v, colour, (const D3D11_RECT *)rects, count);
	umd_unlock(dev);
}

// Unordered access views (feature level 11): the DDI's description is the
// API's, field for field.
static void APIENTRY create_uav(D3D10DDI_HDEVICE h, const D3DWDDM2_0DDIARG_CREATEUNORDEREDACCESSVIEW *args,
				D3D11DDI_HUNORDEREDACCESSVIEW hv, D3D11DDI_HRTUNORDEREDACCESSVIEW rt)
{
	UMD_VIEW *v = view_of(hv.pDrvPrivate);
	UMD_RESOURCE *r = resource_of(args->hDrvResource);
	D3D11_UNORDERED_ACCESS_VIEW_DESC *d = &v->desc.uav;

	UNREFERENCED_PARAMETER(rt);
	umd_zero(v, sizeof(*v));
	d->Format = args->Format;
	switch (args->ResourceDimension) {
	case D3D10DDIRESOURCE_BUFFER:
	case D3D11DDIRESOURCE_BUFFEREX:
		d->ViewDimension = D3D11_UAV_DIMENSION_BUFFER;
		d->Buffer.FirstElement = args->Buffer.FirstElement;
		d->Buffer.NumElements = args->Buffer.NumElements;
		d->Buffer.Flags = args->Buffer.Flags;
		break;
	case D3D10DDIRESOURCE_TEXTURE1D:
		d->ViewDimension = D3D11_UAV_DIMENSION_TEXTURE1DARRAY;
		d->Texture1DArray.MipSlice = args->Tex1D.MipSlice;
		d->Texture1DArray.FirstArraySlice = args->Tex1D.FirstArraySlice;
		d->Texture1DArray.ArraySize = args->Tex1D.ArraySize;
		break;
	case D3D10DDIRESOURCE_TEXTURE2D:
		if (is_array(r, args->Tex2D.FirstArraySlice, args->Tex2D.ArraySize)) {
			d->ViewDimension = D3D11_UAV_DIMENSION_TEXTURE2DARRAY;
			d->Texture2DArray.MipSlice = args->Tex2D.MipSlice;
			d->Texture2DArray.FirstArraySlice = args->Tex2D.FirstArraySlice;
			d->Texture2DArray.ArraySize = args->Tex2D.ArraySize;
		} else {
			d->ViewDimension = D3D11_UAV_DIMENSION_TEXTURE2D;
			d->Texture2D.MipSlice = args->Tex2D.MipSlice;
		}
		break;
	case D3D10DDIRESOURCE_TEXTURE3D:
		d->ViewDimension = D3D11_UAV_DIMENSION_TEXTURE3D;
		d->Texture3D.MipSlice = args->Tex3D.MipSlice;
		d->Texture3D.FirstWSlice = args->Tex3D.FirstW;
		d->Texture3D.WSize = args->Tex3D.WSize;
		break;
	case D3D10DDIRESOURCE_TEXTURECUBE:
		// Never sent for an unordered access view, says the DDI.
		break;
	}
	make(umd_device(h), v, r, UMD_VIEW_UNORDERED_ACCESS, "CreateUnorderedAccessView failed");
}

static void APIENTRY destroy_uav(D3D10DDI_HDEVICE h, D3D11DDI_HUNORDEREDACCESSVIEW hv)
{
	destroy(h, view_of(hv.pDrvPrivate));
}

static void APIENTRY clear_uav_uint(D3D10DDI_HDEVICE h, D3D11DDI_HUNORDEREDACCESSVIEW hv, const UINT values[4])
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_VIEW *v = view_of(hv.pDrvPrivate);

	if (!v->inner)
		return;
	umd_lock(dev);
	ID3D11DeviceContext1_ClearUnorderedAccessViewUint(dev->context_inner, (ID3D11UnorderedAccessView *)v->inner,
							  values);
	umd_unlock(dev);
}

static void APIENTRY clear_uav_float(D3D10DDI_HDEVICE h, D3D11DDI_HUNORDEREDACCESSVIEW hv, const FLOAT values[4])
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_VIEW *v = view_of(hv.pDrvPrivate);

	if (!v->inner)
		return;
	umd_lock(dev);
	ID3D11DeviceContext1_ClearUnorderedAccessViewFloat(dev->context_inner, (ID3D11UnorderedAccessView *)v->inner,
							   values);
	umd_unlock(dev);
}

static void APIENTRY cs_uavs(D3D10DDI_HDEVICE h, UINT start, UINT count, const D3D11DDI_HUNORDEREDACCESSVIEW *uavs,
			     const UINT *initial)
{
	UMD_DEVICE *dev = umd_device(h);
	ID3D11UnorderedAccessView *views[D3D11_1_UAV_SLOT_COUNT];
	UINT i;

	if (start >= D3D11_1_UAV_SLOT_COUNT)
		return;
	if (count > D3D11_1_UAV_SLOT_COUNT - start)
		count = D3D11_1_UAV_SLOT_COUNT - start;
	for (i = 0; i < count; i++)
		views[i] = (ID3D11UnorderedAccessView *)inner_of(uavs[i].pDrvPrivate);
	umd_lock(dev);
	ID3D11DeviceContext1_CSSetUnorderedAccessViews(dev->context_inner, start, count, views, initial);
	umd_unlock(dev);
}

static void APIENTRY cs_srvs(D3D10DDI_HDEVICE h, UINT start, UINT count, const D3D10DDI_HSHADERRESOURCEVIEW *v)
{
	set_srvs(h, UMD_STAGE_COMPUTE, start, count, v);
}

void umd_fill_views(D3DWDDM2_0DDI_DEVICEFUNCS *f)
{
	f->pfnCalcPrivateRenderTargetViewSize = (void *)view_size;
	f->pfnCreateRenderTargetView = create_rtv;
	f->pfnDestroyRenderTargetView = destroy_rtv;
	f->pfnCalcPrivateDepthStencilViewSize = (void *)view_size;
	f->pfnCreateDepthStencilView = create_dsv;
	f->pfnDestroyDepthStencilView = destroy_dsv;
	f->pfnCalcPrivateShaderResourceViewSize = (void *)view_size;
	f->pfnCreateShaderResourceView = create_srv;
	f->pfnDestroyShaderResourceView = destroy_srv;
	f->pfnSetRenderTargets = set_render_targets;
	f->pfnVsSetShaderResources = vs_srvs;
	f->pfnGsSetShaderResources = gs_srvs;
	f->pfnPsSetShaderResources = ps_srvs;
	f->pfnClearRenderTargetView = clear_rtv;
	f->pfnClearDepthStencilView = clear_dsv;
	f->pfnClearView = clear_view;
	f->pfnCalcPrivateUnorderedAccessViewSize = (void *)view_size;
	f->pfnCreateUnorderedAccessView = create_uav;
	f->pfnDestroyUnorderedAccessView = destroy_uav;
	f->pfnClearUnorderedAccessViewUint = clear_uav_uint;
	f->pfnClearUnorderedAccessViewFloat = clear_uav_float;
	f->pfnCsSetUnorderedAccessViews = cs_uavs;
	f->pfnCsSetShaderResources = cs_srvs;
}
