// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Objects of kind 8: textures one operation describes from
// another object. The kernel writes no head for them, so it checks the
// whole operation instead: that it names the new object's own number, that
// what it is made from is an object of this task of the right kind, and for
// a texture over a buffer that every byte it can reach lies inside the
// buffer, so the host never reads past what the task owns.
//
//   operation 7, 20 bytes: a view of a whole texture over a surface (kind
//     11), the base's number at 12 (seam study, view-look).
//   operation 8, 52 bytes: a view of some levels and slices of a texture
//     (kind 2 or 11), of a type view_held allows, the base's format: the base at 12,
//     format at 16, type at 18, first level, levels, first slice and slices
//     from 20, 64 bits each (wire library, held by the helper in view-level).
//   operation 9, 64 bytes: a texture over a buffer (kind 1): the buffer's
//     number at 12, the offset at 16 and the bytes a row at 24, 64 bits
//     each, the texture descriptor at 32 (held by the helper in scene
//     texture-buffer, 2026-11-26).

#include "pvg.h"
#include "pvg_task.h"

#define OP_RANGE		8
#define OP_RANGE_BYTES		52
#define RANGE_FORMAT		16
#define RANGE_TYPE		18
#define RANGE_FIRST_LEVEL	20
#define RANGE_LEVELS		28
#define RANGE_FIRST_SLICE	36
#define RANGE_SLICES		44
#define OP_OVER_BUFFER		9
#define OP_OVER_BUFFER_BYTES	64
#define OVER_BUFFER_BASE	12
#define OVER_BUFFER_OFFSET	16
#define OVER_BUFFER_ROW		24
#define DESCRIPTOR		32	// the texture descriptor in an operation 9
#define DESC_TYPE		0	// low four bits
#define DESC_FORMAT		2	// 16 bits
#define DESC_WIDTH		4
#define DESC_HEIGHT		8
#define DESC_DEPTH		12
#define DESC_LEVELS		16	// 16 bits each from here
#define DESC_SAMPLES		18
#define DESC_ARRAY		20
#define TYPE_2D			2	// MTLTextureType2D
#define TYPE_2D_ARRAY		3
#define TYPE_CUBE		5
#define TYPE_CUBE_ARRAY		6
#define TYPE_3D			7
#define TYPE_BUFFER		9	// MTLTextureTypeTextureBuffer
#define MOST_SIDE		16384
#define LEVELS_MASK		0x3fff	// a plain texture's level count, 14 bits
#define PLAIN_DIMENSION		24	// in a kind 2 tail: before the levels
#define PLAIN_LEVEL		36	// each level's layout
#define OP_HEAD			12	// an operation's number, length and object
#define SURFACE_DESCRIPTOR	12	// in operation 12

static ULONG get32(const UCHAR *at)
{
	return (ULONG)at[0] | (ULONG)at[1] << 8 | (ULONG)at[2] << 16 | (ULONG)at[3] << 24;
}

static ULONG64 get64(const UCHAR *at)
{
	return (ULONG64)get32(at) | (ULONG64)get32(at + 4) << 32;
}

static USHORT get16(const UCHAR *at)
{
	return (USHORT)(at[0] | at[1] << 8);
}

// Bytes a pixel of the uncompressed colour formats a texture over a buffer
// may have, by Metal's MTLPixelFormat numbers; 0 for any other, which is
// refused.
static ULONG bytes_per_pixel(ULONG format)
{
	switch (format) {
	case 1:							// A8
	case 10: case 12: case 13: case 14:			// R8
		return 1;
	case 20: case 22: case 23: case 24: case 25:		// R16
	case 30: case 32: case 33: case 34:			// RG8
		return 2;
	case 53: case 54: case 55:				// R32
	case 60: case 62: case 63: case 64: case 65:		// RG16
	case 70: case 71: case 72: case 73: case 74:		// RGBA8
	case 80: case 81:					// BGRA8
	case 90: case 91: case 92: case 93: case 94:		// packed 32-bit
		return 4;
	case 103: case 104: case 105:				// RG32
	case 110: case 112: case 113: case 114: case 115:	// RGBA16
		return 8;
	case 123: case 124: case 125:				// RGBA32
		return 16;
	default:
		return 0;
	}
}

