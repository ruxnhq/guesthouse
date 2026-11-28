// SPDX-License-Identifier: MIT
//
// Blt presents, copied on the device. A windowed swap chain of the older
// model presents by a kernel blt from its back buffer into the compositor's
// surface for the window, which the user-mode driver is not told (run rn43).
// The kernel driver notes where each blt went (blt_present.c); this file
// asks it, then before each present draws the program's back buffer into
// that surface with a second DXMT device of its own, so the program's
// state is untouched. A draw, not a copy: the two differ in format
// (Tutorial 7's back buffer is RGBA, the compositor's surface BGRA) and
// sampling turns one into the other. No CPU touches the pixels.
//
// The shaders are compiled from the HLSL below by Windows' own
// d3dcompiler_47.dll at the first such present.

#include "umd_inner.h"
#include <d3dcompiler.h>

// How often the destination is asked again, in presents: a window that
// changes size gets a new surface.
#define ASK_EVERY	32

typedef BOOL(WINAPI *SURFACE_IMPORT_NEXT)(ULONG64 token, UINT width, UINT height);
typedef HRESULT(WINAPI *D3D_COMPILE)(LPCVOID, SIZE_T, LPCSTR, const D3D_SHADER_MACRO *, ID3DInclude *, LPCSTR,
				     LPCSTR, UINT, UINT, ID3DBlob **, ID3DBlob **);

static const char shader_text[] =
	"Texture2D picture : register(t0);\n"
	"SamplerState sampled : register(s0);\n"
	"struct v { float4 at : SV_Position; float2 uv : TEXCOORD0; };\n"
	"v vs(uint id : SV_VertexID) {\n"
	"	v o;\n"
	"	o.uv = float2((id << 1) & 2, id & 2);\n"
	"	o.at = float4(o.uv * float2(2, -2) + float2(-1, 1), 0, 1);\n"
	"	return o;\n"
	"}\n"
	"float4 ps(v i) : SV_Target { return float4(picture.Sample(sampled, i.uv).rgb, 1); }\n";

struct umd_blt {
	ID3D11Device1 *device;
	ID3D11DeviceContext1 *context;
	ID3D11VertexShader *vs;
	ID3D11PixelShader *ps;
	ID3D11SamplerState *sampler;
	BOOL broken;			// something could not be made: no more tries
	ULONG64 src_token, dst_token;
	ID3D11Texture2D *src, *dst;
	ID3D11ShaderResourceView *srv;
	ID3D11RenderTargetView *rtv;
	UINT width, height;
	ULONG presents;
};

static void release(IUnknown **u)
{
	if (*u) {
		IUnknown_Release(*u);
		*u = NULL;
	}
}

static void drop_surfaces(struct umd_blt *b)
{
	release((IUnknown **)&b->srv);
	release((IUnknown **)&b->rtv);
	release((IUnknown **)&b->src);
	release((IUnknown **)&b->dst);
	b->src_token = b->dst_token = 0;
}

// The kernel driver's note of where this surface was last blt to.
static BOOL ask(UMD_DEVICE *dev, ULONG64 src_token, struct ruxnwddm_escape_blt_dest *q)
{
	D3DDDICB_ESCAPE esc;

	if (!dev->kt || !dev->kt->pfnEscapeCb)
		return FALSE;
	umd_zero(q, sizeof(*q));
	q->header.magic = RUXNWDDM_ESCAPE_MAGIC;
	q->header.version = RUXNWDDM_ESCAPE_VERSION;
	q->header.command = RUXNWDDM_ESCAPE_BLT_DEST;
	q->src_token = src_token;
	umd_zero(&esc, sizeof(esc));
	esc.pPrivateDriverData = q;
	esc.PrivateDriverDataSize = sizeof(*q);
	return SUCCEEDED(dev->kt->pfnEscapeCb(dev->adapter->runtime.handle, &esc)) && q->dst_token;
}

