// SPDX-License-Identifier: MIT
//
// winemetal over ruxnpvg: the views of textures, and what DXMT asks of a
// texture. A view is an object of its own (kind 8, operation 8) with its
// own resource id, placed by naming it with its base listed; a pass or a
// blit into it names the base at the view's level and slice (translate.c).

#include "wm_internal.h"

// How many slices a view of the whole texture has, as Metal counts them for
// a view (and DXMT's arrayLength): six faces to a cube, an array's elements.
static uint32_t slices_of(const struct WMTTextureInfo *info)
{
	uint32_t length = info->array_length ? info->array_length : 1;

	switch (info->type) {
	case WMTTextureTypeCube:
		return 6;
	case WMTTextureType2DArray:
		return length;
	default:
		return 1;
	}
}

static bool swizzle_is_identity(struct WMTTextureSwizzleChannels swizzle)
{
	return swizzle.r == WMTTextureSwizzleRed && swizzle.g == WMTTextureSwizzleGreen &&
	       swizzle.b == WMTTextureSwizzleBlue && swizzle.a == WMTTextureSwizzleAlpha;
}

// A view may read a texture as another format of its family (the same
// channels, another number kind or sRGB), as the kernel lets through
// (pvg_view.c, the same table; scenes bgra-srgb-view, bc1-srgb-view).
static uint32_t format_family(uint32_t f)
{
	static const uint16_t families[][2] = {
		{ 10, 14 }, { 20, 25 }, { 30, 34 }, { 53, 55 }, { 60, 65 }, { 70, 74 }, { 80, 81 }, { 90, 91 },
		{ 103, 105 }, { 110, 115 }, { 123, 125 },
		{ 130, 131 }, { 132, 133 }, { 134, 135 }, { 140, 141 }, { 142, 143 }, { 150, 151 }, { 152, 153 },
	};

	for (unsigned i = 0; i < sizeof(families) / sizeof(families[0]); i++)
		if (f >= families[i][0] && f <= families[i][1])
			return families[i][0];
	return 0;
}

// RUXN_NO_FORMAT_VIEWS in a process's environment keeps its views to the
// texture's own format, as before 0.3.64 (one new device behaviour a run).
static bool format_viewable(uint32_t base, uint32_t view)
{
	static int own_only = -1;

	if (own_only < 0)
		own_only = GetEnvironmentVariableA("RUXN_NO_FORMAT_VIEWS", NULL, 0) != 0;
	return view == base || (!own_only && format_family(base) && format_family(base) == format_family(view));
}

// A format and its sRGB twin: Metal views between them without the
// pixel-format-view usage.
static bool srgb_pair(uint32_t a, uint32_t b)
{
	static const uint16_t linear[] = { 10, 30, 70, 80, 130, 132, 134, 152 };

	for (unsigned i = 0; i < sizeof(linear) / sizeof(linear[0]); i++)
		if ((a == linear[i] && b == linear[i] + 1u) || (b == linear[i] && a == linear[i] + 1u))
			return true;
	return false;
}

// The view types held over each kind (harness/kinds.sh, after the
// contributor's scenes): a 2D view of a level, of a slice of a 2D array and
// of a face of a cube (view-level, array-view, cube-view); a 2D array view
// of slices of a 2D array (array-view-array); a cube view over a cube and
// over six slices of a 2D array (cube-sample, array-cube-view); a 3D view
// of a 3D texture's levels (3d-view-level). The format is of the texture's
// family (format_viewable).
static bool view_is_held(uint32_t base_type, uint32_t type, uint32_t slices)
{
	switch (base_type) {
	case WMTTextureType2D:
		return type == WMTTextureType2D && slices == 1;
	case WMTTextureType2DArray:
		return (type == WMTTextureType2D && slices == 1) || type == WMTTextureType2DArray ||
		       (type == WMTTextureTypeCube && slices == 6);
	case WMTTextureTypeCube:
		return (type == WMTTextureType2D && slices == 1) || (type == WMTTextureTypeCube && slices == 6);
	case WMTTextureType3D:
		return type == WMTTextureType3D && slices == 1;
	default:
		return false;
	}
}

