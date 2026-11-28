// SPDX-License-Identifier: MIT
//
// kmtprobe: by hand, what DXGI plausibly does to tie a window to an output
// when it makes a swap chain, with each call's status and result on our
// adapter (item B: "Failed to find an output for the swapchain" at every
// swap-chain creation, runs rn99 to rn101). One line a fact, then
// "kmtprobe: done". The graphics kernel's calls are reached through
// gdi32's exports by name. Built against the SDK's d3dkmthk.h.
// Usage: kmtprobe [nofull]   (nofull: no SetFullscreenState)

#include <windows.h>
#include <winternl.h>
#include <d3dkmthk.h>
#include <d3d11.h>
#include <dxgi1_6.h>
#include <stdio.h>
#include <string.h>

typedef NTSTATUS(APIENTRY *kmt_fn)(void *);
static kmt_fn kmt(const char *name)
{
	static HMODULE gdi;

	if (!gdi)
		gdi = LoadLibraryW(L"gdi32.dll");
	return (kmt_fn)(void *)GetProcAddress(gdi, name);
}

// Calls D3DKMT`name` with `args`, printing its status; 0xdead if gdi32 has no such export.
static NTSTATUS call(const char *name, void *args)
{
	char full[96];

	snprintf(full, sizeof(full), "D3DKMT%s", name);
	kmt_fn f = kmt(full);
	NTSTATUS s = f ? f(args) : (NTSTATUS)0xdead;

	printf("%s: status 0x%08lx", name, (unsigned long)s);
	return s;
}

static void hex(const void *data, unsigned bytes)
{
	const unsigned char *b = (const unsigned char *)data;

	printf(" [");
	for (unsigned i = 0; i < bytes && i < 48; i++)
		printf("%02x", b[i]);
	printf("]");
}

static LRESULT CALLBACK window_proc(HWND w, UINT m, WPARAM wp, LPARAM lp)
{
	return DefWindowProcW(w, m, wp, lp);
}

static HWND make_window(int x, int y)
{
	static bool registered;
	WNDCLASSW c = {};

	if (!registered) {
		c.lpfnWndProc = window_proc;
		c.hInstance = GetModuleHandleW(NULL);
		c.lpszClassName = L"kmtprobe";
		c.hCursor = LoadCursor(NULL, IDC_ARROW);
		RegisterClassW(&c);
		registered = true;
	}
	HWND w = CreateWindowW(L"kmtprobe", L"kmtprobe", WS_OVERLAPPEDWINDOW | WS_VISIBLE, x, y, 480, 360, NULL, NULL,
			       GetModuleHandleW(NULL), NULL);
	MSG msg;
	for (int i = 0; i < 20; i++) {
		while (PeekMessageW(&msg, NULL, 0, 0, PM_REMOVE))
			DispatchMessageW(&msg);
		Sleep(10);
	}
	return w;
}

struct query {
	KMTQUERYADAPTERINFOTYPE type;
	const char *name;
	UINT size;
	bool source_first;	// the payload starts with the VidPnSourceId
};

