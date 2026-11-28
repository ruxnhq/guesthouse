// SPDX-License-Identifier: MIT
//
// winemetal's memory: buffer objects of the driver ("chunks"), each one
// kind 1 object on the device over all of it, cut into page-rounded ranges
// for buffers, textures and command buffers.
//
// A range given back (wm_give_back: a buffer or texture whose last
// reference went) is kept in its chunk's list of free ranges and handed
// out again, first fit, so a program that makes and drops resources does
// not grow (run rn38: dwm's chunks grew by about 2.2 MB each time a
// program opened and closed, when nothing was given back). Chunks are
// kept until the task ends.
//
// An object bigger than CHUNK_BYTES gets a chunk of its own, a whole
// chunk, and nothing else is ever cut from it: when the object goes, the
// chunk waits whole for the next big object it can hold. Up to 0.3.58 the
// smaller objects took the freed room of such a chunk, so the next big
// object found no room and made a new chunk: Edge's GPU process made 43
// chunks of 8 MiB that way and grew to 581 MB (run rn88).
//
// A chunk with nothing alive in it goes back to the driver (free_chunk:
// its object deleted, then BO_DESTROY, which has the device unmap it from
// the task), but for the last IDLE_KEPT of its sort, which wait for the
// next objects. Every command buffer is waited for at its commit, so the
// device no longer uses what was in it (run rn91: Edge's GPU process kept
// 400 MB of empty room after Chrome let its tiles go).
//
// A range handed out for the first time is zero (the driver zeroes a buffer
// object's pages); one handed out again is not, and says so (zeroed), so
// that a fresh buffer's zeroing is not dropped (translate_blit.c).
//
// Every command buffer is waited for at its commit, so a range given back
// is no longer read or written by the device.

#include "wm_internal.h"

// Free ranges a chunk keeps; more than this and the smallest new one is
// left unused, counted.
#define MOST_FREE_RANGES 256

struct range {
	ULONG64 offset, length;
};

struct chunk_free {
	struct range ranges[MOST_FREE_RANGES];	// by offset, none touching
	unsigned count;
};

static struct chunk chunks[MOST_CHUNKS];
static struct chunk_free chunk_free[MOST_CHUNKS];
static unsigned chunk_count;
static uint64_t allocated_bytes;
static uint64_t given_back_bytes, reused_bytes, lost_ranges;
static uint64_t live_bytes[WM_USES], live_ranges[WM_USES], whole_reused;

static ULONG64 rounded(ULONG64 length)
{
	ULONG64 r = (length + PAGE - 1) & ~(ULONG64)(PAGE - 1);

	return r ? r : PAGE;
}

// Every chunk has a tail no range is ever cut from: a read a little past
// the last buffer of a chunk (an index fetch, a vertex, a constant buffer
// indexed past its end, which Direct3D answers with zeros) then lands in
// mapped memory instead of faulting the GPU (runs rn71, rn75, rn76: page
// faults after minutes of Edge's work, the faulting buffer unknown).
#define CHUNK_TAIL (64u << 10)

// Idle chunks of each sort (whole, and of CHUNK_BYTES) kept for reuse.
#define IDLE_KEPT 2

static uint64_t chunks_freed, chunks_lost;

static struct chunk *new_chunk(ULONG64 size)
{
	struct ruxnpvg_bo_create bo;
	struct memory whole;
	struct chunk *c;

	unsigned slot;

	// A slot a freed chunk left, else a new one.
	for (slot = 0; slot < chunk_count && chunks[slot].handle; slot++)
		;
	if (slot == MOST_CHUNKS || size + CHUNK_TAIL > RUXNPVG_BO_MAX_BYTES) {
		pvg_log("FAILED: no buffer object of %llu bytes (%u made)", size, chunk_count);
		return NULL;
	}
	memset(&bo, 0, sizeof(bo));
	bo.size = size + CHUNK_TAIL;
	if (!wm_control("bo_create", RUXNPVG_IOCTL_BO_CREATE, &bo, sizeof(bo), &bo, sizeof(bo)))
		return NULL;
	c = &chunks[slot];
	chunk_free[slot].count = 0;
	c->handle = bo.handle;
	c->user = (UCHAR *)(ULONG_PTR)bo.user_address;
	c->task_address = bo.address;
	c->size = bo.size - CHUNK_TAIL;
	c->used = 0;
	c->live = 0;
	c->whole = false;
	// One buffer object over the whole chunk, tail too, and its address on
	// the host.
	memset(&whole, 0, sizeof(whole));
	whole.chunk = c;
	whole.length = bo.size;
	c->number = wm_take_number();
	if (!c->number || !create_object(c->number, RUXNPVG_OBJECT_BUFFER, &whole, 0, NULL, 0) ||
	    !wm_place(c->number, WM_PLACE_LISTED, 0, &c->gpu_address)) {
		pvg_log("FAILED: the buffer object over chunk %u", slot);
		return NULL;
	}
	pvg_log("chunk %u: %llu bytes, object %u at host address %#llx", slot, c->size, c->number,
		(unsigned long long)c->gpu_address);
	if (slot == chunk_count)
		chunk_count++;
	allocated_bytes += bo.size;
	return c;
}

