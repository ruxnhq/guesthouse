// SPDX-License-Identifier: MIT
//
// winemetal.dll over ruxnpvg: the library DXMT's Direct3D side calls for
// Metal (winemetal.h), answered with ruxnpvg's task controls and pvg_wire's
// bytes, as pvgtri draws, instead of Wine's calls into macOS.
//
// This is the first slice, for one triangle: one device, buffers and 2D
// textures over buffer objects, the two shaders of the triangle, render
// pipelines, depth and stencil states, samplers, render and blit encoders,
// and a command buffer submitted and waited for at commit. translate.c
// writes the encoders' bytes. Every other function of winemetal.h is a stub
// (stubs.c, made by gen_stubs.py) that logs its name and fails.
//
// Shaders: DXMT's airconv, built for Windows on arm64 and linked in, turns
// DXBC into libraries for the device inside the guest (wm_shader.c, which
// also keeps the first slice's stand-in for the triangle, on request). The
// task controls go to ruxnwddm, the WDDM driver, by escapes (pvg_kmt.c).
//
// Every function logs its name the first time it is called, to
// winemetal-pvg.log beside the DLL, so a run shows what DXMT really calls.


#include "wm_internal.h"

CRITICAL_SECTION lock;
ULONG64 max_objects, max_resources, max_buffers;	// the driver's limits
static uint32_t next_number = 1;

// ---------------------------------------------------------------------
// The log.

static HANDLE log_file = INVALID_HANDLE_VALUE;
static bool to_stderr = true;	// the test programs' own console
// Inside the user-mode driver with no log file (a sandboxed process, such as
// Edge's GPU process, may not write C:\ruxnlog): each line goes to the
// kernel driver, which keeps it while Parameters\SandboxLog is 1 (klog.c).
static bool to_kernel;

// Failures said on this thread since the user-mode driver last asked
// (RuxnTakeFailures): a create in DXMT that winemetal could not make (out
// of object numbers or memory, a refused placement) must fail to the
// program, not hand it a resource with nothing behind it (run rn80: past
// 1,024 objects the program got a target that drew nowhere).
static _Thread_local unsigned failures;

__declspec(dllexport) unsigned WINAPI RuxnTakeFailures(void)
{
	unsigned n = failures;

	failures = 0;
	return n;
}

// Work lost for a reason of winemetal's own (no handle, no number, no
// memory, a command buffer it could not build): a FAILED line of those
// kinds (lost_work_lines) said while no create is under way
// (RuxnCreating), so no program call can return it. The user-mode driver asks RuxnWorkLost at each present, flush and
// map and then reports the device removed, so the program stops or makes
// its device again instead of showing pictures that were never drawn (run
// rn83: the handle table full, frames drew nothing, a wrong colour read
// back). A submit the device refused (STATUS_DEVICE_DATA_ERROR) is not
// counted here: it has its own handling.
static _Thread_local bool creating;
static volatile LONG work_lost;

// The FAILED lines that mean work is lost: a table ran out, or a command
// buffer could not be built or sent. Not an answer such as a view format
// the device does not take (run rn87: Edge's view of format 860420, made
// by DXMT at a draw, was counted, and the device it reported removed left
// Edge's WebGL white).
static const char *const lost_work_lines[] = {
	"out of object", "no buffer object", "control bo_create", "control object_create",
	"commandBuffer", "encoder", "command buffer", "segments", "too many events", "too big",
	"commit with",
};

static bool means_lost_work(const char *line)
{
	unsigned i;

	for (i = 0; i < sizeof(lost_work_lines) / sizeof(lost_work_lines[0]); i++) {
		if (strstr(line, lost_work_lines[i]))
			return true;
	}
	return false;
}

__declspec(dllexport) void WINAPI RuxnCreating(BOOL on)
{
	creating = on != FALSE;
}

__declspec(dllexport) unsigned WINAPI RuxnWorkLost(void)
{
	return (unsigned)work_lost;
}

void pvg_log(const char *format, ...)
{
	char line[512];
	va_list args;
	DWORD written;
	int n;

	bool lost = false;

	va_start(args, format);
	n = vsnprintf(line, sizeof(line) - 2, format, args);
	va_end(args);
	if (n < 0)
		return;
	if (!strncmp(format, "FAILED", 6)) {
		failures++;
		lost = !creating && means_lost_work(line) && !strstr(line, "status 0xc000009c") &&
		       InterlockedIncrement(&work_lost) == 1;
	}
	if (n > (int)sizeof(line) - 3)
		n = sizeof(line) - 3;
	line[n++] = '\r';
	line[n++] = '\n';
	if (log_file != INVALID_HANDLE_VALUE)
		WriteFile(log_file, line, (DWORD)n, &written, NULL);
	if (to_stderr)
		WriteFile(GetStdHandle(STD_ERROR_HANDLE), line, (DWORD)n, &written, NULL);
	if (to_kernel && log_file == INVALID_HANDLE_VALUE && !pvg_kmt_log(line, (unsigned)n - 2))
		to_kernel = false;
	if (lost)
		pvg_log("work lost: the first in this process (the line above); the device is reported removed");
}

