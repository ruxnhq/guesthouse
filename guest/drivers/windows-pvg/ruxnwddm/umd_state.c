// SPDX-License-Identifier: MIT
//
// Fixed-function state of the drawing device: blend, depth-stencil,
// rasterizer and sampler states as inner states, and what is set on the
// context without an object: viewports, scissors, the input assembler's
// buffers and topology, constant buffers and samplers.
//
// The DDI's enumerations for blending, comparisons, stencil operations,
// filters, address modes, fill and cull modes, topologies and map types
// are numbered as the API's, so their values pass as they are; where a DDI
// structure is laid out differently from the API's, it is copied field by
// field.

#include "umd_inner.h"

static UMD_STATE *state_of(void *h)
{
	return h;
}

static UMD_RESOURCE *resource_of(D3D10DDI_HRESOURCE h)
{
	return h.pDrvPrivate;
}

static ID3D11Buffer *buffer_of(D3D10DDI_HRESOURCE h)
{
	UMD_RESOURCE *r = resource_of(h);

	return r ? (ID3D11Buffer *)r->inner : NULL;
}

static SIZE_T APIENTRY state_size(D3D10DDI_HDEVICE h, const void *args)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(args);
	return sizeof(UMD_STATE);
}

static void made(UMD_DEVICE *dev, HRESULT hr, const char *what)
{
	if (FAILED(hr))
		umd_error(dev, hr, what);
}

static void APIENTRY create_blend(D3D10DDI_HDEVICE h, const D3D11_1_DDI_BLEND_DESC *ddi, D3D10DDI_HBLENDSTATE hs,
				  D3D10DDI_HRTBLENDSTATE rt)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_STATE *s = state_of(hs.pDrvPrivate);
	D3D11_BLEND_DESC1 d;
	HRESULT hr;
	UINT i;

	UNREFERENCED_PARAMETER(rt);
	umd_zero(s, sizeof(*s));
	umd_zero(&d, sizeof(d));
	d.AlphaToCoverageEnable = ddi->AlphaToCoverageEnable;
	d.IndependentBlendEnable = ddi->IndependentBlendEnable;
	for (i = 0; i < D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT; i++) {
		const D3D11_1_DDI_RENDER_TARGET_BLEND_DESC *from = &ddi->RenderTarget[i];
		D3D11_RENDER_TARGET_BLEND_DESC1 *to = &d.RenderTarget[i];

		to->BlendEnable = from->BlendEnable;
		to->LogicOpEnable = from->LogicOpEnable;
		to->SrcBlend = (D3D11_BLEND)from->SrcBlend;
		to->DestBlend = (D3D11_BLEND)from->DestBlend;
		to->BlendOp = (D3D11_BLEND_OP)from->BlendOp;
		to->SrcBlendAlpha = (D3D11_BLEND)from->SrcBlendAlpha;
		to->DestBlendAlpha = (D3D11_BLEND)from->DestBlendAlpha;
		to->BlendOpAlpha = (D3D11_BLEND_OP)from->BlendOpAlpha;
		to->LogicOp = (D3D11_LOGIC_OP)from->LogicOp;
		to->RenderTargetWriteMask = from->RenderTargetWriteMask;
	}
	umd_lock(dev);
	umd_inner_clear_failures();
	hr = ID3D11Device1_CreateBlendState1(dev->inner, &d, (ID3D11BlendState1 **)&s->inner);
	hr = umd_inner_checked(hr, (void **)&s->inner);
	umd_unlock(dev);
	made(dev, hr, "CreateBlendState failed");
}