#define Q(t, s, src) { KMTQAITYPE_##t, #t, sizeof(s), src }
static const struct query queries[] = {
	Q(UMDRIVERNAME, D3DKMT_UMDFILENAMEINFO, false),
	Q(DRIVERVERSION, D3DKMT_DRIVERVERSION, false),
	Q(ADAPTERTYPE, D3DKMT_ADAPTERTYPE, false),
	Q(KMD_DRIVER_VERSION, D3DKMT_KMD_DRIVER_VERSION, false),
	Q(WDDM_1_2_CAPS, D3DKMT_WDDM_1_2_CAPS, false),
	Q(WDDM_1_3_CAPS, D3DKMT_WDDM_1_3_CAPS, false),
	Q(WDDM_2_0_CAPS, D3DKMT_WDDM_2_0_CAPS, false),
	Q(WDDM_2_7_CAPS, D3DKMT_WDDM_2_7_CAPS, false),
	Q(WDDM_2_9_CAPS, D3DKMT_WDDM_2_9_CAPS, false),
	Q(WDDM_3_0_CAPS, D3DKMT_WDDM_3_0_CAPS, false),
	Q(DIRECTFLIP_SUPPORT, D3DKMT_DIRECTFLIP_SUPPORT, false),
	Q(INDEPENDENTFLIP_SUPPORT, D3DKMT_INDEPENDENTFLIP_SUPPORT, false),
	Q(MULTIPLANEOVERLAY_SUPPORT, D3DKMT_MULTIPLANEOVERLAY_SUPPORT, false),
	Q(MULTIPLANEOVERLAY_HUD_SUPPORT, D3DKMT_MULTIPLANEOVERLAY_HUD_SUPPORT, false),
	Q(MPO3DDI_SUPPORT, D3DKMT_MPO3DDI_SUPPORT, false),
	Q(MPOKERNELCAPS_SUPPORT, D3DKMT_MPOKERNELCAPS_SUPPORT, false),
	Q(PANELFITTER_SUPPORT, D3DKMT_PANELFITTER_SUPPORT, false),
	Q(DISPLAY_CAPS, D3DKMT_DISPLAY_CAPS, false),
	Q(DRIVERCAPS_EXT, D3DKMT_DRIVERCAPS_EXT, false),
	Q(PHYSICALADAPTERCOUNT, UINT, false),
	Q(CHECKDRIVERUPDATESTATUS, BOOL, false),
	Q(OUTPUTDUPLCONTEXTSCOUNT, D3DKMT_OUTPUTDUPLCONTEXTSCOUNT, true),
	Q(CURRENTDISPLAYMODE, D3DKMT_CURRENTDISPLAYMODE, true),
	Q(SCANOUT_CAPS, D3DKMT_QUERY_SCANOUT_CAPS, true),
};

static void query_adapter(D3DKMT_HANDLE adapter, UINT source)
{
	for (const struct query &q : queries) {
		unsigned char data[2048] = {};
		D3DKMT_QUERYADAPTERINFO a = {};

		if (q.source_first)
			memcpy(data, &source, sizeof(source));
		if (q.type == KMTQAITYPE_UMDRIVERNAME)
			((D3DKMT_UMDFILENAMEINFO *)data)->Version = KMTUMDVERSION_DX11;
		a.hAdapter = adapter;
		a.Type = q.type;
		a.pPrivateDriverData = data;
		a.PrivateDriverDataSize = q.size;
		NTSTATUS s = call("QueryAdapterInfo", &a);
		printf(" %s (%u bytes)", q.name, q.size);
		if (!s)
			hex(data, q.size);
		printf("\n");
	}
}