bool wm_log_in_kernel(void)
{
	return log_file == INVALID_HANDLE_VALUE && to_kernel;
}

bool wm_lab_log_on(void)
{
	return log_file != INVALID_HANDLE_VALUE || to_kernel;
}

// Beside the DLL, or, when it is loaded from the driver store (inside
// ruxnwddm's user-mode driver, where nothing can be written beside it), in
// the lab's folder C:\ruxnlog as wm-PID.log, if that folder exists.
static void open_log(HINSTANCE self)
{
	WCHAR path[MAX_PATH];
	DWORD n = GetModuleFileNameW(self, path, MAX_PATH - 20);
	WCHAR *slash;

	if (!n || n >= MAX_PATH - 20)
		return;
	// Inside the user-mode driver the program's console is not ours to
	// write to (run rn16: spin's window filled with our lines).
	if (wcsstr(path, L"DriverStore")) {
		to_stderr = false;
		to_kernel = true;
		if (GetFileAttributesW(L"C:\\ruxnlog") == INVALID_FILE_ATTRIBUTES)
			return;
		swprintf(path, MAX_PATH, L"C:\\ruxnlog\\wm-%lu.log", GetCurrentProcessId());
		// Written through, so that a guest ended at once after a GPU
		// fault still has the lines on its disk (run rn79 lost them).
		log_file = CreateFileW(path, GENERIC_WRITE, FILE_SHARE_READ, NULL, CREATE_ALWAYS,
				       FILE_ATTRIBUTE_NORMAL | FILE_FLAG_WRITE_THROUGH, NULL);
		return;
	}
	slash = wcsrchr(path, L'\\');
	if (!slash)
		return;
	wcscpy(slash + 1, L"winemetal-pvg.log");
	log_file = CreateFileW(path, GENERIC_WRITE, FILE_SHARE_READ, NULL, CREATE_ALWAYS,
			       FILE_ATTRIBUTE_NORMAL, NULL);
}

// Notes (dropped state and the like) are logged once each.
static const char *noted[64];
static unsigned noted_count;

void note_once(const char *what)
{
	unsigned i;

	for (i = 0; i < noted_count; i++)
		if (noted[i] == what || !strcmp(noted[i], what))
			return;
	if (noted_count < 64)
		noted[noted_count++] = what;
	pvg_log("dropped: %s", what);
}


// Whether the process is ending: ntdll says so from the moment ExitProcess
// starts letting DLLs go, before DXMT's own (loaded after this one, so let
// go of first) releases what it holds.
bool wm_process_ending(void)
{
	static BOOLEAN(NTAPI *shutting)(void);

	if (!shutting)
		shutting = (BOOLEAN(NTAPI *)(void))(void *)GetProcAddress(GetModuleHandleW(L"ntdll.dll"),
									  "RtlDllShutdownInProgress");
	return shutting && shutting();
}

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved)
{
	(void)reserved;
	if (reason == DLL_PROCESS_ATTACH) {
		DisableThreadLibraryCalls(instance);
		InitializeCriticalSection(&lock);
		open_log(instance);
		pvg_log("winemetal over ruxnpvg: loaded");
	} else if (reason == DLL_PROCESS_DETACH) {
		wm_memory_report();
		pvg_log("winemetal over ruxnpvg: unloaded");
	}
	return TRUE;
}

// ---------------------------------------------------------------------
// The device and its memory.

// The kernel's answer for a handle whose task went with a power-down
// (STATUS_DEVICE_NOT_READY): the device this process drew on is gone, and
// nothing it draws reaches the screen again (run rn23b: after a resume from
// fast startup). Said once, plainly, in the log.
#define TASK_GONE ((LONG)0xC00000A3L)
#define GRAPHICS_KERNEL_REFUSED ((LONG)0xC0000001L)	// STATUS_UNSUCCESSFUL from D3DKMTEscape
static volatile LONG device_lost, kernel_gone;

