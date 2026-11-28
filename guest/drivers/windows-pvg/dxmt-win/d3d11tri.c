// SPDX-License-Identifier: MIT
//
// d3d11tri: the triangle of pvgtri, drawn through plain Direct3D 11, two
// ways:
//
//   d3d11tri dxmt OUT.ppm     the device comes from DXMT's d3d11 in this
//                             program's own folder (ruxnd3d11.dll, with its
//                             ruxndxgi.dll and our winemetal.dll), loaded by
//                             full path, never from Windows' own
//   d3d11tri system OUT.ppm   the device comes from Windows' own d3d11.dll
//                             on ruxnwddm's adapter, so the runtime drives
//                             our user-mode driver, ruxnumd.dll, which draws
//                             with DXMT inside it
//
// With "quad" after the mode, the triangle's target is then sampled by a
// second draw: a quad from a vertex buffer, with an input layout, a
// constant buffer and a point sampler, drawn into a second target of the
// same size, texel for texel. That second picture is the one written, and
// it too must equal the reference: what a texture, a sampler, a constant
// buffer and a vertex buffer reached through the inner device's tables
// change of it is nothing.
//
// The program checks where the modules and the device's code came from
// before it draws. It draws into a 256 by 256 B8G8R8A8 render target
// cleared to 0.25, copies that into a staging texture, maps it and writes a
// PPM, as pvgtri does, so the picture can be held against pvgtri's Metal
// reference.
//
// Exit status 0 when the picture has pixels other than the clear colour and
// of it, 1 when not or something failed, 2 for a usage error.

#define COBJMACROS
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d11_1.h>
#include <dxgi.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>

#include "triangle_dxbc.h"

#define W 256
#define H 256
#define CLEAR 0x40
// The paravirtualized device, as DXGI names ruxnwddm's adapter.
#define OUR_VENDOR 0x106B
#define OUR_DEVICE 0xEEEE

static WCHAR folder[MAX_PATH];

