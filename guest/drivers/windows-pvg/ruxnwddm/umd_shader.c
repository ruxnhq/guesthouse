// SPDX-License-Identifier: MIT
//
// Shaders and element layouts of the drawing device.
//
// A shader arrives as tokens and signatures; umd_dxbc.c makes a DXBC
// container of them, and the inner device compiles that with airconv inside
// the guest (winemetal's wm_shader.c). Compute shaders come with feature
// level 11; hull and domain shaders are refused there (umd_compute.c), and
// geometry shaders with stream output are not taken.
//
// An element layout arrives with each element's input register and no
// semantic name. The inner device matches a layout to a vertex shader by
// name, so the layout is made against a container holding only an input
// signature, whose elements are named from their registers exactly as
// umd_dxbc.c names a vertex shader's inputs: the same register, the same
// name.

#include "umd_inner.h"

static UMD_SHADER *shader_of(D3D10DDI_HSHADER h)
{
	return h.pDrvPrivate;
}

static UMD_LAYOUT *layout_of(D3D10DDI_HELEMENTLAYOUT h)
{
	return h.pDrvPrivate;
}

static SIZE_T APIENTRY shader_size(D3D10DDI_HDEVICE h, const UINT *code, const void *sig)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(code);
	UNREFERENCED_PARAMETER(sig);
	return sizeof(UMD_SHADER);
}

static void create_shader(D3D10DDI_HDEVICE h, UINT stage, const UINT *code, D3D10DDI_HSHADER hs,
			  const D3D11_1DDIARG_STAGE_IO_SIGNATURES *sig)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_SHADER *s = shader_of(hs);
	SIZE_T bytes = 0;
	void *dxbc;
	HRESULT hr;

	umd_zero(s, sizeof(*s));
	s->stage = stage;
	dxbc = umd_dxbc_build(code, sig, stage == UMD_STAGE_PIXEL, &bytes);
	if (!dxbc) {
		umd_error(dev, E_OUTOFMEMORY, "CreateShader: no memory");
		return;
	}
	umd_lock(dev);
	if (stage == UMD_STAGE_VERTEX)
		hr = ID3D11Device1_CreateVertexShader(dev->inner, dxbc, bytes, NULL,
						      (ID3D11VertexShader **)&s->inner);
	else if (stage == UMD_STAGE_GEOMETRY)
		hr = ID3D11Device1_CreateGeometryShader(dev->inner, dxbc, bytes, NULL,
							(ID3D11GeometryShader **)&s->inner);
	else if (stage == UMD_STAGE_COMPUTE)
		hr = ID3D11Device1_CreateComputeShader(dev->inner, dxbc, bytes, NULL,
						       (ID3D11ComputeShader **)&s->inner);
	else
		hr = ID3D11Device1_CreatePixelShader(dev->inner, dxbc, bytes, NULL,
						     (ID3D11PixelShader **)&s->inner);
	umd_unlock(dev);
	umd_free(dxbc);
	if (FAILED(hr))
		umd_error(dev, hr, "CreateShader failed");
}

static void APIENTRY create_vs(D3D10DDI_HDEVICE h, const UINT *code, D3D10DDI_HSHADER hs, D3D10DDI_HRTSHADER rt,
			       const D3D11_1DDIARG_STAGE_IO_SIGNATURES *sig)
{
	UNREFERENCED_PARAMETER(rt);
	create_shader(h, UMD_STAGE_VERTEX, code, hs, sig);
}

static void APIENTRY create_gs(D3D10DDI_HDEVICE h, const UINT *code, D3D10DDI_HSHADER hs, D3D10DDI_HRTSHADER rt,
			       const D3D11_1DDIARG_STAGE_IO_SIGNATURES *sig)
{
	UNREFERENCED_PARAMETER(rt);
	create_shader(h, UMD_STAGE_GEOMETRY, code, hs, sig);
}

static void APIENTRY create_ps(D3D10DDI_HDEVICE h, const UINT *code, D3D10DDI_HSHADER hs, D3D10DDI_HRTSHADER rt,
			       const D3D11_1DDIARG_STAGE_IO_SIGNATURES *sig)
{
	UNREFERENCED_PARAMETER(rt);
	create_shader(h, UMD_STAGE_PIXEL, code, hs, sig);
}

// A compute shader has no signatures; its container has empty ones.
static void APIENTRY create_cs(D3D10DDI_HDEVICE h, const UINT *code, D3D10DDI_HSHADER hs, D3D10DDI_HRTSHADER rt)
{
	UNREFERENCED_PARAMETER(rt);
	create_shader(h, UMD_STAGE_COMPUTE, code, hs, NULL);
}

static void APIENTRY destroy_shader(D3D10DDI_HDEVICE h, D3D10DDI_HSHADER hs)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_SHADER *s = shader_of(hs);

	if (dev->vs == s)
		dev->vs = NULL;
	if (!s->inner)
		return;
	umd_lock(dev);
	ID3D11DeviceChild_Release(s->inner);
	umd_unlock(dev);
	s->inner = NULL;
}

static void set_shader(D3D10DDI_HDEVICE h, UINT stage, D3D10DDI_HSHADER hs)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_SHADER *s = shader_of(hs);
	ID3D11DeviceChild *inner = s ? s->inner : NULL;

	umd_lock(dev);
	if (stage == UMD_STAGE_VERTEX) {
		dev->vs = s;
		ID3D11DeviceContext1_VSSetShader(dev->context_inner, (ID3D11VertexShader *)inner, NULL, 0);
	} else if (stage == UMD_STAGE_GEOMETRY) {
		ID3D11DeviceContext1_GSSetShader(dev->context_inner, (ID3D11GeometryShader *)inner, NULL, 0);
	} else if (stage == UMD_STAGE_COMPUTE) {
		ID3D11DeviceContext1_CSSetShader(dev->context_inner, (ID3D11ComputeShader *)inner, NULL, 0);
	} else {
		ID3D11DeviceContext1_PSSetShader(dev->context_inner, (ID3D11PixelShader *)inner, NULL, 0);
	}
	umd_unlock(dev);
}

