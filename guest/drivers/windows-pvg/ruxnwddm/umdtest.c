// SPDX-License-Identifier: GPL-2.0
//
// umdtest: checks, in the guest, the two things the compositor needs of
// ruxnumd.dll before it will use our adapter:
//
//   umdtest load PATH     loads the user-mode driver as Windows would and
//                         finds OpenAdapter10_2 in it
//   umdtest device LOG    makes a Direct3D 11 device on the adapter whose
//                         PCI ids are ours, with the user-mode driver's
//                         log (RUXNUMD_LOG) going to the file LOG, and
//                         prints the result and the feature level
//
// Built without a C runtime, like wddmtool. Exit status 0 when all went
// well, 1 when not, 2 for a usage error.

#define COBJMACROS
#define CINTERFACE
#include <windows.h>
#include <shellapi.h>
#include <initguid.h>
#include <dxgi.h>
#include <d3d11.h>

#define OUR_VENDOR	0x106B
#define OUR_DEVICE	0xEEEE

static char out_buf[512];
static unsigned int out_len;

static void flush(void)
{
	DWORD written;

	if (out_len)
		WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), out_buf, out_len, &written, NULL);
	out_len = 0;
}

static void put_str(const char *s)
{
	while (*s) {
		if (out_len == sizeof(out_buf))
			flush();
		out_buf[out_len++] = *s++;
	}
}

static void put_hex(const char *name, ULONG64 value)
{
	static const char hex[] = "0123456789abcdef";
	char text[19];
	int i;

	text[0] = '0';
	text[1] = 'x';
	for (i = 0; i < 16; i++)
		text[2 + i] = hex[(value >> (60 - 4 * i)) & 0xf];
	text[18] = 0;
	put_str(name);
	put_str(" ");
	put_str(text);
	put_str("\r\n");
}

static int load(const WCHAR *path)
{
	HMODULE dll = LoadLibraryW(path);
	FARPROC open;

	put_hex("load error", dll ? 0 : GetLastError());
	if (!dll)
		return 1;
	open = GetProcAddress(dll, "OpenAdapter10_2");
	put_hex("OpenAdapter10_2", (ULONG64)(ULONG_PTR)open);
	return open ? 0 : 1;
}

static IDXGIAdapter1 *our_adapter(void)
{
	IDXGIFactory1 *factory;
	IDXGIAdapter1 *adapter;
	UINT i;

	if (FAILED(CreateDXGIFactory1(&IID_IDXGIFactory1, (void **)&factory)))
		return NULL;
	for (i = 0; IDXGIFactory1_EnumAdapters1(factory, i, &adapter) == S_OK; i++) {
		DXGI_ADAPTER_DESC1 desc;

		IDXGIAdapter1_GetDesc1(adapter, &desc);
		put_hex("adapter vendor", desc.VendorId);
		put_hex("adapter device", desc.DeviceId);
		if (desc.VendorId == OUR_VENDOR && desc.DeviceId == OUR_DEVICE) {
			IDXGIFactory1_Release(factory);
			return adapter;
		}
		IDXGIAdapter1_Release(adapter);
	}
	IDXGIFactory1_Release(factory);
	return NULL;
}

static int device(const WCHAR *log)
{
	static const D3D_FEATURE_LEVEL levels[] = {
		D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0,
		D3D_FEATURE_LEVEL_9_3, D3D_FEATURE_LEVEL_9_1,
	};
	IDXGIAdapter1 *adapter;
	ID3D11Device *dev = NULL;
	ID3D11DeviceContext *ctx = NULL;
	D3D_FEATURE_LEVEL got = 0;
	HRESULT hr;

	SetEnvironmentVariableW(L"RUXNUMD_LOG", log);
	adapter = our_adapter();
	put_hex("our adapter found", adapter != NULL);
	if (!adapter)
		return 1;
	hr = D3D11CreateDevice((IDXGIAdapter *)adapter, D3D_DRIVER_TYPE_UNKNOWN, NULL, 0, levels,
			       ARRAYSIZE(levels), D3D11_SDK_VERSION, &dev, &got, &ctx);
	put_hex("D3D11CreateDevice", (ULONG)hr);
	put_hex("feature level", got);
	if (ctx)
		ID3D11DeviceContext_Release(ctx);
	if (dev)
		ID3D11Device_Release(dev);
	IDXGIAdapter1_Release(adapter);
	return SUCCEEDED(hr) ? 0 : 1;
}

static BOOL same(const WCHAR *a, const WCHAR *b)
{
	while (*a && *a == *b)
		a++, b++;
	return *a == *b;
}

static int run(void)
{
	int argc;
	WCHAR **argv = CommandLineToArgvW(GetCommandLineW(), &argc);

	if (argv && argc == 3 && same(argv[1], L"load"))
		return load(argv[2]);
	if (argv && argc == 3 && same(argv[1], L"device"))
		return device(argv[2]);
	put_str("usage: umdtest load PATH | device LOG\r\n");
	return 2;
}

void umdtest_entry(void)
{
	int status = run();

	flush();
	ExitProcess((UINT)status);
}
