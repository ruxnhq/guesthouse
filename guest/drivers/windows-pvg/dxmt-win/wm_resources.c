// SPDX-License-Identifier: MIT
//
// winemetal over ruxnpvg: buffers and textures, over the task's memory or
// over surfaces and buffers. The views of textures are wm_views.c's.

#include "wm_internal.h"

// ---------------------------------------------------------------------
// Buffers and textures.

obj_handle_t MTLDevice_newBuffer(obj_handle_t dev, struct WMTBufferInfo *info)
{
	struct memory m;
	obj_handle_t h = 0;
	uint32_t number;

	CALLED();
	(void)dev;
	if (info->memory.ptr) {
		// The memory the GPU reads has to be a buffer object's: a
		// program's own pages cannot be handed to the device.
		pvg_log("FAILED: newBuffer with the caller's own memory (%llu bytes)", info->length);
		return 0;
	}
	EnterCriticalSection(&lock);
	if ((info->options & 0xf0) == WMTResourceStorageModePrivate)
		note_once("private storage for a buffer: made shared, as pvgtri's are");
	// A range of a chunk: the chunk's object, at the range's offset.
	if (allocate(info->length, WM_USE_BUFFER, &m)) {
		number = m.chunk->number;
		h = new_object(K_BUFFER);
		if (h) {
			objects[h].u.buffer.memory = m;
			objects[h].u.buffer.number = number;
			// Zero only when the range is new (wm_memory.c).
			objects[h].u.buffer.fresh = m.zeroed;
			info->memory.ptr = address(&m);
			info->gpu_address = m.chunk->gpu_address + m.offset;
		}
	}
	LeaveCriticalSection(&lock);
	if (!h)
		pvg_log("FAILED: newBuffer of %llu bytes", info->length);
	return h;
}

void MTLBuffer_didModifyRange(obj_handle_t buffer, uint64_t start, uint64_t length)
{
	struct object *b;

	CALLED();
	(void)start;
	(void)length;
	// Shared memory: nothing to send. The buffer is no longer fresh.
	b = get(buffer, K_BUFFER);
	if (b)
		b->u.buffer.fresh = false;
}

void MTLBuffer_updateContents(obj_handle_t buffer, uint64_t offset, struct WMTConstMemoryPointer data,
			      uint64_t length)
{
	struct object *b;

	CALLED();
	b = get(buffer, K_BUFFER);
	if (!b || offset + length > b->u.buffer.memory.length) {
		pvg_log("FAILED: updateContents out of range");
		return;
	}
	memcpy((UCHAR *)address(&b->u.buffer.memory) + offset, data.ptr, length);
	b->u.buffer.fresh = false;
}

static uint32_t bytes_per_pixel(uint32_t format)
{
	switch (format) {
	case WMTPixelFormatA8Unorm: case WMTPixelFormatR8Unorm: case WMTPixelFormatR8Unorm_sRGB:
	case WMTPixelFormatR8Snorm: case WMTPixelFormatR8Uint: case WMTPixelFormatR8Sint:
	case WMTPixelFormatStencil8:
		return 1;
	case WMTPixelFormatR16Unorm: case WMTPixelFormatR16Snorm: case WMTPixelFormatR16Uint:
	case WMTPixelFormatR16Sint: case WMTPixelFormatR16Float: case WMTPixelFormatRG8Unorm:
	case WMTPixelFormatRG8Unorm_sRGB: case WMTPixelFormatRG8Snorm: case WMTPixelFormatRG8Uint:
	case WMTPixelFormatRG8Sint: case WMTPixelFormatB5G6R5Unorm: case WMTPixelFormatDepth16Unorm:
		return 2;
	case WMTPixelFormatR32Uint: case WMTPixelFormatR32Sint: case WMTPixelFormatR32Float:
	case WMTPixelFormatRG16Unorm: case WMTPixelFormatRG16Snorm: case WMTPixelFormatRG16Uint:
	case WMTPixelFormatRG16Sint: case WMTPixelFormatRG16Float: case WMTPixelFormatRGBA8Unorm:
	case WMTPixelFormatRGBA8Unorm_sRGB: case WMTPixelFormatRGBA8Snorm: case WMTPixelFormatRGBA8Uint:
	case WMTPixelFormatRGBA8Sint: case WMTPixelFormatBGRA8Unorm: case WMTPixelFormatBGRA8Unorm_sRGB:
	case WMTPixelFormatRGB10A2Unorm: case WMTPixelFormatRGB10A2Uint: case WMTPixelFormatRG11B10Float:
	case WMTPixelFormatRGB9E5Float: case WMTPixelFormatBGR10A2Unorm: case WMTPixelFormatDepth32Float:
		return 4;
	case WMTPixelFormatRG32Uint: case WMTPixelFormatRG32Sint: case WMTPixelFormatRG32Float:
	case WMTPixelFormatRGBA16Unorm: case WMTPixelFormatRGBA16Snorm: case WMTPixelFormatRGBA16Uint:
	case WMTPixelFormatRGBA16Sint: case WMTPixelFormatRGBA16Float:
		return 8;
	case WMTPixelFormatRGBA32Uint: case WMTPixelFormatRGBA32Sint: case WMTPixelFormatRGBA32Float:
		return 16;
	default:
		return 0;
	}
}

