// SPDX-License-Identifier: GPL-2.0
//
// wddmtool: talks to ruxnwddm through the graphics kernel's escapes.
//
//   wddmtool adapters   lists every WDDM adapter with its PCI ids and
//                       number of sources, so a run can see which driver
//                       owns which display
//   wddmtool info       prints ruxnwddm's counters (no device access)
//   wddmtool ring N     turns the device's first ring on and sends N
//                       command 30s, each waited for; administrators only
//   wddmtool pvg        prints the counters of the tasks, the surfaces, the
//                       display and the flips (no device access)
//   wddmtool allocs     lists the live allocations, made and opened, with the
//                       process of each and whether it still runs;
//                       administrators only
//   wddmtool klog FILE  writes the lines that processes which cannot write a
//                       log sent to the driver (Parameters\SandboxLog 1);
//                       administrators only
//   wddmtool klogfollow FILE  the same, and goes on adding what comes, five
//                       times a second, each write through to the disk, so
//                       that a guest ended at once (a GPU fault) keeps it
//
// Built without a C runtime, as pvgtest is: its own entry point, kernel32
// and gdi32 only. Exit status 0 when all went well, 1 when not, 2 for a
// usage error.

#include <windows.h>

typedef LONG NTSTATUS;
#include <d3dkmthk.h>

#include "ruxnwddm_escape.h"

#define MOST_ADAPTERS	16
#define MOST_NOPS	100000

static char out_buf[1024];
static unsigned int out_len;

static void flush(void)
{
	DWORD written;

	if (out_len)
		WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), out_buf, out_len, &written, NULL);
	out_len = 0;
}

static void put_char(char c)
{
	if (out_len == sizeof(out_buf))
		flush();
	out_buf[out_len++] = c;
}

static void put_str(const char *s)
{
	while (*s)
		put_char(*s++);
}

static void put_dec(ULONG64 value)
{
	char digits[20];
	int n = 0;

	do {
		digits[n++] = (char)('0' + value % 10);
		value /= 10;
	} while (value);
	while (n)
		put_char(digits[--n]);
}

static void put_hex(ULONG64 value, int digits)
{
	static const char hex[] = "0123456789abcdef";
	int shift;

	put_str("0x");
	for (shift = (digits - 1) * 4; shift >= 0; shift -= 4)
		put_char(hex[(value >> shift) & 0xf]);
}

static void put_line(const char *name, ULONG64 value)
{
	put_str(name);
	put_char(' ');
	put_dec(value);
	put_str("\r\n");
}

static void put_line_hex(const char *name, ULONG64 value)
{
	put_str(name);
	put_char(' ');
	put_hex(value, value >> 32 ? 16 : 8);
	put_str("\r\n");
}

static void *zero(void *p, SIZE_T n)
{
	volatile UCHAR *b = p;

	while (n--)
		*b++ = 0;
	return p;
}

// The vendor and device ids of an adapter, or FALSE if Windows will not say.
static BOOL adapter_ids(D3DKMT_HANDLE adapter, UINT *vendor, UINT *device)
{
	D3DKMT_QUERY_DEVICE_IDS ids;
	D3DKMT_QUERYADAPTERINFO query;

	zero(&ids, sizeof(ids));
	zero(&query, sizeof(query));
	query.hAdapter = adapter;
	query.Type = KMTQAITYPE_PHYSICALADAPTERDEVICEIDS;
	query.pPrivateDriverData = &ids;
	query.PrivateDriverDataSize = sizeof(ids);
	if (D3DKMTQueryAdapterInfo(&query) < 0)
		return FALSE;
	*vendor = ids.DeviceIds.VendorID;
	*device = ids.DeviceIds.DeviceID;
	return TRUE;
}

static D3DKMT_ADAPTERINFO adapters[MOST_ADAPTERS];

// Opens every adapter; the caller closes them.
static ULONG enum_adapters(void)
{
	D3DKMT_ENUMADAPTERS2 en;

	zero(&en, sizeof(en));
	en.NumAdapters = MOST_ADAPTERS;
	en.pAdapters = adapters;
	if (D3DKMTEnumAdapters2(&en) < 0)
		return 0;
	return en.NumAdapters;
}

