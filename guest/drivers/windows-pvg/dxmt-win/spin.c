// SPDX-License-Identifier: MIT
//
// spin: a window whose colour changes every frame, for a while, so that the
// compositor has something to compose at every vertical blank. It draws
// through Windows' own Direct3D 11 on the default adapter (ruxnwddm's,
// when its output is the only one) into a flip-model swap chain, presents
// with the vertical blank, and prints how many frames it presented a
// second. The driver's count of swaps over the same time says how many the
// display showed (wddmtool pvg before and after).
//
// Usage: spin SECONDS [X Y]   the window's top left corner, 100 100 if not given

#define COBJMACROS
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d11.h>
#include <dxgi1_2.h>
#include <stdio.h>
#include <stdlib.h>

#define WIDTH		480
#define HEIGHT		320
#define BUFFERS		2
#define MOST_SECONDS	600

static LRESULT CALLBACK window_proc(HWND window, UINT message, WPARAM w, LPARAM l)
{
	if (message == WM_DESTROY) {
		PostQuitMessage(0);
		return 0;
	}
	return DefWindowProcW(window, message, w, l);
}

static HWND make_window(int x, int y)
{
	WNDCLASSW c = { 0 };

	c.lpfnWndProc = window_proc;
	c.hInstance = GetModuleHandleW(NULL);
	c.lpszClassName = L"ruxnspin";
	c.hCursor = LoadCursor(NULL, IDC_ARROW);
	if (!RegisterClassW(&c))
		return NULL;
	return CreateWindowExW(0, c.lpszClassName, L"spin", WS_OVERLAPPEDWINDOW | WS_VISIBLE, x, y, WIDTH,
			       HEIGHT, NULL, NULL, c.hInstance, NULL);
}

// A colour that goes round the hue circle once every two seconds at 60 a second.
static void colour_of(unsigned frame, float rgba[4])
{
	float t = (float)(frame % 120) / 120.0f * 6.0f;
	int sector = (int)t;
	float f = t - (float)sector;

	rgba[3] = 1.0f;
	switch (sector) {
	case 0: rgba[0] = 1; rgba[1] = f; rgba[2] = 0; break;
	case 1: rgba[0] = 1 - f; rgba[1] = 1; rgba[2] = 0; break;
	case 2: rgba[0] = 0; rgba[1] = 1; rgba[2] = f; break;
	case 3: rgba[0] = 0; rgba[1] = 1 - f; rgba[2] = 1; break;
	case 4: rgba[0] = f; rgba[1] = 0; rgba[2] = 1; break;
	default: rgba[0] = 1; rgba[1] = 0; rgba[2] = 1 - f; break;
	}
}

int main(int argc, char **argv)
{
	DXGI_SWAP_CHAIN_DESC desc = { 0 };
	ID3D11Device *device = NULL;
	ID3D11DeviceContext *context = NULL;
	IDXGISwapChain *chain = NULL;
	ID3D11Texture2D *back = NULL;
	ID3D11RenderTargetView *view = NULL;
	LARGE_INTEGER start, now, ticks;
	unsigned frames = 0, seconds;
	HRESULT hr;
	HWND window;
	MSG m;

	if (argc != 2 && argc != 4) {
		fprintf(stderr, "usage: spin SECONDS [X Y]\n");
		return 2;
	}
	seconds = (unsigned)atoi(argv[1]);
	if (!seconds || seconds > MOST_SECONDS) {
		fprintf(stderr, "spin: 1 to %d seconds\n", MOST_SECONDS);
		return 2;
	}
	window = make_window(argc == 4 ? atoi(argv[2]) : 100, argc == 4 ? atoi(argv[3]) : 100);
	if (!window) {
		fprintf(stderr, "spin: no window\n");
		return 1;
	}
	desc.BufferDesc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
	desc.SampleDesc.Count = 1;
	desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
	desc.BufferCount = BUFFERS;
	desc.OutputWindow = window;
	desc.Windowed = TRUE;
	desc.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
	hr = D3D11CreateDeviceAndSwapChain(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, NULL, 0, D3D11_SDK_VERSION, &desc,
					   &chain, &device, NULL, &context);
	printf("D3D11CreateDeviceAndSwapChain: 0x%08lx\n", (unsigned long)hr);
	if (FAILED(hr))
		return 1;
	hr = IDXGISwapChain_GetBuffer(chain, 0, &IID_ID3D11Texture2D, (void **)&back);
	if (SUCCEEDED(hr))
		hr = ID3D11Device_CreateRenderTargetView(device, (ID3D11Resource *)back, NULL, &view);
	if (FAILED(hr)) {
		printf("render target: 0x%08lx\n", (unsigned long)hr);
		return 1;
	}
	QueryPerformanceFrequency(&ticks);
	QueryPerformanceCounter(&start);
	for (;;) {
		float rgba[4];

		while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) {
			TranslateMessage(&m);
			DispatchMessageW(&m);
		}
		QueryPerformanceCounter(&now);
		if ((now.QuadPart - start.QuadPart) / ticks.QuadPart >= (LONGLONG)seconds)
			break;
		colour_of(frames, rgba);
		ID3D11DeviceContext_OMSetRenderTargets(context, 1, &view, NULL);
		ID3D11DeviceContext_ClearRenderTargetView(context, view, rgba);
		hr = IDXGISwapChain_Present(chain, 1, 0);
		if (FAILED(hr)) {
			printf("Present: 0x%08lx after %u frames\n", (unsigned long)hr, frames);
			break;
		}
		frames++;
	}
	printf("frames %u in %u s: %.1f a second\n", frames, seconds, (double)frames / seconds);
	ID3D11RenderTargetView_Release(view);
	ID3D11Texture2D_Release(back);
	IDXGISwapChain_Release(chain);
	ID3D11DeviceContext_Release(context);
	ID3D11Device_Release(device);
	DestroyWindow(window);
	return 0;
}