static HMODULE load(const WCHAR *name)
{
	WCHAR path[MAX_PATH];
	HMODULE m;

	swprintf(path, MAX_PATH, L"%ls\\%ls", folder, name);
	m = LoadLibraryExW(path, NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
	if (!m)
		printf("d3d11tri: loading %ls: error %lu\n", path, GetLastError());
	return m;
}

// Whether a module lies in this program's folder; prints where it lies.
static BOOL ours(const char *what, HMODULE m)
{
	WCHAR path[MAX_PATH];
	size_t n = wcslen(folder);

	if (!m || !GetModuleFileNameW(m, path, MAX_PATH)) {
		printf("d3d11tri: %s: not loaded\n", what);
		return FALSE;
	}
	printf("d3d11tri: %s from %ls\n", what, path);
	return _wcsnicmp(path, folder, n) == 0 && path[n] == L'\\' && !wcschr(path + n + 1, L'\\');
}

static BOOL write_picture(const char *path, const UCHAR *pixels, UINT pitch, ULONG *lit, ULONG *clear)
{
	FILE *f = fopen(path, "wb");
	UINT x, y;

	if (!f)
		return FALSE;
	fprintf(f, "P6\n%d %d\n255\n", W, H);
	*lit = *clear = 0;
	for (y = 0; y < H; y++) {
		const UCHAR *row = pixels + (size_t)y * pitch;

		for (x = 0; x < W; x++) {
			UCHAR rgb[3] = { row[4 * x + 2], row[4 * x + 1], row[4 * x] };

			if (rgb[0] != CLEAR || rgb[1] != CLEAR || rgb[2] != CLEAR)
				(*lit)++;
			else
				(*clear)++;
			fwrite(rgb, 1, 3, f);
		}
	}
	return fclose(f) == 0;
}

// ruxnwddm's adapter, through Windows' own DXGI.
static IDXGIAdapter *our_adapter(void)
{
	IDXGIFactory1 *factory;
	IDXGIAdapter1 *adapter;
	DXGI_ADAPTER_DESC1 desc;
	UINT i;

	if (FAILED(CreateDXGIFactory1(&IID_IDXGIFactory1, (void **)&factory)))
		return NULL;
	for (i = 0; IDXGIFactory1_EnumAdapters1(factory, i, &adapter) == S_OK; i++) {
		if (SUCCEEDED(IDXGIAdapter1_GetDesc1(adapter, &desc)) && desc.VendorId == OUR_VENDOR &&
		    desc.DeviceId == OUR_DEVICE)
			return (IDXGIAdapter *)adapter;
		IDXGIAdapter1_Release(adapter);
	}
	return NULL;
}

// Whether a module lies in Windows' own folder.
static BOOL windows_own(const char *what, HMODULE m)
{
	WCHAR path[MAX_PATH], system[MAX_PATH];
	UINT n = GetSystemDirectoryW(system, MAX_PATH);

	if (!m || !GetModuleFileNameW(m, path, MAX_PATH)) {
		printf("d3d11tri: %s: not loaded\n", what);
		return FALSE;
	}
	printf("d3d11tri: %s from %ls\n", what, path);
	return n && _wcsnicmp(path, system, n) == 0;
}

// The quad: two triangles over the whole target, each corner's position
// and where in the texture it samples (Direct3D's: v grows downwards).
static const float quad_vertices[6][4] = {
	{ -1, 1, 0, 0 }, { 1, 1, 1, 0 }, { -1, -1, 0, 1 },
	{ -1, -1, 0, 1 }, { 1, 1, 1, 0 }, { 1, -1, 1, 1 },
};
// The vertex shader's cb0[0]: texture coordinates scaled by xy, moved by zw.
static const float quad_constants[4] = { 1, 1, 0, 0 };

#define CHECK(what, call)                                                   \
	do {                                                                \
		HRESULT hr_ = (call);                                       \
		if (FAILED(hr_)) {                                          \
			printf("d3d11tri: %s: 0x%08lx\n", what, hr_);       \
			return 1;                                           \
		}                                                           \
	} while (0)

static ID3D11Texture2D *quad_target;

// The second draw: the triangle's target, sampled into quad_target.
static int draw_quad(ID3D11Device *device, ID3D11DeviceContext *context, ID3D11Texture2D *source,
		     const D3D11_TEXTURE2D_DESC *target_desc)
{
	static const D3D11_INPUT_ELEMENT_DESC elements[2] = {
		{ "POSITION", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 0, D3D11_INPUT_PER_VERTEX_DATA, 0 },
		{ "TEXCOORD", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 8, D3D11_INPUT_PER_VERTEX_DATA, 0 },
	};
	D3D11_TEXTURE2D_DESC desc = *target_desc;
	D3D11_BUFFER_DESC vb = { sizeof(quad_vertices), D3D11_USAGE_IMMUTABLE, D3D11_BIND_VERTEX_BUFFER, 0, 0, 0 };
	D3D11_BUFFER_DESC cb = { sizeof(quad_constants), D3D11_USAGE_IMMUTABLE, D3D11_BIND_CONSTANT_BUFFER, 0, 0, 0 };
	D3D11_SUBRESOURCE_DATA vdata = { quad_vertices, 0, 0 }, cdata = { quad_constants, 0, 0 };
	D3D11_SAMPLER_DESC sd;
	ID3D11Buffer *vertices, *constants;
	ID3D11InputLayout *layout;
	ID3D11VertexShader *vs;
	ID3D11PixelShader *ps;
	ID3D11ShaderResourceView *view;
	ID3D11RenderTargetView *rtv;
	ID3D11SamplerState *sampler;
	UINT stride = sizeof(quad_vertices[0]), offset = 0;

	desc.Usage = D3D11_USAGE_DEFAULT;
	desc.CPUAccessFlags = 0;
	desc.BindFlags = D3D11_BIND_RENDER_TARGET;
	CHECK("the quad's target", ID3D11Device_CreateTexture2D(device, &desc, NULL, &quad_target));
	CHECK("the quad's target view",
	      ID3D11Device_CreateRenderTargetView(device, (ID3D11Resource *)quad_target, NULL, &rtv));
	CHECK("the triangle's view",
	      ID3D11Device_CreateShaderResourceView(device, (ID3D11Resource *)source, NULL, &view));
	CHECK("the vertex buffer", ID3D11Device_CreateBuffer(device, &vb, &vdata, &vertices));
	CHECK("the constant buffer", ID3D11Device_CreateBuffer(device, &cb, &cdata, &constants));
	CHECK("the quad's vertex shader",
	      ID3D11Device_CreateVertexShader(device, quad_vs_dxbc, sizeof(quad_vs_dxbc), NULL, &vs));
	CHECK("the quad's pixel shader",
	      ID3D11Device_CreatePixelShader(device, quad_ps_dxbc, sizeof(quad_ps_dxbc), NULL, &ps));
	CHECK("the input layout",
	      ID3D11Device_CreateInputLayout(device, elements, 2, quad_vs_dxbc, sizeof(quad_vs_dxbc), &layout));
	memset(&sd, 0, sizeof(sd));
	sd.Filter = D3D11_FILTER_MIN_MAG_MIP_POINT;
	sd.AddressU = sd.AddressV = sd.AddressW = D3D11_TEXTURE_ADDRESS_CLAMP;
	sd.ComparisonFunc = D3D11_COMPARISON_NEVER;
	sd.MaxLOD = D3D11_FLOAT32_MAX;
	CHECK("the sampler", ID3D11Device_CreateSamplerState(device, &sd, &sampler));

	ID3D11DeviceContext_OMSetRenderTargets(context, 1, &rtv, NULL);
	ID3D11DeviceContext_IASetInputLayout(context, layout);
	ID3D11DeviceContext_IASetVertexBuffers(context, 0, 1, &vertices, &stride, &offset);
	ID3D11DeviceContext_VSSetShader(context, vs, NULL, 0);
	ID3D11DeviceContext_VSSetConstantBuffers(context, 0, 1, &constants);
	ID3D11DeviceContext_PSSetShader(context, ps, NULL, 0);
	ID3D11DeviceContext_PSSetShaderResources(context, 0, 1, &view);
	ID3D11DeviceContext_PSSetSamplers(context, 0, 1, &sampler);
	ID3D11DeviceContext_Draw(context, 6, 0);
	printf("d3d11tri: the quad drawn from the triangle's target\n");
	return 0;
}

// The device from DXMT's DLLs in this program's folder.
static HRESULT dxmt_device(ID3D11Device **device, D3D_FEATURE_LEVEL *level, ID3D11DeviceContext **context)
{
	static const D3D_FEATURE_LEVEL levels[] = { D3D_FEATURE_LEVEL_11_0 };
	PFN_D3D11_CREATE_DEVICE create;
	HMODULE winemetal, dxgi, d3d11, code;
	HRESULT hr;

	// dxgi before d3d11, so that d3d11's import of it binds to it.
	winemetal = load(L"winemetal.dll");
	dxgi = load(L"ruxndxgi.dll");
	d3d11 = load(L"ruxnd3d11.dll");
	if (!winemetal || !dxgi || !d3d11)
		return E_FAIL;
	if (!ours("winemetal.dll", winemetal) || !ours("ruxndxgi.dll", dxgi) || !ours("ruxnd3d11.dll", d3d11)) {
		printf("d3d11tri: a module is not DXMT's from this folder\n");
		return E_FAIL;
	}
	create = (PFN_D3D11_CREATE_DEVICE)(void *)GetProcAddress(d3d11, "D3D11CreateDevice");
	if (!create)
		return E_FAIL;
	hr = create(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, levels, 1, D3D11_SDK_VERSION, device, level, context);
	if (FAILED(hr))
		return hr;
	// The device's methods must be DXMT's code.
	if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
				(LPCWSTR)(void *)(*device)->lpVtbl->CreateTexture2D, &code) ||
	    !ours("the device's code", code))
		return E_FAIL;
	return S_OK;
}