BOOL wm_control(const char *what, DWORD code, const void *in, DWORD in_size, void *out, DWORD out_size)
{
	LONG status;

	if (kernel_gone)
		return FALSE;
	if (!pvg_kmt_control(code, in, in_size, out, out_size, &status)) {
		// At exit nothing more reaches the driver, and nothing needs to.
		if (wm_process_ending())
			return FALSE;
		// An escape the graphics kernel itself turned away (the driver
		// sets its own status in the escape): the process's kernel
		// objects are being taken down, as when a program is ended from
		// outside (runs rn55, rn56: every later control failed the same
		// way, hundreds of lines). Said once; nothing more is sent.
		if (status == GRAPHICS_KERNEL_REFUSED) {
			kernel_gone = 1;
			pvg_log("the graphics kernel takes no more escapes from this process (%s): it is being ended",
				what);
			return FALSE;
		}
		if (status == TASK_GONE && !InterlockedExchange(&device_lost, 1))
			pvg_log("the task is gone (a power-down): the device is lost");
		pvg_log("FAILED: control %s: status 0x%08lx", what, (unsigned long)status);
		return FALSE;
	}
	return TRUE;
}

static ULONG64 param(ULONG which)
{
	struct ruxnpvg_get_param p;

	memset(&p, 0, sizeof(p));
	p.param = which;
	if (!wm_control("get_param", RUXNPVG_IOCTL_GET_PARAM, &p, sizeof(p), &p, sizeof(p)))
		return 0;
	return p.value;
}

static void log_line(const char *line)
{
	pvg_log("%s", line);
}

// The task is this process's, through a kernel device on ruxnwddm's
// adapter (pvg_kmt.c).
bool wm_open_device(void)
{
	static bool opened;

	if (opened)
		return true;
	if (!pvg_kmt_open(log_line))
		return false;
	opened = true;
	pvg_log("ruxnwddm: interface version %llu, protocol %llu", param(RUXNPVG_PARAM_UAPI_VERSION),
		param(RUXNPVG_PARAM_PROTOCOL_VERSION));
	max_objects = param(RUXNPVG_PARAM_MAX_OBJECTS);
	max_resources = param(RUXNPVG_PARAM_MAX_RESOURCES);
	max_buffers = param(RUXNPVG_PARAM_MAX_BUFFERS);
	pvg_log("ruxnwddm: at most %llu objects, %llu resources and %llu command buffers a submit", max_objects,
		max_resources, max_buffers);
	return true;
}


// An object in the task, as pvgtri makes them: the driver writes the
// description's head from the buffer object, offset and size, and the
// tail follows.
bool create_object(uint32_t number, ULONG kind, const struct memory *m, ULONG flags, const void *tail,
			  ULONG tail_size)
{
	static UCHAR in[sizeof(struct ruxnpvg_object_create) + PVG_WIRE_MOST_DESCRIPTION];
	struct ruxnpvg_object_create *create = (void *)in;

	if (tail_size > PVG_WIRE_MOST_DESCRIPTION)
		return false;
	memset(create, 0, sizeof(*create));
	create->id = number;
	create->kind = kind;
	create->flags = flags;
	if (m) {
		create->handle = m->chunk->handle;
		create->offset = m->offset;
		create->size = m->length;
	}
	create->data_size = tail_size;
	if (tail_size)
		memcpy(in + sizeof(*create), tail, tail_size);
	if (!wm_control("object_create", RUXNPVG_IOCTL_OBJECT_CREATE, in, sizeof(*create) + tail_size, NULL, 0)) {
		pvg_log("FAILED: that was object %u of kind %lu", number, kind);
		return false;
	}
	return true;
}

// An object that names no memory: a texture over a surface (kind 11), whose
// handle is the surface's as the driver's OBJECT_CREATE takes it, or a view
// of one (kind 8), with no handle.
bool wm_create_bare_object(uint32_t number, ULONG kind, ULONG handle, const void *tail, ULONG tail_size)
{
	static UCHAR in[sizeof(struct ruxnpvg_object_create) + PVG_WIRE_MOST_DESCRIPTION];
	struct ruxnpvg_object_create *create = (void *)in;

	if (tail_size > PVG_WIRE_MOST_DESCRIPTION)
		return false;
	memset(create, 0, sizeof(*create));
	create->id = number;
	create->kind = kind;
	create->handle = handle;
	create->data_size = tail_size;
	memcpy(in + sizeof(*create), tail, tail_size);
	if (!wm_control("object_create", RUXNPVG_IOCTL_OBJECT_CREATE, in, sizeof(*create) + tail_size, NULL, 0)) {
		pvg_log("FAILED: that was object %u of kind %lu over nothing of its own", number, kind);
		return false;
	}
	return true;
}