static void kernel_part(HWND window)
{
	HMONITOR monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONULL);
	MONITORINFOEXW info = {};

	info.cbSize = sizeof(info);
	printf("MonitorFromWindow: %p (nearest %p, primary %p)\n", (void *)monitor,
	       (void *)MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST),
	       (void *)MonitorFromWindow(window, MONITOR_DEFAULTTOPRIMARY));
	if (!GetMonitorInfoW(monitor ? monitor : MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST), &info))
		printf("GetMonitorInfo: failed %lu\n", GetLastError());
	printf("monitor: %ls %ld,%ld-%ld,%ld flags 0x%lx\n", info.szDevice, info.rcMonitor.left, info.rcMonitor.top,
	       info.rcMonitor.right, info.rcMonitor.bottom, info.dwFlags);
	DISPLAY_DEVICEW dd = {};
	dd.cb = sizeof(dd);
	for (DWORD i = 0; EnumDisplayDevicesW(NULL, i, &dd, 0); i++, dd.cb = sizeof(dd))
		printf("display device %lu: %ls \"%ls\" state 0x%lx\n", i, dd.DeviceName, dd.DeviceString, dd.StateFlags);

	D3DKMT_ENUMADAPTERS2 e = {};
	call("EnumAdapters2", &e);
	printf(" count %lu\n", (unsigned long)e.NumAdapters);
	D3DKMT_ADAPTERINFO list[16] = {};
	e.pAdapters = list;
	if (e.NumAdapters > 16)
		e.NumAdapters = 16;
	call("EnumAdapters2", &e);
	printf("\n");
	for (ULONG i = 0; i < e.NumAdapters; i++)
		printf("adapter %lu: handle 0x%x luid %08lx:%08lx sources %lu\n", i, list[i].hAdapter,
		       (unsigned long)list[i].AdapterLuid.HighPart, list[i].AdapterLuid.LowPart, list[i].NumOfSources);

	D3DKMT_OPENADAPTERFROMGDIDISPLAYNAME byname = {};
	wcsncpy(byname.DeviceName, info.szDevice, 31);
	call("OpenAdapterFromGdiDisplayName", &byname);
	printf(" %ls: handle 0x%x luid %08lx:%08lx source %u\n", byname.DeviceName, byname.hAdapter,
	       (unsigned long)byname.AdapterLuid.HighPart, byname.AdapterLuid.LowPart, byname.VidPnSourceId);
	D3DKMT_OPENADAPTERFROMHDC byhdc = {};
	byhdc.hDc = GetDC(window);
	call("OpenAdapterFromHdc", &byhdc);
	printf(" window's DC: handle 0x%x luid %08lx:%08lx source %u\n", byhdc.hAdapter,
	       (unsigned long)byhdc.AdapterLuid.HighPart, byhdc.AdapterLuid.LowPart, byhdc.VidPnSourceId);
	D3DKMT_OPENADAPTERFROMHDC bydc = {};
	bydc.hDc = CreateDCW(NULL, info.szDevice, NULL, NULL);
	call("OpenAdapterFromHdc", &bydc);
	printf(" monitor's DC: handle 0x%x luid %08lx:%08lx source %u\n", bydc.hAdapter,
	       (unsigned long)bydc.AdapterLuid.HighPart, bydc.AdapterLuid.LowPart, bydc.VidPnSourceId);

	D3DKMT_HANDLE adapter = byname.hAdapter;
	UINT source = byname.VidPnSourceId;
	if (!adapter)
		return;
	query_adapter(adapter, source);

	D3DKMT_GETDISPLAYMODELIST modes = {};
	modes.hAdapter = adapter;
	modes.VidPnSourceId = source;
	call("GetDisplayModeList", &modes);
	printf(" count %u\n", modes.ModeCount);
	static D3DKMT_DISPLAYMODE mode_list[256];
	if (modes.ModeCount > 256)
		modes.ModeCount = 256;
	modes.pModeList = mode_list;
	call("GetDisplayModeList", &modes);
	printf("\n");
	for (UINT i = 0; i < modes.ModeCount && i < 12; i++)
		printf("mode %u: %ux%u format %d refresh %u/%u integer %u scanline %d rotation %d\n", i,
		       mode_list[i].Width, mode_list[i].Height, mode_list[i].Format, mode_list[i].RefreshRate.Numerator,
		       mode_list[i].RefreshRate.Denominator, mode_list[i].IntegerRefreshRate,
		       mode_list[i].ScanLineOrdering, mode_list[i].DisplayOrientation);

	D3DKMT_CREATEDEVICE device = {};
	device.hAdapter = adapter;
	call("CreateDevice", &device);
	printf(" device 0x%x\n", device.hDevice);
	const D3DKMT_DEVICESTATE_TYPE types[] = { D3DKMT_DEVICESTATE_EXECUTION, D3DKMT_DEVICESTATE_PRESENT,
						  D3DKMT_DEVICESTATE_RESET, D3DKMT_DEVICESTATE_PRESENT_DWM,
						  D3DKMT_DEVICESTATE_PRESENT_QUEUE };
	for (D3DKMT_DEVICESTATE_TYPE t : types) {
		D3DKMT_GETDEVICESTATE st = {};
		st.hDevice = device.hDevice;
		st.StateType = t;
		st.PresentState.VidPnSourceId = source;
		call("GetDeviceState", &st);
		printf(" type %d", t);
		hex(&st.ExecutionState, 16);
		printf("\n");
	}
	D3DKMT_CHECKVIDPNEXCLUSIVEOWNERSHIP own = {};
	own.hAdapter = adapter;
	own.VidPnSourceId = source;
	call("CheckVidPnExclusiveOwnership", &own);
	printf("\n");
	kmt_fn exclusive = kmt("D3DKMTCheckExclusiveOwnership");
	printf("CheckExclusiveOwnership: %d\n", exclusive ? (int)(BOOLEAN)(ULONG_PTR)exclusive(NULL) : -1);
	D3DKMT_GETSCANLINE line = {};
	line.hAdapter = adapter;
	line.VidPnSourceId = source;
	call("GetScanLine", &line);
	printf(" in blank %d line %u\n", line.InVerticalBlank, line.ScanLine);
	D3DKMT_WAITFORVERTICALBLANKEVENT vb = {};
	vb.hAdapter = adapter;
	vb.hDevice = device.hDevice;
	vb.VidPnSourceId = source;
	ULONGLONG t0 = GetTickCount64();
	call("WaitForVerticalBlankEvent", &vb);
	printf(" after %llu ms\n", GetTickCount64() - t0);
	D3DKMT_CHECKMONITORPOWERSTATE power = {};
	power.hAdapter = adapter;
	power.VidPnSourceId = source;
	call("CheckMonitorPowerState", &power);
	printf("\n");
	D3DKMT_CHECKOCCLUSION occ = {};
	occ.hWindow = window;
	call("CheckOcclusion", &occ);
	printf("\n");
	D3DKMT_GETMULTISAMPLEMETHODLIST ms = {};
	ms.hAdapter = adapter;
	ms.VidPnSourceId = source;
	ms.Width = 1024;
	ms.Height = 768;
	ms.Format = D3DDDIFMT_A8R8G8B8;
	call("GetMultisampleMethodList", &ms);
	printf(" count %u\n", ms.MethodCount);
	D3DKMT_GETSHAREDPRIMARYHANDLE primary = {};
	primary.hAdapter = adapter;
	primary.VidPnSourceId = source;
	call("GetSharedPrimaryHandle", &primary);
	printf(" handle 0x%x\n", primary.hSharedPrimary);
}

