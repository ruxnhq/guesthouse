// SPDX-License-Identifier: MIT
//
// The drawing device's work: draws, copies, uploads, mipmaps, resolves,
// flushes and queries, each the inner context's own call.
//
// Queries are answered at once. Every command buffer winemetal commits is
// waited for, so once the inner context is flushed all work before a query
// is done: an event query flushes and says done; a timestamp is the
// performance counter's time in nanoseconds, never disjoint; occlusion and
// statistics queries count nothing.

#include "umd_inner.h"

#define NANOSECONDS		1000000000ull

static UMD_RESOURCE *resource_of(D3D10DDI_HRESOURCE h)
{
	return h.pDrvPrivate;
}

static ID3D11Resource *inner_of(D3D10DDI_HRESOURCE h)
{
	UMD_RESOURCE *r = resource_of(h);

	return r ? r->inner : NULL;
}

static void APIENTRY draw(D3D10DDI_HDEVICE h, UINT count, UINT start)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	ID3D11DeviceContext1_Draw(dev->context_inner, count, start);
	dev->draws++;
	umd_unlock(dev);
}

static void APIENTRY draw_indexed(D3D10DDI_HDEVICE h, UINT count, UINT start, INT base)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	ID3D11DeviceContext1_DrawIndexed(dev->context_inner, count, start, base);
	dev->draws++;
	umd_unlock(dev);
}

static void APIENTRY draw_instanced(D3D10DDI_HDEVICE h, UINT count, UINT instances, UINT start,
				    UINT first_instance)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	ID3D11DeviceContext1_DrawInstanced(dev->context_inner, count, instances, start, first_instance);
	dev->draws++;
	umd_unlock(dev);
}

static void APIENTRY draw_indexed_instanced(D3D10DDI_HDEVICE h, UINT count, UINT instances, UINT start,
					    INT base, UINT first_instance)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	ID3D11DeviceContext1_DrawIndexedInstanced(dev->context_inner, count, instances, start, base,
						  first_instance);
	dev->draws++;
	umd_unlock(dev);
}

static void APIENTRY copy(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE dst, D3D10DDI_HRESOURCE src)
{
	UMD_DEVICE *dev = umd_device(h);

	if (!inner_of(dst) || !inner_of(src))
		return;
	umd_lock(dev);
	ID3D11DeviceContext1_CopyResource(dev->context_inner, inner_of(dst), inner_of(src));
	umd_unlock(dev);
}

static void APIENTRY copy_region(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE dst, UINT dst_sub, UINT x, UINT y,
				 UINT z, D3D10DDI_HRESOURCE src, UINT src_sub, const D3D10_DDI_BOX *box,
				 UINT flags)
{
	UMD_DEVICE *dev = umd_device(h);

	if (!inner_of(dst) || !inner_of(src))
		return;
	umd_lock(dev);
	// The DDI's box is the API's; so are the copy flags.
	ID3D11DeviceContext1_CopySubresourceRegion1(dev->context_inner, inner_of(dst), dst_sub, x, y, z,
						    inner_of(src), src_sub, (const D3D11_BOX *)box, flags);
	umd_unlock(dev);
}

static void APIENTRY update(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE dst, UINT sub, const D3D10_DDI_BOX *box,
			    const VOID *data, UINT row_pitch, UINT depth_pitch, UINT flags)
{
	UMD_DEVICE *dev = umd_device(h);

	if (!inner_of(dst))
		return;
	umd_lock(dev);
	ID3D11DeviceContext1_UpdateSubresource1(dev->context_inner, inner_of(dst), sub, (const D3D11_BOX *)box,
						data, row_pitch, depth_pitch, flags);
	umd_unlock(dev);
}

static void APIENTRY gen_mips(D3D10DDI_HDEVICE h, D3D10DDI_HSHADERRESOURCEVIEW hv)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_VIEW *v = hv.pDrvPrivate;

	if (!v || !v->inner)
		return;
	umd_lock(dev);
	ID3D11DeviceContext1_GenerateMips(dev->context_inner, (ID3D11ShaderResourceView *)v->inner);
	umd_unlock(dev);
}

static void APIENTRY resolve(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE dst, UINT dst_sub, D3D10DDI_HRESOURCE src,
			     UINT src_sub, DXGI_FORMAT format)
{
	UMD_DEVICE *dev = umd_device(h);

	if (!inner_of(dst) || !inner_of(src))
		return;
	umd_lock(dev);
	ID3D11DeviceContext1_ResolveSubresource(dev->context_inner, inner_of(dst), dst_sub, inner_of(src), src_sub,
						format);
	umd_unlock(dev);
}