// Every 2,000 submits, how much of the process is alive: its task's object
// numbers in use, winemetal's handles, the handles that went, its memory
// (runs rn75, rn76: Edge's GPU process gave nothing back and faulted the
// GPU with 1,014 of 1,024 numbers taken). It shows whether a process's
// objects stay flat.
void wm_say_live_now_and_then(void)
{
	static unsigned submits;

	if (++submits % 2000)
		return;
	pvg_log("alive after %u submits: %u task object numbers, %u handles in use, %llu handles gone, "
		"%llu bytes of chunks", submits, next_number - 1 - free_number_count,
		object_count - 1 - wm_free_handle_count(), (unsigned long long)objects_destroyed,
		(unsigned long long)wm_allocated_bytes());
	wm_memory_say_live();
}

// Object numbers given back by destroyed objects, used again first.
uint32_t free_numbers[TASK_NUMBERS_KEPT];
unsigned free_number_count;

uint32_t wm_take_number(void)
{
	uint32_t number;

	if (free_number_count) {
		number = free_numbers[--free_number_count];
		wm_broken_clear(number);
		return number;
	}
	if (max_objects && next_number >= max_objects) {
		pvg_log("FAILED: out of object numbers (%llu)", max_objects);
		return 0;
	}
	// How near the task's table is to full, said when it passes these
	// marks (runs rn75, rn76: Edge's GPU process faulted the GPU with its
	// numbers at about 1,014 of 1,024 and none given back).
	if (max_objects && (next_number == max_objects / 2 || next_number == max_objects - max_objects / 8 ||
			    next_number == max_objects - 24 || next_number == max_objects - 4))
		pvg_log("object numbers: %u taken, %u given back and free", next_number, free_number_count);
	return next_number++;
}

// ---------------------------------------------------------------------
// The device.

static obj_handle_t the_device, the_array;

obj_handle_t WMTCopyAllDevices(void)
{
	CALLED();
	EnterCriticalSection(&lock);
	if (!the_array && wm_open_device()) {
		the_device = new_object(K_DEVICE);
		the_array = new_object(K_ARRAY);
		objects[the_array].u.array.first = the_device;
		objects[the_array].u.array.count = 1;
	}
	LeaveCriticalSection(&lock);
	return the_array;
}

uint64_t MTLDevice_recommendedMaxWorkingSetSize(obj_handle_t dev)
{
	CALLED();
	(void)dev;
	return 1ull << 30;
}

uint64_t MTLDevice_currentAllocatedSize(obj_handle_t dev)
{
	CALLED();
	(void)dev;
	return wm_allocated_bytes();
}

obj_handle_t MTLDevice_name(obj_handle_t dev)
{
	obj_handle_t h;

	CALLED();
	(void)dev;
	EnterCriticalSection(&lock);
	h = new_string("ruxnpvg paravirtual GPU");
	LeaveCriticalSection(&lock);
	return h;
}

uint64_t MTLDevice_registryID(obj_handle_t dev)
{
	CALLED();
	(void)dev;
	return 1;
}

bool MTLDevice_hasUnifiedMemory(obj_handle_t dev)
{
	CALLED();
	(void)dev;
	return true;
}

// The host is an M4: GPU family 9, Metal 3.
bool MTLDevice_supportsFamily(obj_handle_t dev, enum WMTGPUFamily family)
{
	CALLED();
	(void)dev;
	return (family >= WMTGPUFamilyApple1 && family <= WMTGPUFamilyApple9) || family == WMTGPUFamilyMac2 ||
	       (family >= WMTGPUFamilyCommon1 && family <= WMTGPUFamilyCommon3) || family == WMTGPUFamilyMetal3;
}

// Every BC format was drawn through the helper as Metal on the Mac draws the
// same bytes (harness/kinds.sh); levels smaller than a block are left out.
bool MTLDevice_supportsBCTextureCompression(obj_handle_t dev)
{
	CALLED();
	(void)dev;
	return true;
}

bool MTLDevice_supportsTextureSampleCount(obj_handle_t dev, uint8_t count)
{
	CALLED();
	(void)dev;
	return count == 1;
}

uint64_t MTLDevice_minimumLinearTextureAlignmentForPixelFormat(obj_handle_t dev, enum WMTPixelFormat format)
{
	CALLED();
	(void)dev;
	(void)format;
	return 16;
}

void MTLDevice_setShouldMaximizeConcurrentCompilation(obj_handle_t dev, bool value)
{
	CALLED();
	(void)dev;
	(void)value;
}

// The host's macOS, 27.2.1.
void WMTGetOSVersion(uint64_t *major, uint64_t *minor, uint64_t *patch)
{
	CALLED();
	if (major)
		*major = 27;
	if (minor)
		*minor = 2;
	if (patch)
		*patch = 1;
}

bool WMTSetMetalShaderCachePath(const char *path)
{
	CALLED();
	(void)path;
	return false;
}