static void APIENTRY create_depth(D3D10DDI_HDEVICE h, const D3D10_DDI_DEPTH_STENCIL_DESC *ddi,
				  D3D10DDI_HDEPTHSTENCILSTATE hs, D3D10DDI_HRTDEPTHSTENCILSTATE rt)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_STATE *s = state_of(hs.pDrvPrivate);
	D3D11_DEPTH_STENCIL_DESC d;
	HRESULT hr;

	UNREFERENCED_PARAMETER(rt);
	umd_zero(s, sizeof(*s));
	d.DepthEnable = ddi->DepthEnable;
	d.DepthWriteMask = (D3D11_DEPTH_WRITE_MASK)ddi->DepthWriteMask;
	d.DepthFunc = (D3D11_COMPARISON_FUNC)ddi->DepthFunc;
	d.StencilEnable = ddi->StencilEnable;
	d.StencilReadMask = ddi->StencilReadMask;
	d.StencilWriteMask = ddi->StencilWriteMask;
	d.FrontFace.StencilFailOp = (D3D11_STENCIL_OP)ddi->FrontFace.StencilFailOp;
	d.FrontFace.StencilDepthFailOp = (D3D11_STENCIL_OP)ddi->FrontFace.StencilDepthFailOp;
	d.FrontFace.StencilPassOp = (D3D11_STENCIL_OP)ddi->FrontFace.StencilPassOp;
	d.FrontFace.StencilFunc = (D3D11_COMPARISON_FUNC)ddi->FrontFace.StencilFunc;
	d.BackFace.StencilFailOp = (D3D11_STENCIL_OP)ddi->BackFace.StencilFailOp;
	d.BackFace.StencilDepthFailOp = (D3D11_STENCIL_OP)ddi->BackFace.StencilDepthFailOp;
	d.BackFace.StencilPassOp = (D3D11_STENCIL_OP)ddi->BackFace.StencilPassOp;
	d.BackFace.StencilFunc = (D3D11_COMPARISON_FUNC)ddi->BackFace.StencilFunc;
	umd_lock(dev);
	umd_inner_clear_failures();
	hr = ID3D11Device1_CreateDepthStencilState(dev->inner, &d, (ID3D11DepthStencilState **)&s->inner);
	hr = umd_inner_checked(hr, (void **)&s->inner);
	umd_unlock(dev);
	made(dev, hr, "CreateDepthStencilState failed");
}

static void APIENTRY create_raster(D3D10DDI_HDEVICE h, const D3DWDDM2_0DDI_RASTERIZER_DESC *ddi,
				   D3D10DDI_HRASTERIZERSTATE hs, D3D10DDI_HRTRASTERIZERSTATE rt)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_STATE *s = state_of(hs.pDrvPrivate);
	D3D11_RASTERIZER_DESC1 d;
	HRESULT hr;

	UNREFERENCED_PARAMETER(rt);
	umd_zero(s, sizeof(*s));
	d.FillMode = (D3D11_FILL_MODE)ddi->FillMode;
	d.CullMode = (D3D11_CULL_MODE)ddi->CullMode;
	d.FrontCounterClockwise = ddi->FrontCounterClockwise;
	d.DepthBias = ddi->DepthBias;
	d.DepthBiasClamp = ddi->DepthBiasClamp;
	d.SlopeScaledDepthBias = ddi->SlopeScaledDepthBias;
	d.DepthClipEnable = ddi->DepthClipEnable;
	d.ScissorEnable = ddi->ScissorEnable;
	d.MultisampleEnable = ddi->MultisampleEnable;
	d.AntialiasedLineEnable = ddi->AntialiasedLineEnable;
	d.ForcedSampleCount = ddi->ForcedSampleCount;
	umd_lock(dev);
	umd_inner_clear_failures();
	hr = ID3D11Device1_CreateRasterizerState1(dev->inner, &d, (ID3D11RasterizerState1 **)&s->inner);
	hr = umd_inner_checked(hr, (void **)&s->inner);
	umd_unlock(dev);
	made(dev, hr, "CreateRasterizerState failed");
}