static HRESULT compile(D3D_COMPILE fn, const char *entry, const char *target, ID3DBlob **code)
{
	ID3DBlob *errors = NULL;
	HRESULT hr = fn(shader_text, sizeof(shader_text) - 1, "ruxn blt", NULL, NULL, entry, target, 0, 0, code,
			&errors);

	if (errors)
		ID3D10Blob_Release(errors);
	return hr;
}

// The copier's device, shaders and sampler, once.
static HRESULT make_copier(struct umd_blt *b)
{
	D3D11_SAMPLER_DESC sd;
	ID3DBlob *code = NULL;
	D3D_COMPILE fn;
	HMODULE compiler;
	HRESULT hr;

	compiler = LoadLibraryW(L"d3dcompiler_47.dll");
	fn = compiler ? (D3D_COMPILE)(void *)GetProcAddress(compiler, "D3DCompile") : NULL;
	if (!fn)
		return E_FAIL;
	hr = umd_inner_make(&b->device, &b->context);
	if (SUCCEEDED(hr))
		hr = compile(fn, "vs", "vs_4_0", &code);
	if (SUCCEEDED(hr)) {
		hr = ID3D11Device1_CreateVertexShader(b->device, ID3D10Blob_GetBufferPointer(code),
						      ID3D10Blob_GetBufferSize(code), NULL, &b->vs);
		ID3D10Blob_Release(code);
		code = NULL;
	}
	if (SUCCEEDED(hr))
		hr = compile(fn, "ps", "ps_4_0", &code);
	if (SUCCEEDED(hr)) {
		hr = ID3D11Device1_CreatePixelShader(b->device, ID3D10Blob_GetBufferPointer(code),
						     ID3D10Blob_GetBufferSize(code), NULL, &b->ps);
		ID3D10Blob_Release(code);
	}
	if (SUCCEEDED(hr)) {
		umd_zero(&sd, sizeof(sd));
		sd.Filter = D3D11_FILTER_MIN_MAG_MIP_LINEAR;
		sd.AddressU = sd.AddressV = sd.AddressW = D3D11_TEXTURE_ADDRESS_CLAMP;
		sd.MaxLOD = D3D11_FLOAT32_MAX;
		hr = ID3D11Device1_CreateSamplerState(b->device, &sd, &b->sampler);
	}
	return hr;
}

// A texture of the copier's device over a surface, by its token.
static HRESULT surface_texture(struct umd_blt *b, ULONG64 token, UINT width, UINT height, UINT format, UINT bind,
			       ID3D11Texture2D **out)
{
	SURFACE_IMPORT_NEXT import_next =
		(SURFACE_IMPORT_NEXT)(void *)GetProcAddress(GetModuleHandleW(L"winemetal.dll"), "RuxnSurfaceImportNext");
	D3D11_TEXTURE2D_DESC d;

	if (!import_next || !import_next(token, width, height))
		return E_FAIL;
	umd_zero(&d, sizeof(d));
	d.Width = width;
	d.Height = height;
	d.MipLevels = d.ArraySize = d.SampleDesc.Count = 1;
	d.Format = (DXGI_FORMAT)format;
	d.Usage = D3D11_USAGE_DEFAULT;
	d.BindFlags = bind;
	return ID3D11Device1_CreateTexture2D(b->device, &d, NULL, out);
}

// The program's surface and the compositor's, when the kernel names a new
// destination for it.
static HRESULT take_surfaces(struct umd_blt *b, const UMD_RESOURCE *src, const struct ruxnwddm_escape_blt_dest *q)
{
	HRESULT hr;

	drop_surfaces(b);
	hr = surface_texture(b, src->surface_token, src->width, src->height, src->format, D3D11_BIND_SHADER_RESOURCE,
			     &b->src);
	if (SUCCEEDED(hr))
		hr = surface_texture(b, q->dst_token, q->width, q->height, q->format, D3D11_BIND_RENDER_TARGET, &b->dst);
	if (SUCCEEDED(hr))
		hr = ID3D11Device1_CreateShaderResourceView(b->device, (ID3D11Resource *)b->src, NULL, &b->srv);
	if (SUCCEEDED(hr))
		hr = ID3D11Device1_CreateRenderTargetView(b->device, (ID3D11Resource *)b->dst, NULL, &b->rtv);
	if (FAILED(hr)) {
		drop_surfaces(b);
		return hr;
	}
	b->src_token = src->surface_token;
	b->dst_token = q->dst_token;
	b->width = q->width;
	b->height = q->height;
	return S_OK;
}

