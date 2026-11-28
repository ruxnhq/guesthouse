// SPDX-License-Identifier: MIT
//
// memgrow: one process's GPU memory grown in steps to about 256 MB, every
// step drawn from its newest memory. Edge's GPU process faulted the Mac's
// GPU after its winemetal memory had grown to about 160 MB (runs rn75,
// rn76), more than any other process had held; this asks whether growth
// alone does it, in a program that logs to a file.
//
// One small vertex buffer and one index buffer are made at the start and
// never replaced. Each step makes only a texture of one colour (sampled)
// and a 64x64 render target, draws the triangle sampling the texture into
// the target, and reads the target's middle back. Everything stays alive.
// It stops at the first step that fails or reads a wrong colour, before
// making anything else (run rn79: growing buffers likely turned the same
// aliasing into a GPU fault).
//
// Usage: memgrow [MEGABYTES [TEXTURE_SIDE]]   (256, 1024; a side of 16
// makes many tiny textures, which tests a count, 2000 few huge ones)

#include <windows.h>
#include <d3d11.h>
#include <d3dcompiler.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdarg.h>

extern "C" {
#include "pvg_kmt.h"
}

#define RT_SIDE		64
#define MOST_STEPS	10000

static UINT TEX_SIDE = 1024;

typedef HRESULT(WINAPI *COMPILE)(LPCVOID, SIZE_T, LPCSTR, const D3D_SHADER_MACRO *, ID3DInclude *, LPCSTR,
				 LPCSTR, UINT, UINT, ID3DBlob **, ID3DBlob **);

static const char vs_text[] =
	"struct o { float4 p : SV_Position; float2 t : TEXCOORD0; };"
	"o main(float2 p : POSITION, float2 t : TEXCOORD0) { o r; r.p = float4(p, 0, 1); r.t = t; return r; }";
static const char ps_text[] =
	"Texture2D tex : register(t0); SamplerState s : register(s0);"
	"float4 main(float4 p : SV_Position, float2 t : TEXCOORD0) : SV_Target { return tex.Sample(s, t); }";

struct vertex {
	float x, y, u, v;
};

// Each line to the console and to the driver's kernel log, which keeps what
// was said when a run ends at once after a GPU fault.
static bool kernel_log;	// the line also goes to the driver's kernel log (klog.c)

static void say(const char *format, ...)
{
	char line[512];
	va_list args;
	int n;

	va_start(args, format);
	n = vsnprintf(line, sizeof(line), format, args);
	va_end(args);
	if (n < 0)
		return;
	if (n >= (int)sizeof(line))
		n = sizeof(line) - 1;
	fputs(line, stdout);
	if (kernel_log)
		pvg_kmt_log(line, (unsigned)(n && line[n - 1] == '\n' ? n - 1 : n));
}

static void quiet(const char *line)
{
	(void)line;
}

static ID3DBlob *compile(COMPILE fn, const char *text, const char *target)
{
	ID3DBlob *code = NULL, *errors = NULL;

	if (FAILED(fn(text, strlen(text), NULL, NULL, NULL, "main", target, 0, 0, &code, &errors))) {
		say("FAILED: compile %s: %s\n", target, errors ? (char *)errors->GetBufferPointer() : "?");
		return NULL;
	}
	return code;
}

static UINT32 colour(int step)
{
	return 0xff000000u | (UINT32)((step * 37) & 255) << 16 | (UINT32)((step * 91) & 255) << 8 |
	       (UINT32)((step * 53 + 64) & 255);
}

