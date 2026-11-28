// SPDX-License-Identifier: MIT
//
// What feature level 11 adds to the drawing device beyond shaders and
// views (umd_shader.c, umd_view.c): dispatches, and the stages this device
// does not carry yet. Compute goes through the inner device to winemetal,
// whose compute segments the device runs (translate_blit.c).
//
// Hull and domain shaders are refused with E_NOTIMPL, so a program that
// asks for tessellation is told so at the shader it makes, not given a
// draw that silently comes out wrong: our DXBC containers carry no
// patch-constant signature yet. That is feature level 11 advertised and
// thin, said so in the design note.

#include "umd_inner.h"

static ID3D11Buffer *buffer_of(D3D10DDI_HRESOURCE h)
{
	UMD_RESOURCE *r = h.pDrvPrivate;

	return r ? (ID3D11Buffer *)r->inner : NULL;
}

static void APIENTRY dispatch(D3D10DDI_HDEVICE h, UINT x, UINT y, UINT z)
{
	UMD_DEVICE *dev = umd_device(h);

	umd_lock(dev);
	ID3D11DeviceContext1_Dispatch(dev->context_inner, x, y, z);
	umd_unlock(dev);
}

static void APIENTRY dispatch_indirect(D3D10DDI_HDEVICE h, D3D10DDI_HRESOURCE args, UINT offset)
{
	UMD_DEVICE *dev = umd_device(h);
	ID3D11Buffer *buffer = buffer_of(args);

	if (!buffer)
		return;
	umd_lock(dev);
	ID3D11DeviceContext1_DispatchIndirect(dev->context_inner, buffer, offset);
	umd_unlock(dev);
}

static SIZE_T APIENTRY tessellation_shader_size(D3D10DDI_HDEVICE h, const UINT *code,
						const D3D11DDIARG_TESSELLATION_IO_SIGNATURES *sig)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(code);
	UNREFERENCED_PARAMETER(sig);
	return sizeof(UMD_SHADER);
}

static void APIENTRY refuse_tessellation_shader(D3D10DDI_HDEVICE h, const UINT *code, D3D10DDI_HSHADER hs,
						D3D10DDI_HRTSHADER rt,
						const D3D11_1DDIARG_TESSELLATION_IO_SIGNATURES *sig)
{
	UMD_SHADER *s = hs.pDrvPrivate;

	UNREFERENCED_PARAMETER(code);
	UNREFERENCED_PARAMETER(rt);
	UNREFERENCED_PARAMETER(sig);
	umd_zero(s, sizeof(*s));
	umd_error(umd_device(h), E_NOTIMPL, "a hull or domain shader: tessellation is not carried yet");
}

void umd_fill_compute(D3DWDDM2_0DDI_DEVICEFUNCS *f)
{
	f->pfnDispatch = dispatch;
	f->pfnDispatchIndirect = dispatch_indirect;
	f->pfnCalcPrivateTessellationShaderSize = (void *)tessellation_shader_size;
	f->pfnCreateHullShader = refuse_tessellation_shader;
	f->pfnCreateDomainShader = refuse_tessellation_shader;
}