// A view of some levels and slices of a texture of ours, as a texture of
// `type`. Under winemetal's lock; deleted when its last reference goes.
static obj_handle_t range_view(const struct object *t, uint32_t format, uint32_t type, uint16_t first, uint16_t levels,
			       uint16_t first_slice, uint16_t slices, uint64_t *out_id)
{
	UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	const struct WMTTextureInfo *base_info = &t->u.texture.info;
	uint32_t base = t->u.texture.number, number = wm_take_number();
	uint64_t id = 0;
	obj_handle_t h;
	size_t length;

	length = pvg_wire_texture_view_range(description, sizeof(description), number, base,
					     wm_depth_format(format), type, first, levels, first_slice,
					     slices, NULL);
	if (!number || !length ||
	    !wm_create_bare_object(number, RUXNPVG_OBJECT_TEXTURE_VIEW, 0, description, (ULONG)length))
		return 0;
	if (!wm_place(number, WM_PLACE_VIEW, base, &id))
		pvg_log("FAILED: the placement of view %u of levels %u+%u slices %u+%u of texture %u", number, first,
			levels, first_slice, slices, base);
	h = new_object(K_TEXTURE);
	if (!h)
		return 0;
	objects[h].u.texture.number = number;
	objects[h].u.texture.listed = base;
	objects[h].u.texture.base = base;
	objects[h].u.texture.base_level = first;
	objects[h].u.texture.base_slice = first_slice;
	objects[h].u.texture.device_levels = levels;
	objects[h].u.texture.resource_id = id;
	objects[h].u.texture.info = *base_info;
	objects[h].u.texture.info.type = type;
	objects[h].u.texture.info.pixel_format = format;
	objects[h].u.texture.info.width = base_info->width >> first ? base_info->width >> first : 1;
	objects[h].u.texture.info.height = base_info->height >> first ? base_info->height >> first : 1;
	objects[h].u.texture.info.mipmap_level_count = levels;
	objects[h].u.texture.info.array_length = type == WMTTextureType2DArray ? slices : 1;
	objects[h].u.texture.view = true;	// not a texture's own memory
	objects[h].u.texture.own_view = true;	// deleted with its handle
	if (out_id)
		*out_id = id;
	note_once(type == base_info->type && type == WMTTextureType2D ? "a view of some levels of a texture: kind 8"
								      : "a view of slices or of another type: kind 8");
	return h;
}

// The whole texture as it is: the texture again, with no object of its own.
static obj_handle_t same_texture(obj_handle_t texture, const struct object *t, uint64_t *out_id)
{
	obj_handle_t h = new_object(K_TEXTURE);

	if (!h)
		return 0;
	objects[h].u.texture = t->u.texture;
	objects[h].u.texture.view = true;	// the texture's number is not its own
	objects[h].u.texture.own_view = false;
	objects[h].u.texture.surface_handle = 0;	// the texture's, not the alias's
	// Held, as Metal's view holds its texture: the alias's number is the
	// texture's, which must not leave the task first.
	objects[h].u.texture.over = texture;
	objects[texture].references++;
	if (out_id)
		*out_id = t->u.texture.resource_id;
	note_once("a view that is the whole texture: the texture again");
	return h;
}

obj_handle_t MTLTexture_newTextureView(obj_handle_t texture, enum WMTPixelFormat format,
				       enum WMTTextureType texture_type, uint16_t level_start, uint16_t level_count,
				       uint16_t slice_start, uint16_t slice_count,
				       struct WMTTextureSwizzleChannels swizzle, uint64_t *out_gpu_resource_id)
{
	struct object *t;
	obj_handle_t h = 0;

	CALLED();
	EnterCriticalSection(&lock);
	t = get(texture, K_TEXTURE);
	// A shader-resource view of a depth texture (R24_UNORM_X8_TYPELESS,
	// R32_FLOAT_X8X24_TYPELESS): DXMT asks for the texture's own format with
	// its R001 swizzle bits (Edge's WebGL, rn87: format 860420, that is 260
	// with them). Shaders read depth as .r (airconv), so the texture itself
	// serves, the depth half of a split Depth32Float_Stencil8; only a shader
	// reading .gba would see the texture's values instead of 0, 0, 1.
	if (t && (format & ~(enum WMTPixelFormat)0xfff) == WMTPixelFormatR001Swizzle &&
	    (format & 0xfff) == t->u.texture.info.pixel_format &&
	    (t->u.texture.info.pixel_format == WMTPixelFormatDepth32Float_Stencil8 ||
	     t->u.texture.info.pixel_format == WMTPixelFormatDepth32Float ||
	     t->u.texture.info.pixel_format == WMTPixelFormatDepth16Unorm)) {
		format = t->u.texture.info.pixel_format;
		note_once("a depth view with the R001 swizzle: the depth texture itself");
	}
	uint16_t device_levels = t && t->u.texture.device_levels ? t->u.texture.device_levels : level_count;