// A texture's memory is what the device loads it from at first use, so a
// range taken again (wm_memory.c) is zeroed first, as a new one is. Once,
// when the texture is made: not in any frame's path.
static bool texture_memory_zero(const struct memory *m)
{
	if (!m->zeroed)
		memset(address(m), 0, m->length);
	return true;
}

#define PVG_STORAGE_PRIVATE 2	// MTLStorageModePrivate

static bool is_depth_or_stencil(uint32_t format)
{
	return format == WMTPixelFormatDepth16Unorm || format == WMTPixelFormatDepth32Float ||
	       format == WMTPixelFormatStencil8;
}

// Metal's Depth32Float_Stencil8, which DXMT makes for D24S8 and D32S8, is
// made as a Depth32Float texture and a Stencil8 texture of the same size,
// the pair the device takes in passes and pipelines (scenes depth and
// stencil-test); as one texture of its own format the helper refused it
// (scene depth-stencil-260). Pipelines name the two formats.
uint32_t wm_depth_format(uint32_t format)
{
	return format == WMTPixelFormatDepth32Float_Stencil8 ? WMTPixelFormatDepth32Float : format;
}

uint32_t wm_stencil_format(uint32_t format)
{
	return format == WMTPixelFormatDepth32Float_Stencil8 ? WMTPixelFormatStencil8 : format;
}

// A compressed format's block of 4 x 4 pixels: its bytes, 0 for a format
// that is not compressed. Every BC format was drawn through the helper as
// Metal on the Mac draws the same bytes (harness/kinds.sh).
static uint32_t block_bytes(uint32_t format)
{
	switch (format) {
	case WMTPixelFormatBC1_RGBA: case WMTPixelFormatBC1_RGBA_sRGB: case WMTPixelFormatBC4_RUnorm:
	case WMTPixelFormatBC4_RSnorm:
		return 8;
	case WMTPixelFormatBC2_RGBA: case WMTPixelFormatBC2_RGBA_sRGB: case WMTPixelFormatBC3_RGBA:
	case WMTPixelFormatBC3_RGBA_sRGB: case WMTPixelFormatBC5_RGUnorm: case WMTPixelFormatBC5_RGSnorm:
	case WMTPixelFormatBC6H_RGBFloat: case WMTPixelFormatBC6H_RGBUfloat: case WMTPixelFormatBC7_RGBAUnorm:
	case WMTPixelFormatBC7_RGBAUnorm_sRGB:
		return 16;
	default:
		return 0;
	}
}

// The levels of a compressed texture that are a whole block or more each
// way: with a level of 2 x 2 or 1 x 1 the device fails the command buffer
// (scenes bc1-levels, bc1-levels-shared), so the device's texture stops
// before them. Copies into the smaller levels are dropped (translate.c);
// a sampler that reaches past the last level reads the last.
static uint32_t whole_block_levels(uint32_t width, uint32_t height, uint32_t levels)
{
	uint32_t n = 0;

	while (n < levels && (width >> n) >= 4 && (height >> n) >= 4)
		n++;
	return n;
}