static void APIENTRY create_sampler(D3D10DDI_HDEVICE h, const D3D10_DDI_SAMPLER_DESC *ddi, D3D10DDI_HSAMPLER hs,
				    D3D10DDI_HRTSAMPLER rt)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_STATE *s = state_of(hs.pDrvPrivate);
	HRESULT hr;

	UNREFERENCED_PARAMETER(rt);
	umd_zero(s, sizeof(*s));
	umd_lock(dev);
	// The DDI's sampler description is the API's, field for field.
	umd_inner_clear_failures();
	hr = ID3D11Device1_CreateSamplerState(dev->inner, (const D3D11_SAMPLER_DESC *)ddi,
					      (ID3D11SamplerState **)&s->inner);
	hr = umd_inner_checked(hr, (void **)&s->inner);
	umd_unlock(dev);
	made(dev, hr, "CreateSampler failed");
}

static void destroy_state(D3D10DDI_HDEVICE h, void *hs)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_STATE *s = state_of(hs);

	if (!s->inner)
		return;
	umd_lock(dev);
	IUnknown_Release(s->inner);
	umd_unlock(dev);
	s->inner = NULL;
}

static void APIENTRY destroy_blend(D3D10DDI_HDEVICE h, D3D10DDI_HBLENDSTATE hs)
{
	destroy_state(h, hs.pDrvPrivate);
}

static void APIENTRY destroy_depth(D3D10DDI_HDEVICE h, D3D10DDI_HDEPTHSTENCILSTATE hs)
{
	destroy_state(h, hs.pDrvPrivate);
}

static void APIENTRY destroy_raster(D3D10DDI_HDEVICE h, D3D10DDI_HRASTERIZERSTATE hs)
{
	destroy_state(h, hs.pDrvPrivate);
}

static void APIENTRY destroy_sampler(D3D10DDI_HDEVICE h, D3D10DDI_HSAMPLER hs)
{
	destroy_state(h, hs.pDrvPrivate);
}

static void *inner_state(void *hs)
{
	UMD_STATE *s = hs;

	return s ? s->inner : NULL;
}

static void APIENTRY set_blend(D3D10DDI_HDEVICE h, D3D10DDI_HBLENDSTATE hs, const FLOAT factor[4], UINT mask)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	ID3D11DeviceContext1_OMSetBlendState(dev->context_inner, inner_state(hs.pDrvPrivate), factor, mask);
	umd_unlock(dev);
}

static void APIENTRY set_depth(D3D10DDI_HDEVICE h, D3D10DDI_HDEPTHSTENCILSTATE hs, UINT ref)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	ID3D11DeviceContext1_OMSetDepthStencilState(dev->context_inner, inner_state(hs.pDrvPrivate), ref);
	umd_unlock(dev);
}

static void APIENTRY set_raster(D3D10DDI_HDEVICE h, D3D10DDI_HRASTERIZERSTATE hs)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	ID3D11DeviceContext1_RSSetState(dev->context_inner, inner_state(hs.pDrvPrivate));
	umd_unlock(dev);
}

// Viewports and scissors: the DDI's are laid out as the API's.
static void APIENTRY set_viewports(D3D10DDI_HDEVICE h, UINT count, UINT clear, const D3D10_DDI_VIEWPORT *v)
{
	UMD_DEVICE *dev = umd_device(h);

	UNREFERENCED_PARAMETER(clear);
	umd_lock(dev);
	ID3D11DeviceContext1_RSSetViewports(dev->context_inner, count, (const D3D11_VIEWPORT *)v);
	umd_unlock(dev);
}

static void APIENTRY set_scissors(D3D10DDI_HDEVICE h, UINT count, UINT clear, const D3D10_DDI_RECT *r)
{
	UMD_DEVICE *dev = umd_device(h);

	UNREFERENCED_PARAMETER(clear);
	umd_lock(dev);
	ID3D11DeviceContext1_RSSetScissorRects(dev->context_inner, count, (const D3D11_RECT *)r);
	umd_unlock(dev);
}

static void APIENTRY set_topology(D3D10DDI_HDEVICE h, D3D10_DDI_PRIMITIVE_TOPOLOGY topology)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	ID3D11DeviceContext1_IASetPrimitiveTopology(dev->context_inner, (D3D11_PRIMITIVE_TOPOLOGY)topology);
	umd_unlock(dev);
}