static void output_line(IDXGIOutput *output, HMONITOR window_monitor)
{
	DXGI_OUTPUT_DESC d;
	output->GetDesc(&d);
	printf("  output %ls attached %d %ld,%ld-%ld,%ld rotation %d monitor %p (window's %p)\n", d.DeviceName,
	       d.AttachedToDesktop, d.DesktopCoordinates.left, d.DesktopCoordinates.top, d.DesktopCoordinates.right,
	       d.DesktopCoordinates.bottom, d.Rotation, (void *)d.Monitor, (void *)window_monitor);
	IDXGIOutput6 *o6;
	if (SUCCEEDED(output->QueryInterface(__uuidof(IDXGIOutput6), (void **)&o6))) {
		DXGI_OUTPUT_DESC1 d1;
		HRESULT hr = o6->GetDesc1(&d1);
		printf("  GetDesc1 0x%08lx bits %u colour space %d\n", hr, d1.BitsPerColor, d1.ColorSpace);
		UINT support = 0;
		hr = o6->CheckHardwareCompositionSupport(&support);
		printf("  CheckHardwareCompositionSupport 0x%08lx flags 0x%x\n", hr, support);
		o6->Release();
	}
	const DXGI_FORMAT formats[] = { DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM };
	for (DXGI_FORMAT f : formats) {
		UINT count = 0;
		HRESULT hr = output->GetDisplayModeList(f, 0, &count, NULL);
		printf("  GetDisplayModeList format %d: 0x%08lx count %u\n", f, hr, count);
	}
	DXGI_MODE_DESC want = {}, got = {};
	want.Width = 1024;
	want.Height = 768;
	want.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
	HRESULT hr = output->FindClosestMatchingMode(&want, &got, NULL);
	printf("  FindClosestMatchingMode 0x%08lx %ux%u %u/%u\n", hr, got.Width, got.Height, got.RefreshRate.Numerator,
	       got.RefreshRate.Denominator);
}

static void containing(IDXGISwapChain *chain, const char *what)
{
	IDXGIOutput *output = NULL;
	HRESULT hr = chain->GetContainingOutput(&output);
	DXGI_OUTPUT_DESC d = {};
	if (output)
		output->GetDesc(&d);
	printf("%s: GetContainingOutput 0x%08lx %ls\n", what, hr, d.DeviceName);
	if (output)
		output->Release();
}