int main(int argc, char **argv)
{
	unsigned most_mb = argc > 1 ? (unsigned)atoi(argv[1]) : 256;
	HMODULE dll = LoadLibraryA("d3dcompiler_47.dll");
	COMPILE fn = dll ? (COMPILE)(void *)GetProcAddress(dll, "D3DCompile") : NULL;
	ID3D11Device *d3d;
	ID3D11DeviceContext *ctx;
	D3D_FEATURE_LEVEL level;
	ID3DBlob *vs_code, *ps_code;
	ID3D11VertexShader *vs;
	ID3D11PixelShader *ps;
	ID3D11InputLayout *layout;
	ID3D11SamplerState *sampler;
	unsigned long long total_bytes = 0;
	unsigned total = 0;
	int step;

	setvbuf(stdout, NULL, _IONBF, 0);
	if (argc > 2)
		TEX_SIDE = (UINT)atoi(argv[2]);
	// winemetal says every texture it makes, with its number and place.
	SetEnvironmentVariableA("RUXN_LOG_TEXTURES", "1");
	// The lines go to the driver's kernel log as well (wddmtool klogfollow).
	kernel_log = pvg_kmt_open(quiet);
	if (!fn || FAILED(D3D11CreateDevice(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, NULL, 0, D3D11_SDK_VERSION, &d3d,
					    &level, &ctx))) {
		say("FAILED: no device or no compiler\n");
		return 1;
	}
	vs_code = compile(fn, vs_text, "vs_4_0");
	ps_code = compile(fn, ps_text, "ps_4_0");
	if (!vs_code || !ps_code)
		return 1;
	D3D11_INPUT_ELEMENT_DESC elements[] = {
		{"POSITION", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 0, D3D11_INPUT_PER_VERTEX_DATA, 0},
		{"TEXCOORD", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 8, D3D11_INPUT_PER_VERTEX_DATA, 0},
	};
	D3D11_SAMPLER_DESC sd = {};
	sd.Filter = D3D11_FILTER_MIN_MAG_MIP_POINT;
	sd.AddressU = sd.AddressV = sd.AddressW = D3D11_TEXTURE_ADDRESS_CLAMP;
	sd.MaxLOD = D3D11_FLOAT32_MAX;
	if (FAILED(d3d->CreateVertexShader(vs_code->GetBufferPointer(), vs_code->GetBufferSize(), NULL, &vs)) ||
	    FAILED(d3d->CreatePixelShader(ps_code->GetBufferPointer(), ps_code->GetBufferSize(), NULL, &ps)) ||
	    FAILED(d3d->CreateInputLayout(elements, 2, vs_code->GetBufferPointer(), vs_code->GetBufferSize(), &layout)) ||
	    FAILED(d3d->CreateSamplerState(&sd, &sampler))) {
		say("FAILED: shaders or states\n");
		return 1;
	}
	say("device at feature level %x, process %lu, textures %u wide\n", level, GetCurrentProcessId(), TEX_SIDE);

	// The one vertex and index buffer: a triangle over the whole target.
	struct vertex tri[3] = {{-1, -1, 0, 1}, {-1, 3, 0, -1}, {3, -1, 2, 1}};
	UINT32 indices[3] = {0, 1, 2};
	ID3D11Buffer *vb, *ib;
	ID3D11Texture2D *staging;
	D3D11_BUFFER_DESC bd = {};
	D3D11_SUBRESOURCE_DATA init = {};
	D3D11_TEXTURE2D_DESC td = {};
	UINT stride = sizeof(struct vertex), zero = 0;
	D3D11_VIEWPORT vp = {0, 0, RT_SIDE, RT_SIDE, 0, 1};

	bd.Usage = D3D11_USAGE_DEFAULT;
	bd.ByteWidth = sizeof(tri);
	bd.BindFlags = D3D11_BIND_VERTEX_BUFFER;
	init.pSysMem = tri;
	if (FAILED(d3d->CreateBuffer(&bd, &init, &vb)))
		return 1;
	bd.ByteWidth = sizeof(indices);
	bd.BindFlags = D3D11_BIND_INDEX_BUFFER;
	init.pSysMem = indices;
	if (FAILED(d3d->CreateBuffer(&bd, &init, &ib)))
		return 1;
	td.Width = td.Height = RT_SIDE;
	td.MipLevels = td.ArraySize = 1;
	td.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
	td.SampleDesc.Count = 1;
	td.Usage = D3D11_USAGE_STAGING;
	td.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
	if (FAILED(d3d->CreateTexture2D(&td, NULL, &staging)))
		return 1;

	static ID3D11ShaderResourceView *views[MOST_STEPS + 1];
	static ID3D11RenderTargetView *targets[MOST_STEPS + 1];
	static ID3D11Texture2D *rts[MOST_STEPS + 1];
	UINT32 *texels = (UINT32 *)malloc((size_t)TEX_SIDE * TEX_SIDE * 4);

	// Draws step k's texture into target t and reads the middle back.
	auto draw = [&](int k, int t) -> UINT32 {
		D3D11_MAPPED_SUBRESOURCE m;
		UINT32 got = 0xdeadbeef;

		ctx->IASetInputLayout(layout);
		ctx->IASetVertexBuffers(0, 1, &vb, &stride, &zero);
		ctx->IASetIndexBuffer(ib, DXGI_FORMAT_R32_UINT, 0);
		ctx->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
		ctx->VSSetShader(vs, NULL, 0);
		ctx->PSSetShader(ps, NULL, 0);
		ctx->PSSetShaderResources(0, 1, &views[k]);
		ctx->PSSetSamplers(0, 1, &sampler);
		ctx->OMSetRenderTargets(1, &targets[t], NULL);
		ctx->RSSetViewports(1, &vp);
		ctx->DrawIndexed(3, 0, 0);
		ctx->CopyResource(staging, rts[t]);
		if (SUCCEEDED(ctx->Map(staging, 0, D3D11_MAP_READ, 0, &m))) {
			got = ((UINT32 *)((BYTE *)m.pData + (RT_SIDE / 2) * m.RowPitch))[RT_SIDE / 2];
			ctx->Unmap(staging, 0);
		}
		return got;
	};

	for (step = 1; step <= MOST_STEPS && total < most_mb; step++) {
		ID3D11Texture2D *tex;
		UINT32 want = colour(step), got;
		size_t i;

		for (i = 0; i < (size_t)TEX_SIDE * TEX_SIDE; i++)
			texels[i] = want;
		td.Width = td.Height = TEX_SIDE;
		td.Usage = D3D11_USAGE_DEFAULT;
		td.CPUAccessFlags = 0;
		td.BindFlags = D3D11_BIND_SHADER_RESOURCE;
		init.pSysMem = texels;
		init.SysMemPitch = TEX_SIDE * 4;
		if (FAILED(d3d->CreateTexture2D(&td, &init, &tex)) ||
		    FAILED(d3d->CreateShaderResourceView(tex, NULL, &views[step]))) {
			say("FAILED: step %d: texture\n", step);
			return 2;
		}
		td.Width = td.Height = RT_SIDE;
		td.BindFlags = D3D11_BIND_RENDER_TARGET;
		if (FAILED(d3d->CreateTexture2D(&td, NULL, &rts[step])) ||
		    FAILED(d3d->CreateRenderTargetView(rts[step], NULL, &targets[step]))) {
			say("FAILED: step %d: target\n", step);
			return 2;
		}
		got = draw(step, step);
		total_bytes += (unsigned long long)TEX_SIDE * TEX_SIDE * 4 + RT_SIDE * RT_SIDE * 4;
		total = (unsigned)(total_bytes >> 20);
		say("step %d: %llu KB alive, middle %08x, wanted %08x: %s\n", step, total_bytes >> 10, got, want,
		    got == want ? "ok" : "WRONG");
		if (got == want)
			continue;
		// Which texture was reached: each earlier one drawn again into
		// the first target, nothing new made.
		for (int k = 1; k < step; k++) {
			UINT32 again = draw(k, 1);

			if (again == want || again != colour(k) || got == colour(k))
				say("  step %d's texture drawn again: %08x, its colour %08x%s\n", k, again, colour(k),
				    got == colour(k) ? "  <- the colour the failing step read" :
				    again == colour(k) ? "" : "  WRONG");
		}
		return 3;
	}
	say("done: %llu KB in %d steps, every one right\n", total_bytes >> 10, step - 1);
	return 0;
}