static void APIENTRY set_vertex_buffers(D3D10DDI_HDEVICE h, UINT start, UINT count,
					const D3D10DDI_HRESOURCE *buffers, const UINT *strides, const UINT *offsets)
{
	UMD_DEVICE *dev = umd_device(h);
	ID3D11Buffer *inner[D3D11_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT];
	UINT i;

	if (start >= D3D11_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT)
		return;
	if (count > D3D11_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT - start)
		count = D3D11_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT - start;
	for (i = 0; i < count; i++)
		inner[i] = buffer_of(buffers[i]);
	umd_lock(dev);
	ID3D11DeviceContext1_IASetVertexBuffers(dev->context_inner, start, count, inner, strides, offsets);
	umd_unlock(dev);
}

static void APIENTRY set_index_buffer(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE buffer, DXGI_FORMAT format,
				      UINT offset)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	ID3D11DeviceContext1_IASetIndexBuffer(dev->context_inner, buffer_of(buffer), format, offset);
	umd_unlock(dev);
}

// Constant buffers with the 11.1 offsets, in constants of 16 bytes, which
// the API takes as they are.
static void set_constants(D3D10DDI_HDEVICE h, UINT stage, UINT start, UINT count, const D3D10DDI_HRESOURCE *b,
			  const UINT *first, const UINT *number)
{
	UMD_DEVICE *dev = umd_device(h);
	ID3D11Buffer *inner[D3D11_COMMONSHADER_CONSTANT_BUFFER_API_SLOT_COUNT];
	UINT i;

	if (start >= D3D11_COMMONSHADER_CONSTANT_BUFFER_API_SLOT_COUNT)
		return;
	if (count > D3D11_COMMONSHADER_CONSTANT_BUFFER_API_SLOT_COUNT - start)
		count = D3D11_COMMONSHADER_CONSTANT_BUFFER_API_SLOT_COUNT - start;
	for (i = 0; i < count; i++)
		inner[i] = buffer_of(b[i]);
	umd_lock(dev);
	if (stage == UMD_STAGE_VERTEX)
		ID3D11DeviceContext1_VSSetConstantBuffers1(dev->context_inner, start, count, inner, first, number);
	else if (stage == UMD_STAGE_GEOMETRY)
		ID3D11DeviceContext1_GSSetConstantBuffers1(dev->context_inner, start, count, inner, first, number);
	else if (stage == UMD_STAGE_COMPUTE)
		ID3D11DeviceContext1_CSSetConstantBuffers1(dev->context_inner, start, count, inner, first, number);
	else
		ID3D11DeviceContext1_PSSetConstantBuffers1(dev->context_inner, start, count, inner, first, number);
	umd_unlock(dev);
}

static void APIENTRY vs_constants(D3D10DDI_HDEVICE h, UINT s, UINT n, const D3D10DDI_HRESOURCE *b, const UINT *f,
				  const UINT *c)
{
	set_constants(h, UMD_STAGE_VERTEX, s, n, b, f, c);
}

static void APIENTRY gs_constants(D3D10DDI_HDEVICE h, UINT s, UINT n, const D3D10DDI_HRESOURCE *b, const UINT *f,
				  const UINT *c)
{
	set_constants(h, UMD_STAGE_GEOMETRY, s, n, b, f, c);
}

static void APIENTRY ps_constants(D3D10DDI_HDEVICE h, UINT s, UINT n, const D3D10DDI_HRESOURCE *b, const UINT *f,
				  const UINT *c)
{
	set_constants(h, UMD_STAGE_PIXEL, s, n, b, f, c);
}

