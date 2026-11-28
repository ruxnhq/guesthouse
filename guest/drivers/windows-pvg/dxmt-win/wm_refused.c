// SPDX-License-Identifier: MIT
//
// A command buffer the device refused, kept for the Mac to read.
//
// The device says only that it refused a submit, not which command (run
// rn8: dwm's eleven-segment command buffer, 0xC000009C, and nothing in the
// host's log). So the first few refused submits of a process are written to
// C:\ruxnlog\refused-PID-N.bin, when that folder exists, and refused.py on
// the Mac lists their segments and commands. With RUXN_KEEP_SUBMIT=N in a
// process's environment its Nth submitted command buffer is kept the same
// way, as submit-PID-N.bin (wm_commands.c), to read what a program sent.
//
// The file: "RXRF", the resource count, each resource's number and flags,
// the segment count, then each segment's length and bytes; all numbers 32
// bits, little-endian. Only our own wire bytes are in it.

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

#include "winemetal.h"
#include "winemetal_pvg.h"

#define MOST_DUMPS 4
// Bytes of a kept command buffer on one log line, as hex.
#define LINE_BYTES 160

static unsigned dumps;

// The file's bytes, gathered first so that they can go to a file or, in a
// process that may not write one (Edge's sandboxed GPU process), to the
// log as "keptbin NAME OFFSET HEX" lines, which refused.py turns back into
// the file.
struct gather {
	UCHAR *bytes;
	size_t length, room;
	bool ok;
};

static void put(struct gather *g, const void *bytes, size_t length)
{
	if (!g->ok || g->length + length > g->room) {
		g->ok = false;
		return;
	}
	memcpy(g->bytes + g->length, bytes, length);
	g->length += length;
}

static void put32(struct gather *g, uint32_t value)
{
	put(g, &value, sizeof(value));
}

static void to_log(const char *name, const struct gather *g)
{
	static const char digits[] = "0123456789abcdef";
	char hex[2 * LINE_BYTES + 1];
	size_t at, i, n;

	for (at = 0; at < g->length; at += LINE_BYTES) {
		n = g->length - at < LINE_BYTES ? g->length - at : LINE_BYTES;
		for (i = 0; i < n; i++) {
			hex[2 * i] = digits[g->bytes[at + i] >> 4];
			hex[2 * i + 1] = digits[g->bytes[at + i] & 15];
		}
		hex[2 * n] = 0;
		pvg_log("keptbin %s %u %s", name, (unsigned)at, hex);
	}
}

// The file's bytes for one command buffer; g.ok false if they could not be had.
static struct gather gathered(const uint32_t *ids, const uint32_t *flags, unsigned resources,
			      const UCHAR *const *segments, const uint32_t *sizes, unsigned count)
{
	struct gather g;
	unsigned i;

	g.room = 12 + 8 * (size_t)resources;
	for (i = 0; i < count; i++)
		g.room += 4 + sizes[i];
	g.bytes = malloc(g.room);
	g.length = 0;
	g.ok = g.bytes != NULL;
	put(&g, "RXRF", 4);
	put32(&g, resources);
	for (i = 0; i < resources; i++) {
		put32(&g, ids[i]);
		put32(&g, flags[i]);
	}
	put32(&g, count);
	for (i = 0; i < count; i++) {
		put32(&g, sizes[i]);
		put(&g, segments[i], sizes[i]);
	}
	return g;
}

// To C:\ruxnlog\NAME, or to the log as hex lines where no file can be written.
static bool keep(const char *name, struct gather *g)
{
	WCHAR path[MAX_PATH];
	HANDLE file = INVALID_HANDLE_VALUE;
	DWORD written;

	if (!g->ok)
		return false;
	if (GetFileAttributesW(L"C:\\ruxnlog") != INVALID_FILE_ATTRIBUTES) {
		swprintf(path, MAX_PATH, L"C:\\ruxnlog\\%hs", name);
		file = CreateFileW(path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
	}
	if (file != INVALID_HANDLE_VALUE) {
		g->ok = WriteFile(file, g->bytes, (DWORD)g->length, &written, NULL) && written == g->length;
		CloseHandle(file);
	} else {
		to_log(name, g);
	}
	return g->ok;
}

// The last RECENT submits that went through, kept in memory while the lab
// log is on, and written out at the first refusal: a GPU fault is the
// device refusing a command buffer it had taken, and what came just
// before it may be what set it up (run rn75).
#define RECENT 6
static struct gather recent[RECENT];
static unsigned recent_next, recent_count;

void wm_recent_keep(const uint32_t *ids, const uint32_t *flags, unsigned resources,
		    const UCHAR *const *segments, const uint32_t *sizes, unsigned count)
{
	struct gather *slot = &recent[recent_next];

	if (!wm_lab_log_on() || dumps == MOST_DUMPS)
		return;
	free(slot->bytes);
	*slot = gathered(ids, flags, resources, segments, sizes, count);
	recent_next = (recent_next + 1) % RECENT;
	if (recent_count < RECENT)
		recent_count++;
}

static void recent_dump(void)
{
	char name[64];
	unsigned i, at;

	for (i = 0; i < recent_count; i++) {
		at = (recent_next + RECENT - recent_count + i) % RECENT;
		snprintf(name, sizeof(name), "recent-%lu-%u-%u.bin", GetCurrentProcessId(), dumps, i);
		pvg_log("%s: a command buffer that went through before the refusal, as %s",
			keep(name, &recent[at]) ? "kept" : "FAILED to keep", name);
		free(recent[at].bytes);
		recent[at].bytes = NULL;
	}
	recent_count = 0;
}

void wm_refused_dump(const char *what, const uint32_t *ids, const uint32_t *flags, unsigned resources,
		     const UCHAR *const *segments, const uint32_t *sizes, unsigned count)
{
	struct gather g;
	char name[64];
	bool ok;

	if (dumps == MOST_DUMPS)
		return;
	if (!strcmp(what, "refused"))
		recent_dump();
	g = gathered(ids, flags, resources, segments, sizes, count);
	snprintf(name, sizeof(name), "%s-%lu-%u.bin", what, GetCurrentProcessId(), dumps++);
	ok = keep(name, &g);
	free(g.bytes);
	pvg_log("%s: a command buffer, as %s", ok ? "kept" : "FAILED to keep", name);
}