static void APIENTRY set_vs(D3D10DDI_HDEVICE h, D3D10DDI_HSHADER hs)
{
	set_shader(h, UMD_STAGE_VERTEX, hs);
}

static void APIENTRY set_gs(D3D10DDI_HDEVICE h, D3D10DDI_HSHADER hs)
{
	set_shader(h, UMD_STAGE_GEOMETRY, hs);
}

static void APIENTRY set_ps(D3D10DDI_HDEVICE h, D3D10DDI_HSHADER hs)
{
	set_shader(h, UMD_STAGE_PIXEL, hs);
}

static void APIENTRY set_cs(D3D10DDI_HDEVICE h, D3D10DDI_HSHADER hs)
{
	set_shader(h, UMD_STAGE_COMPUTE, hs);
}

static SIZE_T APIENTRY layout_size(D3D10DDI_HDEVICE h, const D3D10DDIARG_CREATEELEMENTLAYOUT *args)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(args);
	return sizeof(UMD_LAYOUT);
}

static void APIENTRY create_layout(D3D10DDI_HDEVICE h, const D3D10DDIARG_CREATEELEMENTLAYOUT *args,
				   D3D10DDI_HELEMENTLAYOUT hl, D3D10DDI_HRTELEMENTLAYOUT rt)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_LAYOUT *l = layout_of(hl);
	D3D11_1DDIARG_SIGNATURE_ENTRY2 signature[UMD_MOST_ELEMENTS];
	SIZE_T bytes = 0;
	void *dxbc;
	HRESULT hr;
	UINT i;

	UNREFERENCED_PARAMETER(rt);
	umd_zero(l, sizeof(*l));
	umd_zero(signature, sizeof(signature));
	l->count = args->NumElements < UMD_MOST_ELEMENTS ? args->NumElements : UMD_MOST_ELEMENTS;
	for (i = 0; i < l->count; i++) {
		const D3D10DDIARG_INPUT_ELEMENT_DESC *from = &args->pVertexElements[i];
		D3D11_INPUT_ELEMENT_DESC *to = &l->elements[i];

		to->SemanticName = "TEXCOORD";
		to->SemanticIndex = from->InputRegister;
		to->Format = from->Format;
		to->InputSlot = from->InputSlot;
		to->AlignedByteOffset = from->AlignedByteOffset;
		to->InputSlotClass = (D3D11_INPUT_CLASSIFICATION)from->InputSlotClass;
		to->InstanceDataStepRate = from->InstanceDataStepRate;
		signature[i].SystemValue = D3D10_SB_NAME_UNDEFINED;
		signature[i].Register = from->InputRegister;
		signature[i].Mask = 0xf;
		signature[i].RegisterComponentType = D3D10_SB_REGISTER_COMPONENT_FLOAT32;
	}
	dxbc = umd_dxbc_input_signature(signature, l->count, &bytes);
	if (!dxbc) {
		umd_error(dev, E_OUTOFMEMORY, "CreateElementLayout: no memory");
		return;
	}
	umd_lock(dev);
	hr = l->count ? ID3D11Device1_CreateInputLayout(dev->inner, l->elements, l->count, dxbc, bytes, &l->inner)
		      : S_OK;
	umd_unlock(dev);
	umd_free(dxbc);
	if (FAILED(hr))
		umd_error(dev, hr, "CreateElementLayout failed");
}

static void APIENTRY destroy_layout(D3D10DDI_HDEVICE h, D3D10DDI_HELEMENTLAYOUT hl)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_LAYOUT *l = layout_of(hl);

	if (dev->layout == l)
		dev->layout = NULL;
	if (!l->inner)
		return;
	umd_lock(dev);
	ID3D11InputLayout_Release(l->inner);
	umd_unlock(dev);
	l->inner = NULL;
}

static void APIENTRY set_layout(D3D10DDI_HDEVICE h, D3D10DDI_HELEMENTLAYOUT hl)
{
	UMD_DEVICE *dev = umd_device(h);
	UMD_LAYOUT *l = layout_of(hl);

	umd_lock(dev);
	dev->layout = l;
	ID3D11DeviceContext1_IASetInputLayout(dev->context_inner, l ? l->inner : NULL);
	umd_unlock(dev);
}

void umd_fill_shaders(D3DWDDM2_0DDI_DEVICEFUNCS *f)
{
	f->pfnCalcPrivateShaderSize = (void *)shader_size;
	f->pfnCreateVertexShader = create_vs;
	f->pfnCreateGeometryShader = create_gs;
	f->pfnCreatePixelShader = create_ps;
	f->pfnDestroyShader = destroy_shader;
	f->pfnVsSetShader = set_vs;
	f->pfnGsSetShader = set_gs;
	f->pfnPsSetShader = set_ps;
	f->pfnCreateComputeShader = create_cs;
	f->pfnCsSetShader = set_cs;
	f->pfnCalcPrivateElementLayoutSize = layout_size;
	f->pfnCreateElementLayout = create_layout;
	f->pfnDestroyElementLayout = destroy_layout;
	f->pfnIaSetInputLayout = set_layout;
}