// The device from Windows' own runtime on our adapter: our user-mode
// driver must then be loaded in this process, from the driver store.
static HRESULT system_device(ID3D11Device **device, D3D_FEATURE_LEVEL *level, ID3D11DeviceContext **context)
{
	static const D3D_FEATURE_LEVEL levels[] = { D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1,
						    D3D_FEATURE_LEVEL_10_0 };
	IDXGIAdapter *adapter = our_adapter();
	HMODULE code;
	HRESULT hr;

	if (!adapter) {
		printf("d3d11tri: DXGI lists no adapter %04x:%04x\n", OUR_VENDOR, OUR_DEVICE);
		return E_FAIL;
	}
	hr = D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, NULL, 0, levels, ARRAYSIZE(levels), D3D11_SDK_VERSION, device,
			       level, context);
	if (FAILED(hr))
		return hr;
	if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
				(LPCWSTR)(void *)(*device)->lpVtbl->CreateTexture2D, &code) ||
	    !windows_own("the device's code", code))
		return E_FAIL;
	if (!GetModuleHandleW(L"ruxnumd.dll")) {
		printf("d3d11tri: ruxnumd.dll is not loaded\n");
		return E_FAIL;
	}
	windows_own("ruxnumd.dll (from the driver store, not the system folder)", GetModuleHandleW(L"ruxnumd.dll"));
	return S_OK;
}