static void close_adapters(ULONG count)
{
	ULONG i;

	for (i = 0; i < count; i++) {
		D3DKMT_CLOSEADAPTER close = { adapters[i].hAdapter };

		D3DKMTCloseAdapter(&close);
	}
}

static int list_adapters(void)
{
	ULONG count = enum_adapters(), i;

	put_line("adapters", count);
	for (i = 0; i < count; i++) {
		UINT vendor = 0, device = 0;

		put_str("adapter ");
		put_dec(i);
		put_str(" luid ");
		put_hex(((ULONG64)(ULONG)adapters[i].AdapterLuid.HighPart << 32) |
			adapters[i].AdapterLuid.LowPart, 16);
		put_str(" sources ");
		put_dec(adapters[i].NumOfSources);
		if (adapter_ids(adapters[i].hAdapter, &vendor, &device)) {
			put_str(" pci ");
			put_hex(vendor, 4);
			put_char(':');
			put_hex(device, 4);
		}
		put_str("\r\n");
	}
	close_adapters(count);
	return count ? 0 : 1;
}

// Sends one escape to our adapter. A display-only adapter is reported with
// the ids of the render adapter Windows pairs it with (Microsoft's basic
// render driver), so ours is found by asking: every adapter with a display
// source gets the escape, and the one that answers with our magic is ours.
// Others refuse a private escape they do not know. Returns the escape's own
// status, or a negative value if no adapter answered.
static LONG escape(void *data, UINT size)
{
	struct ruxnwddm_escape_header *h = data;
	ULONG count = enum_adapters(), i;
	LONG status = -1;

	for (i = 0; i < count; i++) {
		D3DKMT_ESCAPE esc;

		if (adapters[i].NumOfSources == 0)
			continue;
		h->magic = RUXNWDDM_ESCAPE_MAGIC;
		zero(&esc, sizeof(esc));
		esc.hAdapter = adapters[i].hAdapter;
		esc.Type = D3DKMT_ESCAPE_DRIVERPRIVATE;
		esc.pPrivateDriverData = data;
		esc.PrivateDriverDataSize = size;
		if (D3DKMTEscape(&esc) < 0 || h->magic != RUXNWDDM_ESCAPE_MAGIC)
			continue;
		put_str("adapter ");
		put_dec(i);
		put_str(" answered\r\n");
		status = (LONG)h->status;
		break;
	}
	if (i == count)
		put_str("no adapter of ours\r\n");
	close_adapters(count);
	return status;
}

static void header(struct ruxnwddm_escape_header *h, unsigned int command)
{
	h->magic = RUXNWDDM_ESCAPE_MAGIC;
	h->version = RUXNWDDM_ESCAPE_VERSION;
	h->command = command;
	h->status = 0;
}