static void set_samplers(D3D10DDI_HDEVICE h, UINT stage, UINT start, UINT count, const D3D10DDI_HSAMPLER *s)
{
	UMD_DEVICE *dev = umd_device(h);
	ID3D11SamplerState *inner[D3D11_COMMONSHADER_SAMPLER_SLOT_COUNT];
	UINT i;

	if (start >= D3D11_COMMONSHADER_SAMPLER_SLOT_COUNT)
		return;
	if (count > D3D11_COMMONSHADER_SAMPLER_SLOT_COUNT - start)
		count = D3D11_COMMONSHADER_SAMPLER_SLOT_COUNT - start;
	for (i = 0; i < count; i++)
		inner[i] = inner_state(s[i].pDrvPrivate);
	umd_lock(dev);
	if (stage == UMD_STAGE_VERTEX)
		ID3D11DeviceContext1_VSSetSamplers(dev->context_inner, start, count, inner);
	else if (stage == UMD_STAGE_GEOMETRY)
		ID3D11DeviceContext1_GSSetSamplers(dev->context_inner, start, count, inner);
	else if (stage == UMD_STAGE_COMPUTE)
		ID3D11DeviceContext1_CSSetSamplers(dev->context_inner, start, count, inner);
	else
		ID3D11DeviceContext1_PSSetSamplers(dev->context_inner, start, count, inner);
	umd_unlock(dev);
}

static void APIENTRY vs_samplers(D3D10DDI_HDEVICE h, UINT s, UINT n, const D3D10DDI_HSAMPLER *v)
{
	set_samplers(h, UMD_STAGE_VERTEX, s, n, v);
}

static void APIENTRY gs_samplers(D3D10DDI_HDEVICE h, UINT s, UINT n, const D3D10DDI_HSAMPLER *v)
{
	set_samplers(h, UMD_STAGE_GEOMETRY, s, n, v);
}

static void APIENTRY ps_samplers(D3D10DDI_HDEVICE h, UINT s, UINT n, const D3D10DDI_HSAMPLER *v)
{
	set_samplers(h, UMD_STAGE_PIXEL, s, n, v);
}

static void APIENTRY cs_constants(D3D10DDI_HDEVICE h, UINT s, UINT n, const D3D10DDI_HRESOURCE *b, const UINT *f,
				  const UINT *c)
{
	set_constants(h, UMD_STAGE_COMPUTE, s, n, b, f, c);
}

static void APIENTRY cs_samplers(D3D10DDI_HDEVICE h, UINT s, UINT n, const D3D10DDI_HSAMPLER *v)
{
	set_samplers(h, UMD_STAGE_COMPUTE, s, n, v);
}

void umd_fill_states(D3DWDDM2_0DDI_DEVICEFUNCS *f)
{
	f->pfnCalcPrivateBlendStateSize = (void *)state_size;
	f->pfnCreateBlendState = create_blend;
	f->pfnDestroyBlendState = destroy_blend;
	f->pfnCalcPrivateDepthStencilStateSize = (void *)state_size;
	f->pfnCreateDepthStencilState = create_depth;
	f->pfnDestroyDepthStencilState = destroy_depth;
	f->pfnCalcPrivateRasterizerStateSize = (void *)state_size;
	f->pfnCreateRasterizerState = create_raster;
	f->pfnDestroyRasterizerState = destroy_raster;
	f->pfnCalcPrivateSamplerSize = (void *)state_size;
	f->pfnCreateSampler = create_sampler;
	f->pfnDestroySampler = destroy_sampler;
	f->pfnSetBlendState = set_blend;
	f->pfnSetDepthStencilState = set_depth;
	f->pfnSetRasterizerState = set_raster;
	f->pfnSetViewports = set_viewports;
	f->pfnSetScissorRects = set_scissors;
	f->pfnIaSetTopology = set_topology;
	f->pfnIaSetVertexBuffers = set_vertex_buffers;
	f->pfnIaSetIndexBuffer = set_index_buffer;
	f->pfnVsSetConstantBuffers = vs_constants;
	f->pfnGsSetConstantBuffers = gs_constants;
	f->pfnPsSetConstantBuffers = ps_constants;
	f->pfnVsSetSamplers = vs_samplers;
	f->pfnGsSetSamplers = gs_samplers;
	f->pfnCsSetConstantBuffers = cs_constants;
	f->pfnCsSetSamplers = cs_samplers;
	f->pfnPsSetSamplers = ps_samplers;
}
