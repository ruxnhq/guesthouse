// SPDX-License-Identifier: MIT
//
// Where the host put an object: the numbers DXMT's shaders read from memory.
//
// DXMT binds almost nothing: its shaders (airconv's) reach constant
// buffers, vertex buffers, textures and samplers through tables in memory
// that hold each one's GPU address or resource id, as Metal gives them. On
// this device those are the host's own numbers, which the host writes into
// the task's placement list once it has made the object: a buffer's word is
// its GPU address, a texture's and a sampler's is its resource id
// (measured on the device). The
// host makes a buffer or a texture when a command 67 first lists it as a
// resource, and a sampler when a command first names it.
//
// So right after winemetal makes such an object, wm_place hands the device
// a command buffer that does nothing but that (a blit segment with no
// command, listing the object; for a sampler a compute segment that binds
// it), and then reads the object's placement. The segment lies in a page of
// its own, outside the memory DXMT is given.
//
// A texture over a surface gets no placement at all (seam study: nothing
// is written for kind 11). A view of it does, as a view of a plain texture
// did, once a command names it with its base listed: so a view (kind 8)
// is made over each such texture and placed that way, a compute segment
// binding it.

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winioctl.h>
#include <stdbool.h>
#include <string.h>

#include "winemetal.h"
#include "ruxnpvg_ioctl.h"
#include "pvg_wire.h"
#include "winemetal_pvg.h"

#define SCRATCH_BYTES	RUXNPVG_DEVICE_PAGE_SIZE

static ULONG scratch_handle;
static UCHAR *scratch;

static bool scratch_ready(void)
{
	struct ruxnpvg_bo_create bo;

	if (scratch)
		return true;
	memset(&bo, 0, sizeof(bo));
	bo.size = SCRATCH_BYTES;
	if (!wm_control("bo_create (placement)", RUXNPVG_IOCTL_BO_CREATE, &bo, sizeof(bo), &bo, sizeof(bo)))
		return false;
	scratch_handle = bo.handle;
	scratch = (UCHAR *)(ULONG_PTR)bo.user_address;
	return scratch != NULL;
}

// Objects whose placement the device refused: it never made them (run
// rn71: a texture over a surface whose rows were not 16-byte aligned), and
// every later command buffer naming one is refused too. Such a command
// buffer is not sent at all (wm_commands.c). A number taken again is
// cleared (winemetal_pvg.c). Under winemetal's lock.
#define BROKEN_NUMBERS 4096
static uint32_t broken[BROKEN_NUMBERS / 32];

static void broken_set(uint32_t number, bool on)
{
	if (number >= BROKEN_NUMBERS)
		return;
	if (on)
		broken[number / 32] |= 1U << (number % 32);
	else
		broken[number / 32] &= ~(1U << (number % 32));
}

void wm_broken_clear(uint32_t number)
{
	broken_set(number, false);
}

uint32_t wm_broken_named(const uint32_t *ids, unsigned count)
{
	unsigned i;

	for (i = 0; i < count; i++) {
		if (ids[i] < BROKEN_NUMBERS && broken[ids[i] / 32] & 1U << (ids[i] % 32))
			return ids[i];
	}
	return 0;
}

// The host's number for the object, in *word; false if it could not be had.
// base is the listed texture for a view, else unused. Under winemetal's lock.
bool wm_place(uint32_t number, enum wm_place_how how, uint32_t base, uint64_t *word)
{
	static UCHAR in[sizeof(struct ruxnpvg_submit) + sizeof(struct ruxnpvg_resource) +
			sizeof(struct ruxnpvg_command_buffer)];
	struct ruxnpvg_submit *s = (void *)in;
	struct ruxnpvg_resource *r = (void *)(s + 1);
	struct ruxnpvg_command_buffer *b;
	struct ruxnpvg_object_placement placement;
	struct pvg_wire wire;
	size_t length;

	*word = 0;
	if (!scratch_ready())
		return false;
	pvg_wire_init(&wire, scratch, SCRATCH_BYTES);
	switch (how) {
	case WM_PLACE_SAMPLER:
		pvg_wire_begin_compute(&wire);
		pvg_wire_set_sampler(&wire, 0, number);
		break;
	case WM_PLACE_VIEW:
		pvg_wire_begin_compute(&wire);
		pvg_wire_set_texture(&wire, 0, number);
		break;
	case WM_PLACE_LISTED:
		pvg_wire_begin_blit(&wire);
		break;
	}
	length = pvg_wire_end(&wire);
	if (!length)
		return false;

	// A sampler is no resource: the list is empty and the command buffer
	// follows the head at once. A view is none either; its base is listed.
	// (Written apart: the resource and the command buffer would otherwise
	// share bytes under two types.)
	memset(in, 0, sizeof(in));
	s->resource_count = how == WM_PLACE_SAMPLER ? 0 : 1;
	s->buffer_count = 1;
	if (how == WM_PLACE_LISTED)
		r->id = number;
	else if (how == WM_PLACE_VIEW)
		r->id = base;
	b = (void *)(in + sizeof(*s) + s->resource_count * sizeof(*r));
	b->handle = scratch_handle;
	b->offset = 0;
	b->size = length;
	if (!wm_control("submit (placement)", RUXNPVG_IOCTL_SUBMIT, in, (DWORD)((UCHAR *)(b + 1) - in), NULL, 0)) {
		// The device refused the object, or its base for a view.
		broken_set(number, true);
		if (how == WM_PLACE_VIEW)
			broken_set(base, true);
		return false;
	}

	memset(&placement, 0, sizeof(placement));
	placement.id = number;
	if (!wm_control("object_placement", RUXNPVG_IOCTL_OBJECT_PLACEMENT, &placement, sizeof(placement),
			&placement, sizeof(placement)))
		return false;
	*word = placement.words[0];
	return *word != 0;
}