static int info(void)
{
	struct ruxnwddm_escape_info in;
	LONG status;

	zero(&in, sizeof(in));
	header(&in.header, RUXNWDDM_ESCAPE_INFO);
	status = escape(&in, sizeof(in));
	if (status != 0)
		return 1;
	put_line_hex("driver_version", in.driver_version);
	put_line("scanout", in.scanout);
	put_line_hex("post_status", in.post_status);
	put_line("post_width", in.post_width);
	put_line("post_height", in.post_height);
	put_line("post_pitch", in.post_pitch);
	put_line("post_format", in.post_format);
	put_line_hex("post_address", in.post_address);
	put_line("width", in.width);
	put_line("height", in.height);
	put_line("pitch", in.pitch);
	put_line_hex("efb_page", in.efb_page);
	put_line("efb_bytes", in.efb_bytes);
	put_line("efb_depth", in.efb_depth);
	put_line_hex("efb_mode", in.efb_mode);
	put_line_hex("bar0_length", in.bar0_length);
	put_line_hex("bar2_length", in.bar2_length);
	put_line("interrupt_messages", in.interrupt_messages);
	put_line("source_visible", in.source_visible);
	put_line("commits", in.commits);
	put_line("power_state", in.power_state);
	put_line("presents", in.presents);
	put_line("presents_skipped", in.presents_skipped);
	put_line("moves", in.moves);
	put_line("dirty_rects", in.dirty_rects);
	put_line("bytes_copied", in.bytes_copied);
	put_line("irq_taken", in.irq_taken);
	put_line("irq_done", in.irq_done);
	put_line("irq_events", in.irq_events);
	put_line("irq_faults", in.irq_faults);
	put_line("irq_empty", in.irq_empty);
	put_line("irq_other", in.irq_other);
	put_line("ring_on", in.ring_on);
	put_line("ring_version", in.ring_version);
	put_line("ring_sent", in.ring_sent);
	put_line("ring_late", in.ring_late);
	put_line("ring_refused", in.ring_refused);
	put_line("model", in.model);
	put_line("vsync_enabled", in.vsync_enabled);
	put_line("devices", in.devices);
	put_line("contexts", in.contexts);
	put_line("processes", in.processes);
	put_line("allocations", in.allocations);
	put_line("allocations_freed", in.allocations_freed);
	put_line("primaries", in.primaries);
	put_line("submits", in.submits);
	put_line("paging_submits", in.paging_submits);
	put_line("paging_ops", in.paging_ops);
	put_line("paging_skipped", in.paging_skipped);
	put_line("fences_written", in.fences_written);
	put_line("fences_lost", in.fences_lost);
	put_line("kernel_presents", in.kernel_presents);
	put_line("flips", in.flips);
	put_line("vsyncs", in.vsyncs);
	{
		static const char *names[RUXNWDDM_UMD_NOTES] = {
			"umd_note0", "umd_open_adapter", "umd_create_device", "umd_create_resource",
			"umd_present", "umd_draws", "umd_failed", "umd_note7",
		};
		UINT i;

		for (i = 1; i < RUXNWDDM_UMD_NOTES; i++) {
			put_str(names[i]);
			put_str(" count ");
			put_dec(in.umd_count[i]);
			put_str(" last ");
			put_hex(in.umd_value[i], 8);
			put_str(" process ");
			put_dec(in.umd_process[i]);
			put_str("\r\n");
		}
	}
	return 0;
}

#define PUT(s, field) put_line(#field, (s).field)

