// SPDX-License-Identifier: MIT
//
// outprobe: what DXGI finds of the displays, for item B's route through
// Windows' own upgrade of blt-model windows (run rn99: the swap chain of
// Tutorial 7 was not upgraded, WinSwapEffectUpgradeReason 2, and DXGI
// logged "Failed to find an output for the swapchain"). Prints the
// adapters and their outputs, GDI's display devices, tearing support, and
// for a blt-model and a flip-model swap chain on a window of its own the
// output that contains it. One line a fact, then "outprobe: done".
// With "hidden", the swap chains' windows are made hidden, as a program
// that shows its window after making its swap chain does.
// Usage: outprobe [hidden]

#include <windows.h>
#include <d3d11.h>
#include <dxgi1_6.h>
#include <stdio.h>
#include <string.h>

static void list_adapters(IDXGIFactory1 *factory)
{
	IDXGIAdapter1 *adapter;
	for (UINT a = 0; factory->EnumAdapters1(a, &adapter) == S_OK; a++) {
		DXGI_ADAPTER_DESC1 d;
		adapter->GetDesc1(&d);
		printf("adapter %u: %ls vendor 0x%04x device 0x%04x luid %08lx:%08lx flags 0x%x\n", a, d.Description,
		       d.VendorId, d.DeviceId, (unsigned long)d.AdapterLuid.HighPart,
		       (unsigned long)d.AdapterLuid.LowPart, d.Flags);
		IDXGIOutput *output;
		for (UINT o = 0; adapter->EnumOutputs(o, &output) == S_OK; o++) {
			DXGI_OUTPUT_DESC od;
			output->GetDesc(&od);
			printf("  output %u: %ls attached %d rect %ld,%ld-%ld,%ld monitor %p\n", o, od.DeviceName,
			       od.AttachedToDesktop, od.DesktopCoordinates.left, od.DesktopCoordinates.top,
			       od.DesktopCoordinates.right, od.DesktopCoordinates.bottom, (void *)od.Monitor);
			output->Release();
		}
		adapter->Release();
	}
}

static void list_gdi_displays(void)
{
	DISPLAY_DEVICEW dd;
	dd.cb = sizeof(dd);
	for (DWORD i = 0; EnumDisplayDevicesW(NULL, i, &dd, 0); i++)
		printf("gdi display %lu: %ls \"%ls\" state 0x%lx\n", (unsigned long)i, dd.DeviceName, dd.DeviceString,
		       (unsigned long)dd.StateFlags);
}

static void tearing(IDXGIFactory1 *factory)
{
	IDXGIFactory5 *f5;
	BOOL allow = FALSE;
	if (factory->QueryInterface(__uuidof(IDXGIFactory5), (void **)&f5) != S_OK) {
		printf("IDXGIFactory5: none\n");
		return;
	}
	HRESULT hr = f5->CheckFeatureSupport(DXGI_FEATURE_PRESENT_ALLOW_TEARING, &allow, sizeof(allow));
	printf("tearing: 0x%08lx allow %d\n", (unsigned long)hr, allow);
	f5->Release();
}

// A window of its own, a swap chain of the given effect, and the output
// that contains it.
static DWORD window_style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;

static void swap_chain(const char *name, DXGI_SWAP_EFFECT effect, UINT buffers)
{
	HWND window = CreateWindowExW(0, L"STATIC", L"outprobe", window_style, 40, 40, 640, 480,
				      NULL, NULL, GetModuleHandleW(NULL), NULL);
	DXGI_SWAP_CHAIN_DESC desc = {};
	desc.BufferDesc.Width = 640;
	desc.BufferDesc.Height = 480;
	desc.BufferDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
	desc.SampleDesc.Count = 1;
	desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
	desc.BufferCount = buffers;
	desc.OutputWindow = window;
	desc.Windowed = TRUE;
	desc.SwapEffect = effect;
	IDXGISwapChain *chain = NULL;
	ID3D11Device *device = NULL;
	ID3D11DeviceContext *context = NULL;
	HRESULT hr = D3D11CreateDeviceAndSwapChain(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, NULL, 0,
						   D3D11_SDK_VERSION, &desc, &chain, &device, NULL, &context);
	printf("%s: create 0x%08lx\n", name, (unsigned long)hr);
	if (SUCCEEDED(hr)) {
		IDXGIOutput *output = NULL;
		hr = chain->GetContainingOutput(&output);
		printf("%s: containing output 0x%08lx\n", name, (unsigned long)hr);
		if (output) {
			DXGI_OUTPUT_DESC od;
			output->GetDesc(&od);
			printf("%s: output %ls\n", name, od.DeviceName);
			output->Release();
		}
		hr = chain->Present(1, 0);
		printf("%s: present 0x%08lx\n", name, (unsigned long)hr);
		chain->Release();
		context->Release();
		device->Release();
	}
	DestroyWindow(window);
}

int main(int argc, char **argv)
{
	if (argc > 1 && !strcmp(argv[1], "hidden"))
		window_style = WS_OVERLAPPEDWINDOW;
	IDXGIFactory1 *factory = NULL;
	HRESULT hr = CreateDXGIFactory1(__uuidof(IDXGIFactory1), (void **)&factory);
	printf("factory: 0x%08lx\n", (unsigned long)hr);
	if (FAILED(hr))
		return 1;
	list_adapters(factory);
	list_gdi_displays();
	tearing(factory);
	factory->Release();
	swap_chain("blt", DXGI_SWAP_EFFECT_DISCARD, 1);
	swap_chain("flip", DXGI_SWAP_EFFECT_FLIP_DISCARD, 2);
	printf("outprobe: done\n");
	return 0;
}