unsigned wm_chunk_index(const struct chunk *c)
{
	return (unsigned)(c - chunks);
}

// The first free range of a chunk that holds this many bytes, cut from it.
static bool take_free(unsigned index, ULONG64 bytes, ULONG64 *offset)
{
	struct chunk_free *f = &chunk_free[index];
	unsigned i;

	for (i = 0; i < f->count; i++) {
		struct range *r = &f->ranges[i];

		if (r->length < bytes)
			continue;
		*offset = r->offset;
		r->offset += bytes;
		r->length -= bytes;
		if (!r->length) {
			memmove(r, r + 1, (f->count - i - 1) * sizeof(*r));
			f->count--;
		}
		reused_bytes += bytes;
		return true;
	}
	return false;
}

// The idle whole chunk that holds this many bytes with the least room to
// spare, or NULL.
static struct chunk *idle_whole(ULONG64 bytes)
{
	struct chunk *best = NULL;
	unsigned i;

	for (i = 0; i < chunk_count; i++) {
		struct chunk *c = &chunks[i];

		if (c->whole && !c->live && c->size >= bytes && (!best || c->size < best->size))
			best = c;
	}
	return best;
}

static void hand_out(struct chunk *c, ULONG64 offset, ULONG64 length, enum wm_use use, bool zeroed,
		     struct memory *out)
{
	out->chunk = c;
	out->offset = offset;
	out->length = length;
	out->zeroed = zeroed;
	out->use = (unsigned char)use;
	c->live++;
	live_bytes[use] += rounded(length);
	live_ranges[use]++;
}

// An object bigger than CHUNK_BYTES: an idle whole chunk, else a new one.
static bool allocate_whole(ULONG64 length, enum wm_use use, struct memory *out)
{
	ULONG64 bytes = rounded(length);
	struct chunk *c = idle_whole(bytes);

	if (c) {
		whole_reused++;
		hand_out(c, 0, length, use, false, out);
		return true;
	}
	c = new_chunk(bytes);
	if (!c)
		return false;
	c->whole = true;
	c->used = bytes;
	hand_out(c, 0, length, use, true, out);
	return true;
}

// Whole device pages of a buffer object: a free range given back before,
// else the untouched end of a chunk (zeroed by the driver), else a new
// chunk; whole chunks are never cut. Under winemetal's lock.
bool allocate(ULONG64 length, enum wm_use use, struct memory *out)
{
	ULONG64 bytes = rounded(length), offset;
	struct chunk *c = NULL;
	unsigned i;

	memset(out, 0, sizeof(*out));
	if (bytes > CHUNK_BYTES)
		return allocate_whole(length, use, out);
	for (i = 0; i < chunk_count; i++) {
		if (!chunks[i].whole && take_free(i, bytes, &offset)) {
			hand_out(&chunks[i], offset, length, use, false, out);
			return true;
		}
	}
	for (i = 0; i < chunk_count; i++) {
		if (!chunks[i].whole && chunks[i].size - chunks[i].used >= bytes) {
			c = &chunks[i];
			break;
		}
	}
	if (!c)
		c = new_chunk(CHUNK_BYTES);
	if (!c)
		return false;
	hand_out(c, c->used, length, use, true, out);
	c->used += bytes;
	return true;
}

// Gives an empty chunk back to the driver: its object deleted, then the
// buffer object destroyed. A chunk whose object or buffer object would not
// go is never used again (lost), so nothing can name memory the device
// still maps or the driver has freed.
static void free_chunk(struct chunk *c)
{
	struct ruxnpvg_bo_destroy d = { c->handle, 0 };
	unsigned index = wm_chunk_index(c);

	if (wm_process_ending())
		return;
	if (!wm_destroy_task_object(c->number) ||
	    !wm_control("bo_destroy", RUXNPVG_IOCTL_BO_DESTROY, &d, sizeof(d), NULL, 0)) {
		if (++chunks_lost == 1)
			pvg_log("FAILED: chunk %u could not be given back; it is not used again", index);
		c->size = c->used = 0;
		c->whole = false;
		chunk_free[index].count = 0;
		return;
	}
	allocated_bytes -= c->size + CHUNK_TAIL;
	chunks_freed++;
	memset(c, 0, sizeof(*c));
	chunk_free[index].count = 0;
}