// The texture the device makes for DXMT's description: 2D, a 2D array, a
// cube or a 3D texture, each with levels (harness/kinds.sh), or 2D
// multisample of 2, 4 or 8 samples and one level (scenes msaa-small-clear,
// msaa-small). A cube array, a multisample array and 1D textures (DXMT
// makes none) are refused. False with the reason logged.
static bool device_texture(const struct WMTTextureInfo *info, struct pvg_wire_texture *texture)
{
	bool multisample = info->type == WMTTextureType2DMultisample && info->mipmap_level_count <= 1 &&
			   (info->sample_count == 2 || info->sample_count == 4 || info->sample_count == 8);
	bool kind = info->type == WMTTextureType2D || info->type == WMTTextureType2DArray ||
		    info->type == WMTTextureTypeCube || info->type == WMTTextureType3D;
	// RUXN_NO_TEXTURE_KINDS in a process's environment keeps it to 2D, as
	// before 0.3.63: one new device behaviour a guest run (BC, then kinds).
	static int plain_only = -1;

	if (plain_only < 0)
		plain_only = GetEnvironmentVariableA("RUXN_NO_TEXTURE_KINDS", NULL, 0) != 0;
	if (plain_only && info->type != WMTTextureType2D)
		kind = false;

	if ((!kind || info->sample_count > 1) && !multisample) {
		pvg_log("FAILED: newTexture of type %u, %u samples, %u slices: 2D, 2D arrays, cubes and 3D "
			"textures, and 2D multisample ones of 2, 4 or 8 samples", (unsigned)info->type,
			(unsigned)info->sample_count, info->array_length);
		return false;
	}
	pvg_wire_texture_init(texture);
	texture->type = info->type;
	texture->format = wm_depth_format(info->pixel_format);
	texture->width = info->width;
	texture->height = info->height;
	texture->depth = info->type == WMTTextureType3D && info->depth ? info->depth : 1;
	texture->array_length = info->type == WMTTextureType2DArray && info->array_length ? info->array_length : 1;
	texture->levels = info->mipmap_level_count ? info->mipmap_level_count : 1;
	texture->usage = info->usage & (WMTTextureUsageShaderRead | WMTTextureUsageShaderWrite |
					WMTTextureUsageRenderTarget | WMTTextureUsagePixelFormatView);
	if (info->options & 0xf0)
		note_once("a texture's storage mode: kept shared, as pvgtri's (its content is the host's)");
	// Depth and stencil are private, as the device took them in the
	// scenes (depth, stencil-test); nothing reads them back. So are the
	// kinds and compressed formats, as the scenes that held them made them.
	if (is_depth_or_stencil(texture->format) || info->type != WMTTextureType2D ||
	    block_bytes(info->pixel_format))
		texture->storage = PVG_STORAGE_PRIVATE;
	// A multisample texture lives on the GPU alone, as Metal has it.
	if (multisample) {
		texture->samples = info->sample_count;
		texture->storage = PVG_STORAGE_PRIVATE;
	}
	if (block_bytes(info->pixel_format)) {
		uint32_t levels = whole_block_levels(info->width, info->height, texture->levels);

		if (!levels || info->type == WMTTextureType3D) {
			pvg_log("FAILED: newTexture %ux%u of compressed format %u, type %u: smaller than a block, "
				"or 3D", info->width, info->height, (unsigned)info->pixel_format, (unsigned)info->type);
			return false;
		}
		if (levels < texture->levels)
			note_once("a compressed texture's levels smaller than a block: left out on the device");
		texture->levels = levels;
	}
	return true;
}

// The description and the bytes of the task's memory of a texture.
static size_t describe(UCHAR *description, size_t size, uint32_t number, const struct pvg_wire_texture *texture,
		       uint32_t format, uint64_t *bytes)
{
	uint32_t block = block_bytes(format), bpp = bytes_per_pixel(texture->format);

	if (block) {
		*bytes = pvg_wire_compressed_texture_bytes(texture, 4, 4, block);
		return pvg_wire_compressed_texture(description, size, number, texture, 0, 4, 4, block);
	}
	*bytes = bpp ? pvg_wire_texture_bytes(texture, bpp) : 0;
	return bpp ? pvg_wire_texture(description, size, number, texture, 0, bpp) : 0;
}