// compute: the compute shader of make_dxbc.py over four groups of 64, each
// thread writing its own number into a raw buffer; then the buffer read
// back. 0 when every word i holds i.
#define COMPUTE_GROUPS 4
#define COMPUTE_WORDS (COMPUTE_GROUPS * 64)

static int run_compute(ID3D11Device *device, ID3D11DeviceContext *context)
{
	D3D11_BUFFER_DESC bd;
	D3D11_UNORDERED_ACCESS_VIEW_DESC ud;
	D3D11_MAPPED_SUBRESOURCE mapped;
	ID3D11Buffer *buffer, *staging;
	ID3D11UnorderedAccessView *uav;
	ID3D11ComputeShader *cs;
	const UINT *words;
	UINT i, wrong = 0;

	memset(&bd, 0, sizeof(bd));
	bd.ByteWidth = COMPUTE_WORDS * 4;
	bd.Usage = D3D11_USAGE_DEFAULT;
	bd.BindFlags = D3D11_BIND_UNORDERED_ACCESS;
	bd.MiscFlags = D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS;
	CHECK("the compute buffer", ID3D11Device_CreateBuffer(device, &bd, NULL, &buffer));
	bd.Usage = D3D11_USAGE_STAGING;
	bd.BindFlags = 0;
	bd.MiscFlags = 0;
	bd.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
	CHECK("the staging buffer", ID3D11Device_CreateBuffer(device, &bd, NULL, &staging));
	memset(&ud, 0, sizeof(ud));
	ud.Format = DXGI_FORMAT_R32_TYPELESS;
	ud.ViewDimension = D3D11_UAV_DIMENSION_BUFFER;
	ud.Buffer.NumElements = COMPUTE_WORDS;
	ud.Buffer.Flags = D3D11_BUFFER_UAV_FLAG_RAW;
	CHECK("the unordered access view", ID3D11Device_CreateUnorderedAccessView(device, (ID3D11Resource *)buffer,
										   &ud, &uav));
	CHECK("the compute shader", ID3D11Device_CreateComputeShader(device, compute_cs_dxbc, sizeof(compute_cs_dxbc),
								     NULL, &cs));
	ID3D11DeviceContext_CSSetShader(context, cs, NULL, 0);
	ID3D11DeviceContext_CSSetUnorderedAccessViews(context, 0, 1, &uav, NULL);
	ID3D11DeviceContext_Dispatch(context, COMPUTE_GROUPS, 1, 1);
	ID3D11DeviceContext_CopyResource(context, (ID3D11Resource *)staging, (ID3D11Resource *)buffer);
	CHECK("the map", ID3D11DeviceContext_Map(context, (ID3D11Resource *)staging, 0, D3D11_MAP_READ, 0, &mapped));
	words = mapped.pData;
	for (i = 0; i < COMPUTE_WORDS; i++)
		if (words[i] != i) {
			if (wrong < 4)
				printf("d3d11tri: word %u is %u\n", i, words[i]);
			wrong++;
		}
	ID3D11DeviceContext_Unmap(context, (ID3D11Resource *)staging, 0);
	printf("d3d11tri: compute: %u of %u words wrong\n", wrong, COMPUTE_WORDS);
	return wrong ? 1 : 0;
}