static NTSTATUS check_over_buffer(struct pvg_task *task, const struct pvg_object *self, const UCHAR *tail)
{
	const struct pvg_object *base = task_object(task, get32(tail + OVER_BUFFER_BASE));
	const UCHAR *d = tail + DESCRIPTOR;
	ULONG64 offset = get64(tail + OVER_BUFFER_OFFSET), row = get64(tail + OVER_BUFFER_ROW);
	ULONG type = d[DESC_TYPE] & 15, width = get32(d + DESC_WIDTH), height = get32(d + DESC_HEIGHT);
	ULONG bpp = bytes_per_pixel(get16(d + DESC_FORMAT));

	if (!base || base == self || base->kind != RUXNPVG_OBJECT_BUFFER)
		return E_INVAL;
	if ((type != TYPE_2D && type != TYPE_BUFFER) || get32(d + DESC_DEPTH) != 1 || get16(d + DESC_LEVELS) != 1 ||
	    get16(d + DESC_SAMPLES) != 1 || get16(d + DESC_ARRAY) != 1 || !bpp)
		return E_INVAL;
	if (!width || !height || width > MOST_SIDE * MOST_SIDE || height > MOST_SIDE ||
	    row < (ULONG64)width * bpp || row > base->bytes)
		return E_INVAL;
	// The last row ends inside the buffer; the sums cannot wrap, as each
	// part is below the buffer's length or bounded above.
	if (offset > base->bytes || row * (height - 1) + (ULONG64)width * bpp > base->bytes - offset)
		return E_INVAL;
	return STATUS_SUCCESS;
}

// Whether a texture over a surface (operation 12, descriptor at 12) fits the
// surface's rows: BGRA8 as wide as the surface, or a format of one byte a
// pixel four times as wide (helper scene surface-r8), in each case short of
// the surface's width by less than the padding that makes a surface a
// multiple of 4 pixels wide (ruxnwddm_umd.h; helper scene surface-narrow).
// Any other format is refused.
#define BGRA8_UNORM	80
#define BGRA8_SRGB	81
#define RGBA8_UNORM	70
#define RGBA8_SRGB	71

BOOLEAN pvg_surface_texture_fits(const UCHAR *tail, ULONG surface_width)
{
	ULONG format = get16(tail + SURFACE_DESCRIPTOR + DESC_FORMAT);
	ULONG width = get32(tail + SURFACE_DESCRIPTOR + DESC_WIDTH);

	// Four bytes a pixel, as the surface: BGRA8, or RGBA8 shared between
	// programs that read it so (helper scene surface-rgba).
	if (format == BGRA8_UNORM || format == BGRA8_SRGB || format == RGBA8_UNORM || format == RGBA8_SRGB)
		return width && RUXNWDDM_SURFACE_WIDTH(width) == surface_width;
	if (bytes_per_pixel(format) != 1 || !width)
		return FALSE;
	return RUXNWDDM_SURFACE_WIDTH_ONE_BYTE(width) == surface_width;
}

// Notes what a view will be checked against: a texture's levels and pixel
// format, read from its own description as the program gave it (kind 2:
// the levels' layout, then operation 1 with its descriptor; kind 11:
// operation 12 with its descriptor). Unreadable, they stay 0 and no view
// of it passes.
void pvg_view_note_texture(struct pvg_object *object, const UCHAR *tail, ULONG size)
{
	ULONG at;

	object->levels = 0;
	object->format = 0;
	object->type = TYPE_2D;
	object->slices = 1;
	if (object->kind == RUXNPVG_OBJECT_SURFACE_TEXTURE) {
		if (size >= SURFACE_DESCRIPTOR + DESC_FORMAT + 2) {
			object->levels = 1;
			object->format = get16(tail + SURFACE_DESCRIPTOR + DESC_FORMAT);
		}
		return;
	}
	if (object->kind != RUXNPVG_OBJECT_TEXTURE || size < 2)
		return;
	at = PLAIN_DIMENSION + PLAIN_LEVEL * (get16(tail) & LEVELS_MASK) + OP_HEAD + DESC_FORMAT;
	if (at + 2 > size)
		return;
	object->levels = get16(tail) & LEVELS_MASK;
	object->format = get16(tail + at);
	// The descriptor's type and array length (from 0.3.64: arrays, cubes, 3D).
	if (at - DESC_FORMAT + DESC_ARRAY + 2 <= size) {
		ULONG length = get16(tail + at - DESC_FORMAT + DESC_ARRAY);

		object->type = tail[at - DESC_FORMAT + DESC_TYPE] & 15;
		object->slices = (length ? length : 1) * (object->type == TYPE_CUBE || object->type == TYPE_CUBE_ARRAY ? 6 : 1);
	}
}