// The stencil half of a depth-and-stencil texture: its own object of the
// same type, size and slices, a byte a pixel, private. Under winemetal's lock.
static bool stencil_half(const struct pvg_wire_texture *depth, uint32_t *number, struct memory *m)
{
	static UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	struct pvg_wire_texture texture = *depth;
	uint64_t bytes = 0;
	size_t length;

	texture.format = WMTPixelFormatStencil8;
	texture.usage = depth->usage & (WMTTextureUsageShaderRead | WMTTextureUsageRenderTarget);
	texture.storage = PVG_STORAGE_PRIVATE;
	*number = wm_take_number();
	length = describe(description, sizeof(description), *number, &texture, texture.format, &bytes);
	return *number && length && allocate(bytes, WM_USE_TEXTURE, m) && texture_memory_zero(m) &&
	       create_object(*number, RUXNPVG_OBJECT_TEXTURE, m, 0, description + PVG_WIRE_HEAD_TEXTURE,
			     (ULONG)(length - PVG_WIRE_HEAD_TEXTURE));
}

// The user-mode driver may have asked that this one lie over a device
// surface (wm_surface.c). The host gives such a texture no placement; its
// resource id is a view's. Under winemetal's lock.
static obj_handle_t surface_texture(struct WMTTextureInfo *info)
{
	uint64_t id = 0;
	uint32_t view = 0, number;
	ULONG handle = 0;
	obj_handle_t h = 0;

	number = wm_surface_texture(info, &view, &id, &handle);
	if (number) {
		h = new_object(K_TEXTURE);
		if (h) {
			objects[h].u.texture.surface_handle = handle;
			objects[h].u.texture.number = number;
			objects[h].u.texture.view_number = view;
			objects[h].u.texture.resource_id = id;
			objects[h].u.texture.info = *info;
			objects[h].u.texture.surface = true;
			info->gpu_resource_id = id;
		}
	}
	if (!h)
		pvg_log("FAILED: newTexture %ux%u over a surface", info->width, info->height);
	return h;
}

obj_handle_t MTLDevice_newTexture(obj_handle_t dev, struct WMTTextureInfo *info)
{
	static UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	struct pvg_wire_texture texture;
	struct memory m;
	obj_handle_t h = 0;
	bool split = info->pixel_format == WMTPixelFormatDepth32Float_Stencil8;
	uint64_t bytes = 0;
	uint32_t number;
	size_t length;

	CALLED();
	(void)dev;
	if (!device_texture(info, &texture))
		return 0;
	EnterCriticalSection(&lock);
	if (wm_surface_wanted(info)) {
		h = surface_texture(info);
		LeaveCriticalSection(&lock);
		return h;
	}
	number = wm_take_number();
	length = describe(description, sizeof(description), number, &texture, info->pixel_format, &bytes);
	if (number && length && allocate(bytes, WM_USE_TEXTURE, &m) && texture_memory_zero(&m) &&
	    create_object(number, RUXNPVG_OBJECT_TEXTURE, &m, 0, description + PVG_WIRE_HEAD_TEXTURE,
			  (ULONG)(length - PVG_WIRE_HEAD_TEXTURE))) {
		uint64_t id = 0;

		// The resource id DXMT writes into its tables: the host's.
		if (!wm_place(number, WM_PLACE_LISTED, 0, &id))
			pvg_log("FAILED: the placement of texture %u", number);
		// Big textures said one by one, where their memory and number are
		// (memgrow, run rn77: at 243 MB a new texture read back an older
		// one's colour).
		static int every = -1;

		if (every < 0)
			every = GetEnvironmentVariableA("RUXN_LOG_TEXTURES", NULL, 0) != 0;
		if (every || m.length >= (1u << 20))
			pvg_log("texture %u: %ux%ux%u type %u, %u slices, %u levels, format %u, %llu bytes at chunk %u "
				"offset %#llx, resource id %llu", number, info->width, info->height, texture.depth,
				(unsigned)info->type, texture.array_length, texture.levels, (unsigned)info->pixel_format,
				(unsigned long long)m.length, wm_chunk_index(m.chunk), (unsigned long long)m.offset,
				(unsigned long long)id);
		h = new_object(K_TEXTURE);
		if (h) {
			objects[h].u.texture.memory = m;
			objects[h].u.texture.number = number;
			objects[h].u.texture.resource_id = id;
			objects[h].u.texture.info = *info;
			objects[h].u.texture.device_levels = (uint16_t)texture.levels;
			info->gpu_resource_id = id;
			if (split && !stencil_half(&texture, &objects[h].u.texture.stencil_number,
						   &objects[h].u.texture.stencil_memory))
				pvg_log("FAILED: the stencil half of a %ux%u depth-and-stencil texture", info->width,
					info->height);
			else if (split)
				note_once("a depth-and-stencil texture: made as a depth and a stencil texture");
		}
	}
	LeaveCriticalSection(&lock);
	if (!h)
		pvg_log("FAILED: newTexture %ux%u type %u format %u", info->width, info->height, (unsigned)info->type,
			(unsigned)info->pixel_format);
	else if (info->type != WMTTextureType2D && info->type != WMTTextureType2DMultisample)
		note_once("a 2D array, cube or 3D texture");
	if (h && block_bytes(info->pixel_format))
		note_once("a block-compressed texture");
	return h;
}