// An empty chunk goes back unless it is one of the last IDLE_KEPT of its
// sort.
static void maybe_free(struct chunk *c)
{
	unsigned i, idle = 0;

	for (i = 0; i < chunk_count; i++)
		if (chunks[i].handle && chunks[i].size && !chunks[i].live && chunks[i].whole == c->whole)
			idle++;
	if (idle > IDLE_KEPT)
		free_chunk(c);
}

// A range no longer used: into its chunk's list, joined to its neighbours.
// Never back into the untouched end, which is taken to be zero. Under
// winemetal's lock.
void wm_give_back(struct memory *m)
{
	struct chunk *c = m->chunk;
	struct chunk_free *f;
	ULONG64 bytes = rounded(m->length), offset = m->offset;
	unsigned i;

	if (!c)
		return;
	m->chunk = NULL;
	c->live--;
	live_bytes[m->use] -= bytes;
	live_ranges[m->use]--;
	given_back_bytes += bytes;
	// A whole chunk waits whole for the next big object.
	if (c->whole) {
		if (!c->live)
			maybe_free(c);
		return;
	}
	f = &chunk_free[c - chunks];

	// Where it goes: before the first range that starts after it.
	for (i = 0; i < f->count && f->ranges[i].offset < offset; i++)
		;
	if (i > 0 && f->ranges[i - 1].offset + f->ranges[i - 1].length == offset) {
		// Joined to the one before, and maybe to the one after too.
		f->ranges[i - 1].length += bytes;
		if (i < f->count && offset + bytes == f->ranges[i].offset) {
			f->ranges[i - 1].length += f->ranges[i].length;
			memmove(&f->ranges[i], &f->ranges[i + 1], (f->count - i - 1) * sizeof(f->ranges[0]));
			f->count--;
		}
	} else if (i < f->count && offset + bytes == f->ranges[i].offset) {
		f->ranges[i].offset = offset;
		f->ranges[i].length += bytes;
	} else if (f->count < MOST_FREE_RANGES) {
		memmove(&f->ranges[i + 1], &f->ranges[i], (f->count - i) * sizeof(f->ranges[0]));
		f->ranges[i].offset = offset;
		f->ranges[i].length = bytes;
		f->count++;
	} else if (++lost_ranges % 64 == 1) {
		pvg_log("%llu ranges not kept: a chunk's list of free ranges is full", lost_ranges);
	}
	if (!c->live)
		maybe_free(c);
}

void *address(const struct memory *m)
{
	return m->chunk->user + m->offset;
}

uint64_t wm_allocated_bytes(void)
{
	return allocated_bytes;
}

// What the chunks hold now, beside the "alive after" line: the live ranges
// by use, the room in the free lists, the untouched ends, and the whole
// chunks, idle or not (run rn88: what Edge's GPU process kept in 581 MB).
void wm_memory_say_live(void)
{
	static const char *const names[WM_USES] = { "buffers", "textures", "command buffers", "libraries",
						    "helpers" };
	uint64_t free_bytes = 0, untouched = 0, whole_bytes = 0, idle_bytes = 0;
	unsigned i, j, whole = 0, idle = 0, ranges = 0;
	char text[512];
	int at = 0;

	for (i = 0; i < chunk_count; i++) {
		if (chunks[i].whole) {
			whole++;
			whole_bytes += chunks[i].size;
			if (!chunks[i].live) {
				idle++;
				idle_bytes += chunks[i].size;
			}
			continue;
		}
		untouched += chunks[i].size - chunks[i].used;
		for (j = 0; j < chunk_free[i].count; j++)
			free_bytes += chunk_free[i].ranges[j].length;
		ranges += chunk_free[i].count;
	}
	for (i = 0; i < WM_USES && at >= 0 && at < (int)sizeof(text); i++)
		at += snprintf(text + at, sizeof(text) - at, "%s%s %llu in %llu", i ? ", " : "", names[i],
			       (unsigned long long)live_bytes[i], (unsigned long long)live_ranges[i]);
	pvg_log("memory alive: %s; free in lists %llu in %u ranges, untouched %llu; whole chunks %u of %llu bytes, "
		"%u idle of %llu, %llu taken again; chunks given back %llu, lost %llu", text, (unsigned long long)free_bytes, ranges,
		(unsigned long long)untouched, whole, (unsigned long long)whole_bytes, idle,
		(unsigned long long)idle_bytes, (unsigned long long)whole_reused, (unsigned long long)chunks_freed,
		(unsigned long long)chunks_lost);
}

// For the log at a process's end: what was given back and taken again.
void wm_memory_report(void)
{
	pvg_log("memory: %u chunks, %llu bytes; %llu bytes given back, %llu taken again, %llu ranges not kept",
		chunk_count, (unsigned long long)allocated_bytes, (unsigned long long)given_back_bytes,
		(unsigned long long)reused_bytes, (unsigned long long)lost_ranges);
}