// A view may read a texture as another format of its family: the same
// channels, another number kind or sRGB (Metal's pixel format views; held
// by the helper for BGRA8 as BGRA8 sRGB and BC1 as BC1 sRGB, scenes
// bgra-srgb-view and bc1-srgb-view). 0 for a format of no family here.
static USHORT format_family(USHORT f)
{
	static const USHORT families[][2] = {
		{ 10, 14 }, { 20, 25 }, { 30, 34 }, { 53, 55 }, { 60, 65 }, { 70, 74 }, { 80, 81 }, { 90, 91 },
		{ 103, 105 }, { 110, 115 }, { 123, 125 },
		{ 130, 131 }, { 132, 133 }, { 134, 135 }, { 140, 141 }, { 142, 143 }, { 150, 151 }, { 152, 153 },
	};
	ULONG i;

	for (i = 0; i < ARRAYSIZE(families); i++)
		if (f >= families[i][0] && f <= families[i][1])
			return families[i][0];
	return 0;
}

static BOOLEAN format_viewable(USHORT base, USHORT view)
{
	return view == base || (format_family(base) && format_family(base) == format_family(view));
}

// The view types let through over each kind of base, as the helper held
// them (harness/kinds.sh): 2D of a level of a 2D texture, of a slice of a
// 2D array or of a face of a cube; 2D arrays of slices of a 2D array; a
// cube of six slices of a 2D array or of a cube; 3D of a 3D texture.
static BOOLEAN view_held(UCHAR base, ULONG64 type, ULONG64 slices)
{
	switch (base) {
	case TYPE_2D:
		return type == TYPE_2D && slices == 1;
	case TYPE_2D_ARRAY:
		return (type == TYPE_2D && slices == 1) || type == TYPE_2D_ARRAY || (type == TYPE_CUBE && slices == 6);
	case TYPE_CUBE:
		return (type == TYPE_2D && slices == 1) || (type == TYPE_CUBE && slices == 6);
	case TYPE_3D:
		return type == TYPE_3D && slices == 1;
	default:
		return FALSE;
	}
}

static NTSTATUS check_range(struct pvg_task *task, const struct pvg_object *self, const UCHAR *tail)
{
	const struct pvg_object *base = task_object(task, get32(tail + OP_VIEW_BASE));
	ULONG64 first = get64(tail + RANGE_FIRST_LEVEL), levels = get64(tail + RANGE_LEVELS);

	if (!base || base == self ||
	    (base->kind != RUXNPVG_OBJECT_TEXTURE && base->kind != RUXNPVG_OBJECT_SURFACE_TEXTURE))
		return E_INVAL;
	ULONG64 first_slice = get64(tail + RANGE_FIRST_SLICE), slices = get64(tail + RANGE_SLICES);

	if (!base->levels || !format_viewable(base->format, get16(tail + RANGE_FORMAT)) ||
	    !view_held(base->type, get16(tail + RANGE_TYPE), slices) || !slices || first_slice >= base->slices ||
	    slices > base->slices - first_slice)
		return E_INVAL;
	if (!levels || first >= base->levels || levels > base->levels - first)
		return E_INVAL;
	return STATUS_SUCCESS;
}

// Whether the tail is a kind 8 operation this driver lets through for the
// object `self` of number `id`.
NTSTATUS pvg_view_check(struct pvg_task *task, const struct pvg_object *self, ULONG id, const UCHAR *tail,
			ULONG size)
{
	if (size < OP_TEXTURE_VIEW_BYTES || get32(tail + 4) != size || get32(tail + 8) != id)
		return E_INVAL;
	switch (get32(tail)) {
	case OP_TEXTURE_VIEW: {
		const struct pvg_object *base = task_object(task, get32(tail + OP_VIEW_BASE));

		if (size != OP_TEXTURE_VIEW_BYTES || !base || base == self ||
		    base->kind != RUXNPVG_OBJECT_SURFACE_TEXTURE || get16(tail + 16) != base->format)
			return E_INVAL;
		return STATUS_SUCCESS;
	}
	case OP_RANGE:
		if (size != OP_RANGE_BYTES)
			return E_INVAL;
		return check_range(task, self, tail);
	case OP_OVER_BUFFER:
		if (size != OP_OVER_BUFFER_BYTES)
			return E_INVAL;
		return check_over_buffer(task, self, tail);
	default:
		return E_INVAL;
	}
}