// A flush is waited for, so what a program has drawn is on its surfaces
// before it tells another process (the compositor, by DirectComposition)
// that it is there; DXMT's own flush hands the work to a thread of its own
// and returns at once. This was made (0.3.38) for the missing title text
// and icons, which it did not fix: their cause was the clear of a newly
// opened surface (wm_surface.c, run rn62). Whether the wait is needed at
// all is still to be tried with it taken out.
void umd_flush(UMD_DEVICE *dev)
{
	umd_inner_wait(dev);
}

static BOOL APIENTRY flush(D3D10DDI_HDEVICE h, UINT context_type, UINT flags)
{
	UNREFERENCED_PARAMETER(context_type);
	UNREFERENCED_PARAMETER(flags);
	umd_flush(umd_device(h));
	umd_inner_work_lost(umd_device(h));
	return TRUE;
}

static SIZE_T APIENTRY query_size(D3D10DDI_HDEVICE h, const void *args)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(args);
	return sizeof(UMD_QUERY);
}

static void APIENTRY create_query(D3D10DDI_HDEVICE h, const D3DWDDM2_0DDIARG_CREATEQUERY *args, D3D10DDI_HQUERY hq,
				  D3D10DDI_HRTQUERY rt)
{
	UMD_QUERY *q = hq.pDrvPrivate;

	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(rt);
	umd_zero(q, sizeof(*q));
	q->type = args->Query;
}

static void APIENTRY destroy_query(D3D10DDI_HDEVICE h, D3D10DDI_HQUERY hq)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(hq);
}

static void APIENTRY query_begin_end(D3D10DDI_HDEVICE h, D3D10DDI_HQUERY hq)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(hq);
}

static ULONG64 now_ns(void)
{
	LARGE_INTEGER t, f;

	QueryPerformanceCounter(&t);
	QueryPerformanceFrequency(&f);
	return (ULONG64)t.QuadPart / (ULONG64)f.QuadPart * NANOSECONDS +
	       (ULONG64)t.QuadPart % (ULONG64)f.QuadPart * NANOSECONDS / (ULONG64)f.QuadPart;
}

static void APIENTRY query_get_data(D3D10DDI_HDEVICE h, D3D10DDI_HQUERY hq, void *data, UINT bytes, UINT flags)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_QUERY *q = hq.pDrvPrivate;

	UNREFERENCED_PARAMETER(flags);
	if (!data || !bytes)
		return;
	umd_zero(data, bytes);
	switch (q->type) {
	case D3D10DDI_QUERY_EVENT:
		umd_flush(dev);
		if (bytes >= sizeof(BOOL))
			*(BOOL *)data = TRUE;
		break;
	case D3D10DDI_QUERY_TIMESTAMP:
		if (bytes >= sizeof(ULONG64))
			*(ULONG64 *)data = now_ns();
		break;
	case D3D10DDI_QUERY_TIMESTAMPDISJOINT:
		if (bytes >= sizeof(D3D10_DDI_QUERY_DATA_TIMESTAMP_DISJOINT))
			((D3D10_DDI_QUERY_DATA_TIMESTAMP_DISJOINT *)data)->Frequency = NANOSECONDS;
		break;
	default:
		break;
	}
}

// Hints the inner device has no use for.
static void APIENTRY nothing(void)
{
}

void umd_fill_draws(D3DWDDM2_0DDI_DEVICEFUNCS *f)
{
	f->pfnDraw = draw;
	f->pfnDrawIndexed = draw_indexed;
	f->pfnDrawInstanced = draw_instanced;
	f->pfnDrawIndexedInstanced = draw_indexed_instanced;
	f->pfnResourceCopy = copy;
	f->pfnResourceConvert = copy;
	f->pfnResourceCopyRegion = copy_region;
	f->pfnResourceConvertRegion = copy_region;
	f->pfnResourceUpdateSubresourceUP = update;
	f->pfnDefaultConstantBufferUpdateSubresourceUP = update;
	f->pfnGenMips = gen_mips;
	f->pfnResourceResolveSubresource = resolve;
	f->pfnFlush = flush;
	f->pfnCalcPrivateQuerySize = (void *)query_size;
	f->pfnCreateQuery = create_query;
	f->pfnDestroyQuery = destroy_query;
	f->pfnQueryBegin = query_begin_end;
	f->pfnQueryEnd = query_begin_end;
	f->pfnQueryGetData = query_get_data;
	f->pfnDiscard = (void *)nothing;
	f->pfnSetPredication = (void *)nothing;
	f->pfnShaderResourceViewReadAfterWriteHazard = (void *)nothing;
	f->pfnResourceReadAfterWriteHazard = (void *)nothing;
}
