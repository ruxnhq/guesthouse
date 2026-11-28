// SPDX-License-Identifier: MIT
//
// The way to the device's tasks under ruxnwddm, the WDDM driver: ruxnpvg's
// task controls (ruxnpvg_ioctl.h), unchanged, carried by D3DKMTEscape on a
// kernel device this library makes on our adapter, instead of by
// DeviceIoControl on \\.\ruxnpvg. The driver's side is pvg_client.c in
// ruxnwddm. The kernel device is the task's handle: the task, its buffer
// objects and its surfaces go when the device is destroyed, or when the
// process ends.
//
// Our adapter is the one that answers ruxnwddm's PVG_INFO escape. The few
// kernel-thunk structures needed are declared here from their published
// layouts, since llvm-mingw has no d3dkmthk.h; the functions are gdi32's.

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winioctl.h>
#include <stdlib.h>
#include <string.h>

#include "ruxnwddm_escape.h"
#include "pvg_kmt.h"

typedef LONG NTSTATUS;
typedef UINT D3DKMT_HANDLE;

#define MOST_ADAPTERS		16
#define ESCAPE_DRIVER_PRIVATE	0	// D3DKMT_ESCAPE_DRIVERPRIVATE
#define STATUS_NOT_SUPPORTED_CODE	0xC00000BBu	// the driver keeps no log

typedef struct {
	D3DKMT_HANDLE hAdapter;
	LUID AdapterLuid;
	ULONG NumOfSources;
	BOOL bPrecisePresentRegionsPreferred;
} KMT_ADAPTERINFO;

typedef struct {
	ULONG NumAdapters;
	KMT_ADAPTERINFO *pAdapters;
} KMT_ENUMADAPTERS2;

typedef struct {
	D3DKMT_HANDLE hAdapter;
} KMT_CLOSEADAPTER;

typedef struct {
	union {
		D3DKMT_HANDLE hAdapter;
		VOID *pAdapter;
	};
	UINT Flags;
	D3DKMT_HANDLE hDevice;			// out
	VOID *pCommandBuffer;
	UINT CommandBufferSize;
	VOID *pAllocationList;
	UINT AllocationListSize;
	VOID *pPatchLocationList;
	UINT PatchLocationListSize;
} KMT_CREATEDEVICE;

typedef struct {
	D3DKMT_HANDLE hAdapter;
	D3DKMT_HANDLE hDevice;
	UINT Type;
	UINT Flags;
	VOID *pPrivateDriverData;
	UINT PrivateDriverDataSize;
	D3DKMT_HANDLE hContext;
} KMT_ESCAPE;

typedef NTSTATUS(APIENTRY *ENUMADAPTERS2)(KMT_ENUMADAPTERS2 *);
typedef NTSTATUS(APIENTRY *CLOSEADAPTER)(const KMT_CLOSEADAPTER *);
typedef NTSTATUS(APIENTRY *CREATEDEVICE)(KMT_CREATEDEVICE *);
typedef NTSTATUS(APIENTRY *ESCAPE)(const KMT_ESCAPE *);

static ESCAPE escape_fn;
static D3DKMT_HANDLE adapter, device;

static void fill_header(struct ruxnwddm_escape_header *h, unsigned int command)
{
	h->magic = RUXNWDDM_ESCAPE_MAGIC;
	h->version = RUXNWDDM_ESCAPE_VERSION;
	h->command = command;
	h->status = 0;
}

// The last D3DKMTEscape that failed: its NTSTATUS, for the log (run rn53:
// hundreds of controls failed and the log could only say 0xc0000001).
static NTSTATUS last_escape_status;

static BOOL send_escape(D3DKMT_HANDLE on_adapter, D3DKMT_HANDLE on_device, void *data, UINT size)
{
	KMT_ESCAPE e;

	memset(&e, 0, sizeof(e));
	e.hAdapter = on_adapter;
	e.hDevice = on_device;
	e.Type = ESCAPE_DRIVER_PRIVATE;
	e.pPrivateDriverData = data;
	e.PrivateDriverDataSize = size;
	last_escape_status = escape_fn(&e);
	return last_escape_status >= 0;
}

// Whether ruxnwddm answers on this adapter.
static BOOL is_ours(D3DKMT_HANDLE candidate)
{
	struct ruxnwddm_escape_pvg_info info;

	memset(&info, 0, sizeof(info));
	fill_header(&info.header, RUXNWDDM_ESCAPE_PVG_INFO);
	return send_escape(candidate, 0, &info, sizeof(info)) && info.header.status == 0 &&
	       info.header.magic == RUXNWDDM_ESCAPE_MAGIC;
}