	if (t && format == t->u.texture.info.pixel_format && texture_type == t->u.texture.info.type &&
	    level_start == 0 && level_count == t->u.texture.info.mipmap_level_count && slice_start == 0 &&
	    slice_count == slices_of(&t->u.texture.info) && swizzle_is_identity(swizzle)) {
		h = same_texture(texture, t, out_gpu_resource_id);
	} else if (t && !t->u.texture.view && !t->u.texture.surface &&
		   format_viewable(t->u.texture.info.pixel_format, format) &&
		   (format == t->u.texture.info.pixel_format || (t->u.texture.info.usage & WMTTextureUsagePixelFormatView) ||
		    srgb_pair(t->u.texture.info.pixel_format, format)) &&
		   level_count && level_start < device_levels && swizzle_is_identity(swizzle) &&
		   slice_start + slice_count <= slices_of(&t->u.texture.info) &&
		   view_is_held(t->u.texture.info.type, texture_type, slice_count)) {
		// A compressed texture's levels smaller than a block are not
		// on the device (wm_resources.c): the view stops before them.
		if (level_start + level_count > device_levels)
			level_count = device_levels - level_start;
		h = range_view(t, format, texture_type, level_start, level_count, slice_start, slice_count,
			       out_gpu_resource_id);
	}
	if (!h)
		pvg_log("FAILED: newTextureView of type %u format %u: format %u type %u levels %u+%u slices %u+%u "
			"swizzle %u%u%u%u", t ? (unsigned)t->u.texture.info.type : 0u,
			t ? (unsigned)t->u.texture.info.pixel_format : 0u, (unsigned)format, (unsigned)texture_type,
			level_start, level_count, slice_start, slice_count, swizzle.r, swizzle.g, swizzle.b, swizzle.a);
	LeaveCriticalSection(&lock);
	return h;
}

static struct WMTTextureInfo *texture_info(obj_handle_t texture)
{
	struct object *t = get(texture, K_TEXTURE);

	return t ? &t->u.texture.info : NULL;
}

enum WMTPixelFormat MTLTexture_pixelFormat(obj_handle_t texture)
{
	struct WMTTextureInfo *i = texture_info(texture);

	CALLED();
	return i ? i->pixel_format : WMTPixelFormatInvalid;
}

uint64_t MTLTexture_width(obj_handle_t texture)
{
	struct WMTTextureInfo *i = texture_info(texture);

	CALLED();
	return i ? i->width : 0;
}

uint64_t MTLTexture_height(obj_handle_t texture)
{
	struct WMTTextureInfo *i = texture_info(texture);

	CALLED();
	return i ? i->height : 0;
}

// Depth and array length as Metal answers them: a 3D texture's depth, an
// array's elements (a cube is one).
uint64_t MTLTexture_depth(obj_handle_t texture)
{
	struct WMTTextureInfo *i = texture_info(texture);

	CALLED();
	return i ? (i->type == WMTTextureType3D && i->depth ? i->depth : 1) : 0;
}

uint64_t MTLTexture_arrayLength(obj_handle_t texture)
{
	struct WMTTextureInfo *i = texture_info(texture);

	CALLED();
	return i ? (i->array_length ? i->array_length : 1) : 0;
}

// Still 1 for every texture: DXMT's GenerateMips asks this and sends a
// mipmap generation only for more, which no helper scene here has held for
// our textures and views. So GenerateMips leaves the levels as they are.
uint64_t MTLTexture_mipmapLevelCount(obj_handle_t texture)
{
	CALLED();
	return texture_info(texture) ? 1 : 0;
}