// The draw, waited for: what the compositor reads next is the frame.
static void draw(struct umd_blt *b)
{
	D3D11_VIEWPORT vp = { 0, 0, (FLOAT)b->width, (FLOAT)b->height, 0, 1 };
	D3D11_QUERY_DESC qd = { D3D11_QUERY_EVENT, 0 };
	ID3D11Query *query;
	BOOL done = FALSE;

	ID3D11DeviceContext1_OMSetRenderTargets(b->context, 1, &b->rtv, NULL);
	ID3D11DeviceContext1_RSSetViewports(b->context, 1, &vp);
	ID3D11DeviceContext1_IASetPrimitiveTopology(b->context, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
	ID3D11DeviceContext1_VSSetShader(b->context, b->vs, NULL, 0);
	ID3D11DeviceContext1_PSSetShader(b->context, b->ps, NULL, 0);
	ID3D11DeviceContext1_PSSetShaderResources(b->context, 0, 1, &b->srv);
	ID3D11DeviceContext1_PSSetSamplers(b->context, 0, 1, &b->sampler);
	ID3D11DeviceContext1_Draw(b->context, 3, 0);
	if (SUCCEEDED(ID3D11Device1_CreateQuery(b->device, &qd, &query))) {
		ID3D11DeviceContext1_End(b->context, (ID3D11Asynchronous *)query);
		while (ID3D11DeviceContext1_GetData(b->context, (ID3D11Asynchronous *)query, &done, sizeof(done), 0) ==
		       S_FALSE)
			SwitchToThread();
		ID3D11Query_Release(query);
	}
}

// Before a blt present of src: its frame into the compositor's surface,
// once the kernel has named it. The caller has waited for the program's
// own work (umd_present.c).
void umd_blt_copy(UMD_DEVICE *dev, UMD_RESOURCE *src)
{
	struct ruxnwddm_escape_blt_dest q;
	struct umd_blt *b = dev->blt;

	if (!dev->inner || !src || !src->surface_token)
		return;
	if (!b) {
		b = dev->blt = umd_alloc(sizeof(*b));
		if (!b)
			return;
		umd_zero(b, sizeof(*b));
		if (FAILED(make_copier(b))) {
			umd_log("blt: the copier could not be made", 0);
			b->broken = TRUE;
		}
	}
	if (b->broken)
		return;
	if ((!b->dst || b->src_token != src->surface_token || ++b->presents % ASK_EVERY == 0) &&
	    ask(dev, src->surface_token, &q) &&
	    (q.dst_token != b->dst_token || src->surface_token != b->src_token)) {
		HRESULT hr = take_surfaces(b, src, &q);

		umd_log("blt: destination token, and the result of taking it", q.dst_token);
		umd_log("blt: destination format, width, height", (ULONG64)q.format << 32 | q.width << 16 | q.height);
		if (FAILED(hr))
			umd_log("blt: the surfaces could not be taken", (ULONG)hr);
	}
	if (b->dst)
		draw(b);
}

void umd_blt_destroy(UMD_DEVICE *dev)
{
	struct umd_blt *b = dev->blt;

	if (!b)
		return;
	drop_surfaces(b);
	release((IUnknown **)&b->sampler);
	release((IUnknown **)&b->vs);
	release((IUnknown **)&b->ps);
	if (b->context) {
		ID3D11DeviceContext1_ClearState(b->context);
		release((IUnknown **)&b->context);
	}
	release((IUnknown **)&b->device);
	umd_free(b);
	dev->blt = NULL;
}