int main(int argc, char **argv)
{
	static const float clear[4] = { 0.25f, 0.25f, 0.25f, 1.0f };
	ID3D11Device *device;
	ID3D11DeviceContext *context;
	ID3D11Texture2D *target, *staging;
	ID3D11RenderTargetView *view;
	ID3D11VertexShader *vs;
	ID3D11PixelShader *ps;
	ID3D11RasterizerState *raster;
	ID3D11DepthStencilState *depth;
	D3D_FEATURE_LEVEL level;
	D3D11_TEXTURE2D_DESC desc;
	D3D11_RASTERIZER_DESC rd;
	D3D11_DEPTH_STENCIL_DESC dd;
	D3D11_VIEWPORT viewport = { 0, 0, W, H, 0, 1 };
	D3D11_MAPPED_SUBRESOURCE mapped;
	ULONG lit, clear_count;
	WCHAR *slash;
	BOOL system, quad, clearview, compute;
	const char *out;

	quad = argc == 4 && !strcmp(argv[2], "quad");
	// clearview: after the triangle, ClearView with one rectangle (the
	// middle half) in red, as Direct2D and Notepad clear parts of a target:
	// DXMT draws that with its helper functions (wm_helpers.c).
	clearview = argc == 4 && !strcmp(argv[2], "clearview");
	compute = argc == 4 && !strcmp(argv[2], "compute");
	if ((argc != 3 && !quad && !clearview && !compute) || (strcmp(argv[1], "dxmt") && strcmp(argv[1], "system"))) {
		printf("usage: d3d11tri dxmt|system [quad|clearview|compute] OUT.ppm\n");
		return 2;
	}
	out = argv[argc - 1];
	system = !strcmp(argv[1], "system");
	GetModuleFileNameW(NULL, folder, MAX_PATH);
	slash = wcsrchr(folder, L'\\');
	if (!slash)
		return 1;
	*slash = 0;

	CHECK("D3D11CreateDevice", system ? system_device(&device, &level, &context) :
					     dxmt_device(&device, &level, &context));
	printf("d3d11tri: device at feature level 0x%x\n", level);
	if (compute)
		return run_compute(device, context);

	memset(&desc, 0, sizeof(desc));
	desc.Width = W;
	desc.Height = H;
	desc.MipLevels = 1;
	desc.ArraySize = 1;
	desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
	desc.SampleDesc.Count = 1;
	desc.Usage = D3D11_USAGE_DEFAULT;
	desc.BindFlags = D3D11_BIND_RENDER_TARGET | (quad ? D3D11_BIND_SHADER_RESOURCE : 0);
	CHECK("the render target", ID3D11Device_CreateTexture2D(device, &desc, NULL, &target));
	desc.Usage = D3D11_USAGE_STAGING;
	desc.BindFlags = 0;
	desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
	CHECK("the staging texture", ID3D11Device_CreateTexture2D(device, &desc, NULL, &staging));
	CHECK("the view", ID3D11Device_CreateRenderTargetView(device, (ID3D11Resource *)target, NULL, &view));
	CHECK("the vertex shader", ID3D11Device_CreateVertexShader(device, triangle_vs_dxbc, sizeof(triangle_vs_dxbc),
								  NULL, &vs));
	CHECK("the pixel shader", ID3D11Device_CreatePixelShader(device, triangle_ps_dxbc, sizeof(triangle_ps_dxbc),
								NULL, &ps));
	// Nothing culled and no depth test: Metal's defaults, as pvgtri draws.
	memset(&rd, 0, sizeof(rd));
	rd.FillMode = D3D11_FILL_SOLID;
	rd.CullMode = D3D11_CULL_NONE;
	rd.DepthClipEnable = TRUE;
	CHECK("the rasterizer state", ID3D11Device_CreateRasterizerState(device, &rd, &raster));
	memset(&dd, 0, sizeof(dd));
	dd.DepthEnable = FALSE;
	dd.DepthWriteMask = D3D11_DEPTH_WRITE_MASK_ZERO;
	dd.DepthFunc = D3D11_COMPARISON_ALWAYS;
	CHECK("the depth state", ID3D11Device_CreateDepthStencilState(device, &dd, &depth));

	ID3D11DeviceContext_OMSetRenderTargets(context, 1, &view, NULL);
	ID3D11DeviceContext_OMSetDepthStencilState(context, depth, 0);
	ID3D11DeviceContext_RSSetState(context, raster);
	ID3D11DeviceContext_RSSetViewports(context, 1, &viewport);
	ID3D11DeviceContext_IASetInputLayout(context, NULL);
	ID3D11DeviceContext_IASetPrimitiveTopology(context, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
	ID3D11DeviceContext_VSSetShader(context, vs, NULL, 0);
	ID3D11DeviceContext_PSSetShader(context, ps, NULL, 0);
	ID3D11DeviceContext_ClearRenderTargetView(context, view, clear);
	ID3D11DeviceContext_Draw(context, 3, 0);
	if (clearview) {
		static const float red[4] = { 1.0f, 0.0f, 0.0f, 1.0f };
		D3D11_RECT middle = { W / 4, H / 4, 3 * W / 4, 3 * H / 4 };
		ID3D11DeviceContext1 *context1;

		CHECK("ID3D11DeviceContext1", ID3D11DeviceContext_QueryInterface(context, &IID_ID3D11DeviceContext1,
										 (void **)&context1));
		ID3D11DeviceContext1_ClearView(context1, (ID3D11View *)view, red, &middle, 1);
		ID3D11DeviceContext1_Release(context1);
	}
	if (quad && draw_quad(device, context, target, &desc) != 0)
		return 1;
	ID3D11DeviceContext_CopyResource(context, (ID3D11Resource *)staging,
					 (ID3D11Resource *)(quad ? quad_target : target));
	CHECK("the map", ID3D11DeviceContext_Map(context, (ID3D11Resource *)staging, 0, D3D11_MAP_READ, 0, &mapped));
	printf("d3d11tri: mapped, %u bytes a row\n", mapped.RowPitch);
	if (!write_picture(out, mapped.pData, mapped.RowPitch, &lit, &clear_count)) {
		printf("d3d11tri: writing %s failed\n", out);
		return 1;
	}
	ID3D11DeviceContext_Unmap(context, (ID3D11Resource *)staging, 0);
	printf("d3d11tri: %lu of %d pixels not the clear colour, %lu of it, written to %s\n", lit, W * H, clear_count,
	       out);
	return lit && clear_count ? 0 : 1;
}