static int pvg(void)
{
	static struct ruxnwddm_escape_pvg_info in;
	LONG status;

	zero(&in, sizeof(in));
	header(&in.header, RUXNWDDM_ESCAPE_PVG_INFO);
	status = escape(&in, sizeof(in));
	if (status != 0)
		return 1;
	PUT(in.tasks, defined);
	PUT(in.tasks, deleted);
	PUT(in.tasks, lost);
	PUT(in.tasks, submits);
	PUT(in.tasks, refused);
	PUT(in.tasks, bos);
	PUT(in.tasks, bos_freed);
	PUT(in.tasks, bos_kept);
	PUT(in.tasks, unmapped);
	PUT(in.tasks, bo_bytes);
	PUT(in.tasks, open_files);
	PUT(in.display, mapper_started);
	PUT(in.display, mapper_commands);
	PUT(in.display, mapper_irqs);
	PUT(in.display, mapper_timeouts);
	PUT(in.display, maps);
	PUT(in.display, unmaps);
	PUT(in.display, outcomes_other);
	PUT(in.display, mapped_now);
	PUT(in.display, held_now);
	PUT(in.display, surfaces);
	PUT(in.display, surfaces_freed);
	PUT(in.display, surfaces_kept);
	PUT(in.display, takes);
	PUT(in.display, releases);
	PUT(in.display, owner_process);
	PUT(in.display, owner_mode);
	PUT(in.display, spoiled);
	PUT(in.display, task0_defined);
	PUT(in.display, hand_overs);
	PUT(in.display, mode_lists);
	PUT(in.display, acknowledgements);
	PUT(in.display, swaps);
	PUT(in.display, swap_done);
	PUT(in.display, display_failed);
	PUT(in.display, bank_offs);
	PUT(in.display, shown_id);
	put_line_hex("first_mode", in.display.first_mode);
	PUT(in.display, exports);
	PUT(in.display, imports);
	PUT(in.display, imports_refused);
	PUT(in.display, holders_now);
	PUT(in, flips_shown);
	PUT(in, flips_copied);
	PUT(in, flip_failures);
	PUT(in, surface_allocations);
	PUT(in, surface_imports_failed);
	PUT(in, present_flips);
	PUT(in, present_blts);
	PUT(in, present_to_primary);
	PUT(in, shown_surface_pid);
	PUT(in, efb_marked);
	PUT(in, high_res_vsync);
	PUT(in, power_downs);
	PUT(in, power_ups);
	PUT(in, mapper_power_downs);
	PUT(in, flip_failures_before_power_up);
	PUT(in, vsyncs);
	PUT(in, flips);
	PUT(in, superseded);
	PUT(in, swap_most_us);
	put_line("qpc_frequency", (unsigned long long)in.qpc_frequency);
	put_line("first_vsync", (unsigned long long)in.first_vsync);
	put_line("last_vsync", (unsigned long long)in.last_vsync);
	put_line("first_swap", (unsigned long long)in.first_swap);
	put_line("last_swap", (unsigned long long)in.last_swap);
	for (UINT i = 0; i < 4; i++) {
		put_str("vsync_gap_bucket ");
		put_dec(i);
		put_char(' ');
		put_dec(in.vsync_gaps[i]);
		put_str("\r\n");
	}
	for (UINT i = 0; i < 5; i++) {
		put_str("swap_time_bucket ");
		put_dec(i);
		put_char(' ');
		put_dec(in.swap_times[i]);
		put_str("\r\n");
	}
	for (UINT i = 0; i < RUXNWDDM_SUBMIT_PROCESSES && in.submit_pid[i]; i++) {
		put_str("submits of process ");
		put_dec(in.submit_pid[i]);
		put_str(" (");
		in.submit_image[i][RUXNWDDM_IMAGE_NAME - 1] = 0;
		put_str(in.submit_image[i]);
		put_str(")");
		put_str(": through ");
		put_dec(in.submit_done[i]);
		put_str(" refused ");
		put_dec(in.submit_refused[i]);
		put_str("\r\n");
	}
	return 0;
}

static int ring(ULONG count)
{
	struct ruxnwddm_escape_header start;
	struct ruxnwddm_escape_ring nops;
	LONG status;

	zero(&start, sizeof(start));
	header(&start, RUXNWDDM_ESCAPE_RING_START);
	status = escape(&start, sizeof(start));
	put_line_hex("ring_start", (ULONG)status);
	if (status != 0)
		return 1;
	zero(&nops, sizeof(nops));
	header(&nops.header, RUXNWDDM_ESCAPE_RING_NOP);
	nops.count = count;
	status = escape(&nops, sizeof(nops));
	put_line_hex("ring_nop", (ULONG)status);
	put_line("asked", count);
	put_line("sent", nops.sent);
	put_line("late", nops.late);
	put_line_hex("first_error", nops.first_error);
	put_line("microseconds", nops.microseconds);
	return status == 0 && nops.sent == count ? 0 : 1;
}

static BOOL same(const WCHAR *a, const WCHAR *b)
{
	while (*a && *a == *b)
		a++, b++;
	return *a == *b;
}

static BOOL parse_count(const WCHAR *s, ULONG *value)
{
	ULONG64 v = 0;

	if (!*s)
		return FALSE;
	for (; *s; s++) {
		if (*s < L'0' || *s > L'9')
			return FALSE;
		v = v * 10 + (ULONG)(*s - L'0');
		if (v > MOST_NOPS)
			return FALSE;
	}
	*value = (ULONG)v;
	return v != 0;
}

// The words after the program's name, split at spaces; no quoting needed.
static int split(WCHAR *line, WCHAR **words, int most)
{
	int n = 0;
	BOOL quoted = FALSE;

	// Skip the program's own name, which may be quoted.
	while (*line && (quoted || *line != L' ')) {
		if (*line == L'"')
			quoted = !quoted;
		line++;
	}
	while (n < most) {
		while (*line == L' ')
			line++;
		if (!*line)
			break;
		words[n++] = line;
		while (*line && *line != L' ')
			line++;
		if (*line)
			*line++ = 0;
	}
	return n;
}