// A texture over a buffer's memory (kind 8, operation 9, as Metal's
// newTextureWithDescriptor:offset:bytesPerRow: makes): DirectWrite's glyphs and other typed buffer views
// are read through these. The buffer's bytes are the texture's, so nothing
// is copied. It lies over the buffer's chunk object at the buffer's place in
// it; command 67 lists that object, never the texture, and its resource id
// comes from naming it with the object listed (wm_place). It is deleted
// when its last reference goes (wm_objects.c).
obj_handle_t MTLBuffer_newTexture(obj_handle_t buffer, struct WMTTextureInfo *info, uint64_t offset,
				  uint64_t bytes_per_row)
{
	UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	struct pvg_wire_texture texture;
	struct object *b;
	obj_handle_t h = 0;
	uint32_t number = 0, chunk = 0;
	uint64_t id = 0;
	size_t length;

	CALLED();
	EnterCriticalSection(&lock);
	b = get(buffer, K_BUFFER);
	if (b) {
		chunk = b->u.buffer.number;
		pvg_wire_texture_init(&texture);
		texture.type = info->type;
		texture.format = info->pixel_format;
		texture.width = info->width;
		texture.height = info->height ? info->height : 1;
		texture.usage = info->usage & (WMTTextureUsageShaderRead | WMTTextureUsageShaderWrite |
					       WMTTextureUsageRenderTarget | WMTTextureUsagePixelFormatView);
		texture.storage = 0;	// shared, as the buffer
		number = wm_take_number();
		length = pvg_wire_texture_over_buffer(description, sizeof(description), number, chunk,
						      b->u.buffer.memory.offset + offset, bytes_per_row, &texture);
		if (number && length &&
		    wm_create_bare_object(number, RUXNPVG_OBJECT_TEXTURE_VIEW, 0, description, (ULONG)length)) {
			if (!wm_place(number, WM_PLACE_VIEW, chunk, &id))
				pvg_log("FAILED: the placement of texture %u over buffer object %u", number, chunk);
			h = new_object(K_TEXTURE);
			if (h) {
				objects[h].u.texture.number = number;
				objects[h].u.texture.listed = chunk;
				objects[h].u.texture.resource_id = id;
				objects[h].u.texture.info = *info;
				objects[h].u.texture.view = true;	// not a texture's own memory
				objects[h].u.texture.own_view = true;	// deleted with its handle
				// Held: the buffer's memory must not be given back
				// while the texture reads it.
				objects[h].u.texture.over = buffer;
				InterlockedIncrement(&objects[buffer].references);
				info->gpu_resource_id = id;
			}
		}
	}
	LeaveCriticalSection(&lock);
	if (!h)
		pvg_log("FAILED: newTexture over buffer %llu: format %u, %ux%u, type %u, offset %llu, %llu bytes a row",
			(unsigned long long)buffer, (unsigned)info->pixel_format, info->width, info->height,
			(unsigned)info->type, (unsigned long long)offset, (unsigned long long)bytes_per_row);
	else
		note_once("a texture over a buffer: kind 8, operation 9");
	return h;
}