static void fullscreen(IDXGISwapChain *chain, ID3D11Device *device, const char *what)
{
	HRESULT hr = chain->SetFullscreenState(TRUE, NULL);
	BOOL on = FALSE;
	chain->GetFullscreenState(&on, NULL);
	printf("%s: SetFullscreenState(TRUE) 0x%08lx, now %d\n", what, hr, on);
	if (SUCCEEDED(hr)) {
		ID3D11Texture2D *back;
		if (SUCCEEDED(chain->GetBuffer(0, __uuidof(ID3D11Texture2D), (void **)&back))) {
			ID3D11RenderTargetView *rtv;
			ID3D11DeviceContext *ctx;
			device->GetImmediateContext(&ctx);
			if (SUCCEEDED(device->CreateRenderTargetView(back, NULL, &rtv))) {
				const float green[4] = { 0, 0.6f, 0.2f, 1 };
				ctx->ClearRenderTargetView(rtv, green);
				rtv->Release();
			}
			ctx->Release();
			back->Release();
		}
		printf("%s: Present in full screen 0x%08lx\n", what, chain->Present(1, 0));
		Sleep(1500);
		printf("%s: SetFullscreenState(FALSE) 0x%08lx\n", what, chain->SetFullscreenState(FALSE, NULL));
	}
}

static void dxgi_part(bool full)
{
	IDXGIFactory2 *factory;
	if (FAILED(CreateDXGIFactory1(__uuidof(IDXGIFactory2), (void **)&factory))) {
		printf("CreateDXGIFactory1 failed\n");
		return;
	}
	HWND one = make_window(60, 60), two = make_window(400, 200);
	HMONITOR monitor = MonitorFromWindow(one, MONITOR_DEFAULTTONEAREST);
	IDXGIAdapter1 *adapter, *ours = NULL;
	for (UINT a = 0; factory->EnumAdapters1(a, &adapter) == S_OK; a++) {
		DXGI_ADAPTER_DESC1 d;
		adapter->GetDesc1(&d);
		printf("DXGI adapter %u: %ls luid %08lx:%08lx flags 0x%x\n", a, d.Description,
		       (unsigned long)d.AdapterLuid.HighPart, d.AdapterLuid.LowPart, d.Flags);
		IDXGIOutput *output;
		for (UINT o = 0; adapter->EnumOutputs(o, &output) == S_OK; o++) {
			output_line(output, monitor);
			output->Release();
		}
		if (!ours)
			ours = adapter;
		else
			adapter->Release();
	}
	ID3D11Device *device = NULL;
	HRESULT hr = D3D11CreateDevice(ours, D3D_DRIVER_TYPE_UNKNOWN, NULL, 0, NULL, 0, D3D11_SDK_VERSION, &device,
				       NULL, NULL);
	printf("D3D11CreateDevice on adapter 0: 0x%08lx\n", hr);
	if (FAILED(hr))
		return;

	DXGI_SWAP_CHAIN_DESC blt = {};
	blt.BufferDesc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
	blt.SampleDesc.Count = 1;
	blt.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
	blt.BufferCount = 1;
	blt.OutputWindow = one;
	blt.Windowed = TRUE;
	blt.SwapEffect = DXGI_SWAP_EFFECT_DISCARD;
	IDXGISwapChain *chain = NULL;
	hr = factory->CreateSwapChain(device, &blt, &chain);
	printf("CreateSwapChain blt model: 0x%08lx\n", hr);
	if (chain) {
		containing(chain, "blt");
		if (full)
			fullscreen(chain, device, "blt");
		chain->Release();
	}

	DXGI_SWAP_CHAIN_DESC1 flip = {};
	flip.Width = 1024;
	flip.Height = 768;
	flip.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
	flip.SampleDesc.Count = 1;
	flip.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
	flip.BufferCount = 2;
	flip.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
	IDXGISwapChain1 *chain1 = NULL;
	hr = factory->CreateSwapChainForHwnd(device, two, &flip, NULL, NULL, &chain1);
	printf("CreateSwapChainForHwnd flip model: 0x%08lx\n", hr);
	if (chain1) {
		containing(chain1, "flip");
		if (full)
			fullscreen(chain1, device, "flip");
		chain1->Release();
	}
	device->Release();
	factory->Release();
}

int main(int argc, char **argv)
{
	bool full = !(argc > 1 && !strcmp(argv[1], "nofull"));

	setvbuf(stdout, NULL, _IONBF, 0);
	HWND window = make_window(100, 100);
	kernel_part(window);
	dxgi_part(full);
	printf("kmtprobe: done\n");
	return 0;
}