// A process's image name, or "ended" when it no longer runs.
static void put_process(ULONG pid)
{
	HANDLE p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
	char name[MAX_PATH];
	DWORD length = sizeof(name), code = 0;
	const char *base = name;
	DWORD i;

	if (!p || !GetExitCodeProcess(p, &code) || code != STILL_ACTIVE ||
	    !QueryFullProcessImageNameA(p, 0, name, &length)) {
		put_str(p && code != STILL_ACTIVE ? "ended" : "unknown");
	} else {
		for (i = 0; i < length; i++)
			if (name[i] == '\\')
				base = name + i + 1;
		put_str(base);
	}
	if (p)
		CloseHandle(p);
}

static int allocs(void)
{
	static struct ruxnwddm_escape_allocations in;
	unsigned int i;

	zero(&in, sizeof(in));
	header(&in.header, RUXNWDDM_ESCAPE_ALLOCATIONS);
	if (escape(&in, sizeof(in)) != 0)
		return 1;
	put_line("live", in.live);
	put_line("returned", in.returned);
	for (i = 0; i < in.returned; i++) {
		const struct ruxnwddm_escape_allocation *e = &in.list[i];

		put_str(e->opened ? "opened by " : "made by ");
		put_dec(e->process);
		put_str(" ");
		put_process(e->process);
		put_str(" format ");
		put_dec(e->format);
		put_str(" ");
		put_dec(e->width);
		put_str("x");
		put_dec(e->height);
		put_str(" flags ");
		put_hex(e->flags, 2);
		put_str(" holders ");
		put_dec(e->surface_holders);
		put_str(" token ");
		put_hex(e->surface_token, 16);
		put_str("\r\n");
	}
	return 0;
}

// Writes the driver's kept log to path from its start; with follow, goes on
// adding what comes, written through to the disk, until it is stopped.
static int klog(const WCHAR *path, BOOL follow)
{
	static struct ruxnwddm_escape_log_read in;
	HANDLE file = CreateFileW(path, GENERIC_WRITE, FILE_SHARE_READ, NULL, CREATE_ALWAYS,
				  FILE_ATTRIBUTE_NORMAL | (follow ? FILE_FLAG_WRITE_THROUGH : 0), NULL);
	ULONG64 from = 0, bytes = 0;
	DWORD written;

	if (file == INVALID_HANDLE_VALUE) {
		put_str("cannot write the file\r\n");
		return 1;
	}
	for (;;) {
		zero(&in, sizeof(in));
		header(&in.header, RUXNWDDM_ESCAPE_LOG_READ);
		in.from = from;
		if (escape(&in, sizeof(in)) != 0) {
			CloseHandle(file);
			put_str("the driver keeps no such log (SandboxLog is not 1, or not an administrator)\r\n");
			return 1;
		}
		if (in.length) {
			WriteFile(file, in.text, in.length, &written, NULL);
			bytes += in.length;
			from = in.from + in.length;
			continue;
		}
		if (!follow)
			break;
		out_len = 0;	// escape()'s "answered" lines are not kept while following
		Sleep(200);
	}
	CloseHandle(file);
	put_line("bytes", bytes);
	put_line("written_ever", in.total);
	return 0;
}

static int run(void)
{
	WCHAR *words[3];
	int n = split(GetCommandLineW(), words, 3);
	ULONG count;

	if (n == 1 && same(words[0], L"adapters"))
		return list_adapters();
	if (n == 1 && same(words[0], L"info"))
		return info();
	if (n == 2 && same(words[0], L"ring") && parse_count(words[1], &count))
		return ring(count);
	if (n == 1 && same(words[0], L"pvg"))
		return pvg();
	if (n == 1 && same(words[0], L"allocs"))
		return allocs();
	if (n == 2 && same(words[0], L"klog"))
		return klog(words[1], FALSE);
	if (n == 2 && same(words[0], L"klogfollow"))
		return klog(words[1], TRUE);
	put_str("usage: wddmtool adapters | info | ring N | pvg | allocs | klog FILE | klogfollow FILE\r\n");
	return 2;
}

void wddmtool_entry(void)
{
	int status = run();

	flush();
	ExitProcess((UINT)status);
}