bool pvg_kmt_open(pvg_kmt_log_fn log)
{
	HMODULE gdi = LoadLibraryW(L"gdi32.dll");
	ENUMADAPTERS2 enum_fn;
	CLOSEADAPTER close_fn;
	CREATEDEVICE create_fn;
	KMT_ADAPTERINFO adapters[MOST_ADAPTERS];
	KMT_ENUMADAPTERS2 en;
	KMT_CREATEDEVICE create;
	ULONG i;

	if (device)
		return true;
	if (!gdi)
		return false;
	enum_fn = (ENUMADAPTERS2)(void *)GetProcAddress(gdi, "D3DKMTEnumAdapters2");
	close_fn = (CLOSEADAPTER)(void *)GetProcAddress(gdi, "D3DKMTCloseAdapter");
	create_fn = (CREATEDEVICE)(void *)GetProcAddress(gdi, "D3DKMTCreateDevice");
	escape_fn = (ESCAPE)(void *)GetProcAddress(gdi, "D3DKMTEscape");
	if (!enum_fn || !close_fn || !create_fn || !escape_fn) {
		log("FAILED: gdi32 has no kernel thunks");
		return false;
	}

	memset(&en, 0, sizeof(en));
	en.NumAdapters = MOST_ADAPTERS;
	en.pAdapters = adapters;
	if (enum_fn(&en) < 0) {
		log("FAILED: D3DKMTEnumAdapters2");
		return false;
	}
	for (i = 0; i < en.NumAdapters; i++) {
		if (!adapter && is_ours(adapters[i].hAdapter)) {
			adapter = adapters[i].hAdapter;
			continue;
		}
		KMT_CLOSEADAPTER close = { adapters[i].hAdapter };
		close_fn(&close);
	}
	if (!adapter) {
		log("FAILED: no adapter answers ruxnwddm's escapes");
		return false;
	}
	memset(&create, 0, sizeof(create));
	create.hAdapter = adapter;
	if (create_fn(&create) < 0 || !create.hDevice) {
		log("FAILED: D3DKMTCreateDevice on our adapter");
		return false;
	}
	device = create.hDevice;
	return true;
}

bool pvg_kmt_control(DWORD code, const void *in, DWORD in_size, void *out, DWORD out_size,
		     LONG *status)
{
	DWORD capacity = in_size > out_size ? in_size : out_size;
	struct ruxnwddm_escape_pvg *esc;
	UINT size = (UINT)(sizeof(*esc) + capacity);
	bool ok;

	*status = 0;
	esc = calloc(1, size);
	if (!esc)
		return false;
	fill_header(&esc->header, RUXNWDDM_ESCAPE_PVG);
	esc->code = code;
	esc->in_length = in_size;
	esc->capacity = capacity;
	if (in_size)
		memcpy(esc->data, in, in_size);
	ok = send_escape(adapter, device, esc, size);
	// The driver's own status, or the graphics kernel's if the escape
	// never reached the driver.
	*status = ok ? (LONG)esc->header.status : (LONG)last_escape_status;
	ok = ok && esc->header.status == 0;
	if (ok && out_size)
		memcpy(out, esc->data, esc->out_length < out_size ? esc->out_length : out_size);
	free(esc);
	return ok;
}

bool pvg_kmt_log(const char *text, unsigned length)
{
	struct ruxnwddm_escape_log line;

	// Lines before the adapter is found are dropped, and the sending goes on.
	if (!adapter)
		return true;
	memset(&line, 0, sizeof(line));
	fill_header(&line.header, RUXNWDDM_ESCAPE_LOG);
	if (length > sizeof(line.text))
		length = sizeof(line.text);
	memcpy(line.text, text, length);
	line.length = length;
	if (!send_escape(adapter, 0, &line, sizeof(line)))
		return false;
	return line.header.status != (unsigned int)STATUS_NOT_SUPPORTED_CODE;
}

bool pvg_kmt_info(struct ruxnwddm_escape_pvg_info *info)
{
	memset(info, 0, sizeof(*info));
	fill_header(&info->header, RUXNWDDM_ESCAPE_PVG_INFO);
	return adapter && send_escape(adapter, 0, info, sizeof(*info)) && info->header.status == 0;
}
