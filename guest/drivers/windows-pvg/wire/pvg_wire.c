/* SPDX-License-Identifier: MIT */
/*
 * See pvg_wire.h. Written from the device's command tables and
 * our own runs; each layout's source is named where it is written.
 */
#include "pvg_wire.h"

/* Declared here and not through <string.h>, so that this builds freestanding. */
void *memcpy(void *destination, const void *source, size_t length);
void *memset(void *destination, int value, size_t length);

static void
put16(uint8_t *at, uint32_t value)
{
   at[0] = (uint8_t)value;
   at[1] = (uint8_t)(value >> 8);
}

static void
put32(uint8_t *at, uint32_t value)
{
   put16(at, value);
   put16(at + 2, value >> 16);
}

static void
put64(uint8_t *at, uint64_t value)
{
   put32(at, (uint32_t)value);
   put32(at + 4, (uint32_t)(value >> 32));
}

static void
put_float(uint8_t *at, float value)
{
   uint32_t bits;

   memcpy(&bits, &value, sizeof bits);
   put32(at, bits);
}

static void
put_double(uint8_t *at, double value)
{
   uint64_t bits;

   memcpy(&bits, &value, sizeof bits);
   put64(at, bits);
}

static void
put64s(uint8_t *at, const uint64_t values[3])
{
   put64(at, values[0]);
   put64(at + 8, values[1]);
   put64(at + 16, values[2]);
}

void
pvg_wire_init(struct pvg_wire *wire, void *memory, size_t size)
{
   wire->bytes = memory;
   wire->size = size;
   wire->used = 0;
   wire->segment = 0;
   wire->open = false;
   wire->failed = false;
}

/* Room for `length` bytes, zeroed: what the serializer leaves alone is zero here. */
static uint8_t *
take(struct pvg_wire *wire, size_t length)
{
   uint8_t *at;

   if (wire->failed || length > wire->size - wire->used) {
      wire->failed = true;
      return NULL;
   }
   at = wire->bytes + wire->used;
   memset(at, 0, length);
   wire->used += length;
   return at;
}

void
pvg_wire_object_entry(void *entry, enum pvg_wire_kind kind, uint32_t length, uint64_t address)
{
   uint8_t *at = entry;

   /* One byte of kind, three of length, then where the description lies. */
   put32(at, (uint32_t)kind | length << 8);
   put64(at + 4, address);
}

void
pvg_wire_resource_entry(void *entry, uint32_t object, uint32_t flags)
{
   uint8_t *at = entry;

   put32(at, object);
   put32(at + 4, flags);
}

void
pvg_wire_command_buffer_entry(void *entry, uint64_t address, uint64_t length)
{
   uint8_t *at = entry;

   put64(at, address);
   put64(at + 8, length);
}

/*
 * The serializer's operations. Each starts as a command does, with its
 * number and its length, and then has the new object's number.
 */
enum {
   OP_TEXTURE = 1,
   OP_SAMPLER = 3,
   OP_DEPTH_STENCIL = 4,
   OP_TEXTURE_VIEW = 7,
   OP_TEXTURE_VIEW_RANGE = 8,
   OP_TEXTURE_VIEW_SWIZZLE = 27,
   OP_COMPUTE_PIPELINE = 11,
   OP_SURFACE_TEXTURE = 12,
   OP_RENDER_PIPELINE = 14,
   OP_FUNCTION = 15,
};

static void
operation(uint8_t *at, uint32_t op, uint32_t length, uint32_t number)
{
   put32(at, op);
   put32(at + 4, length);
   put32(at + 8, number);
}

size_t
pvg_wire_buffer(void *out, size_t size, uint64_t length, uint32_t page, bool private_storage)
{
   uint8_t *at = out;

   if (size < 16)
      return 0;
   memset(at, 0, 16);
   put64(at, length);
   put32(at + 8, page);
   at[12] = private_storage ? 2 : 0;
   return 16;
}

void
pvg_wire_texture_init(struct pvg_wire_texture *texture)
{
   memset(texture, 0, sizeof *texture);
   texture->type = 2;
   texture->width = texture->height = texture->depth = 1;
   texture->levels = texture->samples = texture->array_length = 1;
   texture->usage = 1;
   texture->gpu_optimized = true;
}

/* Metal's MTLTextureType. */
enum {
   TYPE_1D = 0,
   TYPE_1D_ARRAY = 1,
   TYPE_2D = 2,
   TYPE_2D_ARRAY = 3,
   TYPE_2D_MULTISAMPLE = 4,
   TYPE_CUBE = 5,
   TYPE_CUBE_ARRAY = 6,
   TYPE_3D = 7,
   TYPE_2D_MULTISAMPLE_ARRAY = 8,
};

static bool
texture_is_cube(const struct pvg_wire_texture *texture)
{
   return texture->type == TYPE_CUBE || texture->type == TYPE_CUBE_ARRAY;
}

/*
 * Whether the description of kind 2 is written for this texture. The forms
 * are the shared base's: an array's
 * elements, a cube's faces and a 3D texture's planes laid out as below. A
 * form is written here only once a helper scene on this Mac has held it
 * (README.md); the others only with PVG_WIRE_LEADS, for the scenes that send
 * them first.
 */
static bool
texture_is_written(const struct pvg_wire_texture *texture)
{
   bool multisample = texture->type == TYPE_2D_MULTISAMPLE || texture->type == TYPE_2D_MULTISAMPLE_ARRAY;
   bool held;

   if (!texture->width || !texture->height || !texture->depth || !texture->array_length || texture->levels < 1 ||
       texture->levels > 15 || (multisample ? texture->samples < 2 || texture->levels != 1 : texture->samples != 1))
      return false;
   switch (texture->type) {
   case TYPE_1D:
   case TYPE_1D_ARRAY:
      /* Metal has no levels for these. */
      if (texture->height != 1 || texture->depth != 1 || texture->levels != 1)
         return false;
      break;
   case TYPE_CUBE:
   case TYPE_CUBE_ARRAY:
      if (texture->width != texture->height)
         return false;
      /* fallthrough */
   case TYPE_2D:
   case TYPE_2D_ARRAY:
   case TYPE_2D_MULTISAMPLE:
   case TYPE_2D_MULTISAMPLE_ARRAY:
      if (texture->depth != 1)
         return false;
      break;
   case TYPE_3D:
      break;
   default:
      return false;
   }
   if ((texture->type == TYPE_1D || texture->type == TYPE_2D || texture->type == TYPE_2D_MULTISAMPLE ||
        texture->type == TYPE_CUBE || texture->type == TYPE_3D) && texture->array_length != 1)
      return false;
   /*
    * Held here: a 2D texture, a 2D multisample one of 2, 4 or 8 samples
    * (scenes msaa-small-clear, msaa-small), and with levels a 2D array, a
    * cube and a 3D texture (harness/kinds.sh: their slices and planes
    * loaded, copied out, cleared by a pass and written back, sampled as
    * what they are and through views). A cube array of kind 2 is not sent:
    * the contributor saw its second cube sampled as its first; a view of
    * type cube array over a 2D array samples right (array-cube-array-view).
    */
   held = texture->type == TYPE_2D || texture->type == TYPE_2D_ARRAY || texture->type == TYPE_CUBE ||
          texture->type == TYPE_3D ||
          (texture->type == TYPE_2D_MULTISAMPLE && (texture->samples == 2 || texture->samples == 4 || texture->samples == 8));
#ifdef PVG_WIRE_LEADS
   held = true;
#endif
   return held;
}

static uint32_t
level_size(uint32_t size, uint32_t level)
{
   return size >> level ? size >> level : 1;
}

/* A level's depth: a 3D texture's halves with each level, every other texture's is 1. */
static uint32_t
level_depth(const struct pvg_wire_texture *texture, uint32_t level)
{
   return texture->type == TYPE_3D ? level_size(texture->depth, level) : 1;
}

/* A pixel (1 x 1) or a compressed format's block, and its bytes. */
struct block {
   uint32_t width, height, bytes;
};

static uint64_t
blocks(uint32_t pixels, uint32_t block)
{
   return ((uint64_t)pixels + block - 1) / block;
}

/* A row's bytes at a level: of blocks for a compressed format; a multisample pixel's samples lie side by side. */
static uint64_t
level_row(const struct pvg_wire_texture *texture, uint32_t level, struct block block)
{
   return blocks(level_size(texture->width, level), block.width) * block.bytes *
          (texture->samples ? texture->samples : 1);
}

static uint64_t
level_bytes(const struct pvg_wire_texture *texture, uint32_t level, struct block block)
{
   return level_row(texture, level, block) * blocks(level_size(texture->height, level), block.height) *
          level_depth(texture, level);
}

/* How many slices: an array's length, six faces to a cube. */
static uint64_t
texture_slices(const struct pvg_wire_texture *texture)
{
   return (uint64_t)(texture->array_length ? texture->array_length : 1) * (texture_is_cube(texture) ? 6 : 1);
}

/* The serializer's texture descriptor, 32 bytes. */
static void
texture_descriptor(uint8_t *at, const struct pvg_wire_texture *texture)
{
   at[0] = (uint8_t)((texture->type & 15) | (texture->gpu_optimized ? 1 << 6 : 0));
   at[1] = (uint8_t)texture->usage;
   put16(at + 2, texture->format);
   put32(at + 4, texture->width);
   put32(at + 8, texture->height);
   put32(at + 12, texture->depth);
   put16(at + 16, texture->levels);
   put16(at + 18, texture->samples);
   put16(at + 20, texture->array_length);
   /* The resource options: the storage mode from bit 4. */
   put16(at + 22, (texture->storage & 15) << 4);
}

static uint64_t
slice_bytes(const struct pvg_wire_texture *texture, struct block block)
{
   uint64_t bytes = 0;

   for (uint32_t level = 0; level < texture->levels; ++level)
      bytes += level_bytes(texture, level, block);
   return bytes;
}

static uint64_t
texture_bytes(const struct pvg_wire_texture *texture, struct block block)
{
   uint64_t bytes = texture_slices(texture) * slice_bytes(texture, block);

   return (bytes + PVG_WIRE_PAGE - 1) & ~(uint64_t)(PVG_WIRE_PAGE - 1);
}

uint64_t
pvg_wire_texture_slice_bytes(const struct pvg_wire_texture *texture, uint32_t bytes_per_pixel)
{
   return slice_bytes(texture, (struct block){ 1, 1, bytes_per_pixel });
}

uint64_t
pvg_wire_texture_bytes(const struct pvg_wire_texture *texture, uint32_t bytes_per_pixel)
{
   return texture_bytes(texture, (struct block){ 1, 1, bytes_per_pixel });
}

uint64_t
pvg_wire_compressed_texture_slice_bytes(const struct pvg_wire_texture *texture, uint32_t block_width,
                                        uint32_t block_height, uint32_t block_bytes)
{
   return block_width && block_height ? slice_bytes(texture, (struct block){ block_width, block_height, block_bytes })
                                      : 0;
}

uint64_t
pvg_wire_compressed_texture_bytes(const struct pvg_wire_texture *texture, uint32_t block_width, uint32_t block_height,
                                  uint32_t block_bytes)
{
   return block_width && block_height ? texture_bytes(texture, (struct block){ block_width, block_height, block_bytes })
                                      : 0;
}

static size_t
texture_description(uint8_t *at, size_t size, uint32_t number, const struct pvg_wire_texture *texture, uint32_t page,
                    struct block block)
{
   bool compressed = block.width != 1 || block.height != 1;
   size_t length;
   uint64_t offset = 0;
   uint8_t *level_at;

   if (!texture_is_written(texture) || !block.bytes || block.bytes > 255 || !block.width || block.width > 255 ||
       !block.height || block.height > 255)
      return 0;
   length = 12 + 24 + 36 * (size_t)texture->levels + 44;
   if (size < length)
      return 0;
   memset(at, 0, length);
   /* Where the content lies in the task: its length in whole pages, and its first page. */
   put64(at, texture_bytes(texture, block));
   put32(at + 8, page);
   /*
    * The whole texture: the number of levels in 14 bits, a bit for a cube
    * and one for a compressed format, the array length as Metal counts it,
    * where the first slice starts, a slice's size, a block's width, height
    * and depth (a pixel for a format that is not compressed), and a pixel's
    * or a block's bytes. The slices lie one after the other, a slice's size
    * apart: an array's elements, a cube's six faces, a cube array's faces
    * element by element.
    */
   put16(at + 12, texture->levels | (texture_is_cube(texture) ? 1u << 14 : 0) | (compressed ? 1u << 15 : 0));
   put16(at + 14, texture->array_length);
   at[32] = (uint8_t)block.width;
   at[33] = (uint8_t)block.height;
   at[34] = 1;
   at[35] = (uint8_t)block.bytes;
   /*
    * Each level: its place in the slice, its size, a row's length (of
    * blocks, for a compressed format), and its width, height and depth in
    * pixels. A 3D texture's depth planes lie in its level one after the
    * other.
    */
   level_at = at + 36;
   for (uint32_t level = 0; level < texture->levels; ++level, level_at += 36) {
      uint64_t bytes = level_bytes(texture, level, block);

      put64(level_at, offset);
      put64(level_at + 8, bytes);
      put64(level_at + 16, level_row(texture, level, block));
      put32(level_at + 24, level_size(texture->width, level));
      put32(level_at + 28, level_size(texture->height, level));
      put32(level_at + 32, level_depth(texture, level));
      offset += bytes;
   }
   put64(at + 24, offset);
   operation(level_at, OP_TEXTURE, 44, number);
   texture_descriptor(level_at + 12, texture);
   return length;
}

size_t
pvg_wire_texture(void *out, size_t size, uint32_t number, const struct pvg_wire_texture *texture, uint32_t page,
                 uint32_t bytes_per_pixel)
{
   return texture_description(out, size, number, texture, page, (struct block){ 1, 1, bytes_per_pixel });
}

/*
 * A compressed texture: blocks of 4 x 4 of 8 or 16 bytes, every BC format
 * (130 to 153) drawn through the helper here exactly as Metal on the Mac
 * draws the same bytes (harness/kinds.sh, scenes bc1, bc3, bc-N). 2D, a 2D
 * array or a cube, with levels as long as the last is a whole block or
 * more each way: with a level of 2 x 2 or 1 x 1 the command buffer fails
 * (scenes bc1-levels, bc1-levels-shared, refused), though the framework's
 * check of the dimension takes it. Other forms only with PVG_WIRE_LEADS.
 */
size_t
pvg_wire_compressed_texture(void *out, size_t size, uint32_t number, const struct pvg_wire_texture *texture,
                            uint32_t page, uint32_t block_width, uint32_t block_height, uint32_t block_bytes)
{
   uint32_t last = texture->levels ? texture->levels - 1 : 0;
   bool held = (texture->type == TYPE_2D || texture->type == TYPE_2D_ARRAY || texture->type == TYPE_CUBE) &&
               texture->samples == 1 && block_width == 4 && block_height == 4 &&
               (block_bytes == 8 || block_bytes == 16) && level_size(texture->width, last) >= 4 &&
               level_size(texture->height, last) >= 4;

#ifdef PVG_WIRE_LEADS
   held = texture->samples == 1;
#endif
   if (!held || (block_width == 1 && block_height == 1))
      return 0;
   return texture_description(out, size, number, texture, page, (struct block){ block_width, block_height, block_bytes });
}

size_t
pvg_wire_surface_texture(void *out, size_t size, uint32_t number, const struct pvg_wire_texture *texture,
                         uint64_t mapping, uint32_t plane)
{
   uint8_t *at = out;

   if (size < 56 || texture->type != 2)
      return 0;
   memset(at, 0, 56);
   put64(at, mapping);
   operation(at + 8, OP_SURFACE_TEXTURE, 48, number);
   texture_descriptor(at + 20, texture);
   put16(at + 52, plane);
   return 56;
}

size_t
pvg_wire_function(void *out, size_t size, uint32_t number, uint64_t address, uint32_t length)
{
   uint8_t *at = out;

   if (size < 32)
      return 0;
   memset(at, 0, 32);
   put64(at, address);
   put32(at + 8, length);
   /* The operation's last 8 bytes are left alone by the serializer. */
   operation(at + 12, OP_FUNCTION, 20, number);
   return 32;
}

void
pvg_wire_sampler_init(struct pvg_wire_sampler *sampler)
{
   memset(sampler, 0, sizeof *sampler);
   sampler->max_anisotropy = 1;
   sampler->normalized = true;
   sampler->lod_max = 3.40282347e+38f;
}

size_t
pvg_wire_sampler(void *out, size_t size, uint32_t number, const struct pvg_wire_sampler *sampler)
{
   uint8_t *at = out;

   if (size < 36)
      return 0;
   memset(at, 0, 36);
   operation(at, OP_SAMPLER, 36, number);
   at[12] = (uint8_t)((sampler->min_filter & 3) | (sampler->mag_filter & 3) << 2 | (sampler->lod_average ? 1 << 4 : 0) |
                      (sampler->compare & 7) << 5);
   at[13] = (uint8_t)((sampler->address_s & 15) | (sampler->address_t & 15) << 4);
   at[14] = (uint8_t)((sampler->address_r & 15) | (sampler->border_colour & 15) << 4);
   at[15] = (uint8_t)((sampler->mip_filter & 3) | (sampler->max_anisotropy & 31) << 2 |
                      (sampler->normalized ? 1 << 7 : 0));
   at[16] = sampler->argument_buffers ? 1 : 0;
   put_float(at + 20, sampler->lod_min);
   put_float(at + 24, sampler->lod_max);
   return 36;
}

void
pvg_wire_depth_stencil_init(struct pvg_wire_depth_stencil *state)
{
   memset(state, 0, sizeof *state);
   state->compare = 7;
   state->front.compare = state->back.compare = 7;
   state->front.read_mask = state->front.write_mask = 0xffffffff;
   state->back.read_mask = state->back.write_mask = 0xffffffff;
}

static void
stencil(uint8_t *at, const struct pvg_wire_stencil *face)
{
   put32(at, (face->compare & 7) | (face->stencil_fail & 7) << 3 | (face->depth_fail & 7) << 6 |
                (face->pass & 7) << 9);
   put32(at + 4, face->read_mask);
   put32(at + 8, face->write_mask);
}

size_t
pvg_wire_depth_stencil(void *out, size_t size, uint32_t number, const struct pvg_wire_depth_stencil *state)
{
   uint8_t *at = out;

   if (size < 40)
      return 0;
   memset(at, 0, 40);
   operation(at, OP_DEPTH_STENCIL, 40, number);
   at[12] = (uint8_t)((state->compare & 7) | (state->write ? 1 << 3 : 0));
   /* A face is written only when it is there. */
   if (state->has_front) {
      at[12] |= 1 << 4;
      stencil(at + 16, &state->front);
   }
   if (state->has_back) {
      at[12] |= 1 << 5;
      stencil(at + 28, &state->back);
   }
   return 40;
}

/*
 * A pipeline's serialized descriptor. It is a list of settings, each left
 * out while it has Metal's default: a byte that counts them, then for each a
 * key of one byte, a byte 4, and 32 bits, which are the value or, for a list
 * or a descriptor inside this one, where that lies, counted from the
 * settings' own first byte. What a setting points to follows the settings
 * in their order, a list or a descriptor on a multiple of four from the
 * whole descriptor's start.
 */
struct settings {
   struct pvg_wire *wire;
   size_t base;
   unsigned next;
};

static void
settings_open(struct settings *settings, struct pvg_wire *wire, unsigned count)
{
   uint8_t *at;

   settings->wire = wire;
   settings->base = wire->used;
   settings->next = 0;
   at = take(wire, 1 + 6 * (size_t)count);
   if (at)
      at[0] = (uint8_t)count;
}

/* Writes the next setting and returns its place, for settings_point. */
static unsigned
settings_put(struct settings *settings, unsigned key, uint32_t value)
{
   unsigned slot = settings->next++;

   if (!settings->wire->failed) {
      uint8_t *at = settings->wire->bytes + settings->base + 1 + 6 * (size_t)slot;

      at[0] = (uint8_t)key;
      at[1] = 4;
      put32(at + 2, value);
   }
   return slot;
}

/* Lets a setting point to what is written next, which starts on a multiple of four. */
static void
settings_point(struct settings *settings, unsigned slot)
{
   struct pvg_wire *wire = settings->wire;

   take(wire, -wire->used & 3);
   if (!wire->failed)
      put32(wire->bytes + settings->base + 1 + 6 * (size_t)slot + 2, (uint32_t)(wire->used - settings->base));
}

/*
 * A list: a count, then for each element where it lies from the list's
 * start. The elements follow one after the other with nothing between.
 */
static size_t
list_open(struct pvg_wire *wire, unsigned count)
{
   size_t base = wire->used;
   uint8_t *at = take(wire, 4 + 4 * (size_t)count);

   if (at)
      put32(at, count);
   return base;
}

static void
list_element(struct pvg_wire *wire, size_t base, unsigned index)
{
   if (!wire->failed)
      put32(wire->bytes + base + 4 + 4 * (size_t)index, (uint32_t)(wire->used - base));
}

enum {
   /* A render pipeline's settings, in the order they are written. */
   RENDER_VERTEX_DESCRIPTOR = 3,
   RENDER_RASTER_SAMPLE_COUNT = 4,
   RENDER_COLOURS = 8,
   RENDER_DEPTH_FORMAT = 9,
   RENDER_STENCIL_FORMAT = 10,
   RENDER_VERTEX_FUNCTION = 1,
   RENDER_FRAGMENT_FUNCTION = 2,
   /* A vertex descriptor's. */
   VERTEX_ATTRIBUTES = 0,
   VERTEX_LAYOUTS = 1,
   /* An attribute's and a layout's. The first is its index in both. */
   ATTRIBUTE_FORMAT = 1,
   ATTRIBUTE_OFFSET = 2,
   ATTRIBUTE_BUFFER = 3,
   LAYOUT_STEP_FUNCTION = 1,
   LAYOUT_STEP_RATE = 2,
   LAYOUT_STRIDE = 3,
   /* A colour attachment's, after its index. */
   COLOUR_FORMAT = 1,
   COLOUR_BLENDING = 2,
   COLOUR_SOURCE_RGB = 3,
   COLOUR_DESTINATION_RGB = 4,
   COLOUR_RGB_OPERATION = 5,
   COLOUR_SOURCE_ALPHA = 6,
   COLOUR_DESTINATION_ALPHA = 7,
   COLOUR_ALPHA_OPERATION = 8,
   COLOUR_WRITE_MASK = 9,
   /* A compute pipeline's. */
   COMPUTE_FUNCTION = 0,
};

void
pvg_wire_render_pipeline_init(struct pvg_wire_render_pipeline *pipeline)
{
   memset(pipeline, 0, sizeof *pipeline);
   for (unsigned i = 0; i < PVG_WIRE_MOST_LAYOUTS; ++i)
      pipeline->layouts[i].step_function = pipeline->layouts[i].step_rate = 1;
   for (unsigned i = 0; i < PVG_WIRE_MOST_COLOURS; ++i) {
      pipeline->colours[i].source_rgb = pipeline->colours[i].source_alpha = 1;
      pipeline->colours[i].write_mask = 15;
   }
}

static void
vertex_descriptor(struct pvg_wire *wire, const struct pvg_wire_render_pipeline *pipeline, unsigned used)
{
   struct settings descriptor, element;
   unsigned attributes, layouts, at = 0;
   size_t list;

   settings_open(&descriptor, wire, 2);
   attributes = settings_put(&descriptor, VERTEX_ATTRIBUTES, 0);
   layouts = settings_put(&descriptor, VERTEX_LAYOUTS, 0);

   settings_point(&descriptor, attributes);
   list = list_open(wire, used);
   for (unsigned i = 0; i < PVG_WIRE_MOST_ATTRIBUTES; ++i) {
      const struct pvg_wire_attribute *attribute = &pipeline->attributes[i];

      if (!attribute->format)
         continue;
      list_element(wire, list, at++);
      settings_open(&element, wire, 2 + (attribute->offset != 0) + (attribute->buffer != 0));
      settings_put(&element, 0, i);
      settings_put(&element, ATTRIBUTE_FORMAT, attribute->format);
      if (attribute->offset)
         settings_put(&element, ATTRIBUTE_OFFSET, attribute->offset);
      if (attribute->buffer)
         settings_put(&element, ATTRIBUTE_BUFFER, attribute->buffer);
   }

   /* One layout for each attribute, that of the attribute's buffer, said again for every attribute that reads it. */
   settings_point(&descriptor, layouts);
   list = list_open(wire, used);
   at = 0;
   for (unsigned i = 0; i < PVG_WIRE_MOST_ATTRIBUTES; ++i) {
      const struct pvg_wire_attribute *attribute = &pipeline->attributes[i];
      const struct pvg_wire_layout *layout;

      if (!attribute->format)
         continue;
      layout = &pipeline->layouts[attribute->buffer < PVG_WIRE_MOST_LAYOUTS ? attribute->buffer : 0];
      list_element(wire, list, at++);
      settings_open(&element, wire,
                    1 + (layout->step_function != 1) + (layout->step_rate != 1) + (layout->stride != 0));
      settings_put(&element, 0, attribute->buffer);
      if (layout->step_function != 1)
         settings_put(&element, LAYOUT_STEP_FUNCTION, layout->step_function);
      if (layout->step_rate != 1)
         settings_put(&element, LAYOUT_STEP_RATE, layout->step_rate);
      if (layout->stride)
         settings_put(&element, LAYOUT_STRIDE, layout->stride);
   }
}

static bool
colour_is_set(const struct pvg_wire_colour *colour)
{
   return colour->format || colour->blending || colour->source_rgb != 1 || colour->destination_rgb ||
          colour->rgb_operation || colour->source_alpha != 1 || colour->destination_alpha ||
          colour->alpha_operation || colour->write_mask != 15;
}

static void
colour_attachments(struct pvg_wire *wire, const struct pvg_wire_render_pipeline *pipeline, unsigned used)
{
   size_t list = list_open(wire, used);
   unsigned at = 0;

   for (unsigned i = 0; i < PVG_WIRE_MOST_COLOURS; ++i) {
      const struct pvg_wire_colour *colour = &pipeline->colours[i];
      struct settings element;

      if (!colour_is_set(colour))
         continue;
      list_element(wire, list, at++);
      settings_open(&element, wire,
                    1 + (colour->format != 0) + colour->blending + (colour->source_rgb != 1) +
                       (colour->destination_rgb != 0) + (colour->rgb_operation != 0) + (colour->source_alpha != 1) +
                       (colour->destination_alpha != 0) + (colour->alpha_operation != 0) +
                       (colour->write_mask != 15));
      settings_put(&element, 0, i);
      if (colour->format)
         settings_put(&element, COLOUR_FORMAT, colour->format);
      if (colour->blending)
         settings_put(&element, COLOUR_BLENDING, 1);
      if (colour->source_rgb != 1)
         settings_put(&element, COLOUR_SOURCE_RGB, colour->source_rgb);
      if (colour->destination_rgb)
         settings_put(&element, COLOUR_DESTINATION_RGB, colour->destination_rgb);
      if (colour->rgb_operation)
         settings_put(&element, COLOUR_RGB_OPERATION, colour->rgb_operation);
      if (colour->source_alpha != 1)
         settings_put(&element, COLOUR_SOURCE_ALPHA, colour->source_alpha);
      if (colour->destination_alpha)
         settings_put(&element, COLOUR_DESTINATION_ALPHA, colour->destination_alpha);
      if (colour->alpha_operation)
         settings_put(&element, COLOUR_ALPHA_OPERATION, colour->alpha_operation);
      if (colour->write_mask != 15)
         settings_put(&element, COLOUR_WRITE_MASK, colour->write_mask);
   }
}

/*
 * The serializer's operation for a pipeline: its head, the descriptor's
 * length, the descriptor, and zeroes up to a multiple of four.
 */
static size_t
pipeline_end(struct pvg_wire *wire, uint32_t op, uint32_t number)
{
   uint32_t descriptor = (uint32_t)wire->used;

   take(wire, -wire->used & 3);
   if (wire->failed)
      return 0;
   operation(wire->bytes - 16, op, 16 + (uint32_t)wire->used, number);
   put32(wire->bytes - 4, descriptor);
   return 16 + wire->used;
}

size_t
pvg_wire_render_pipeline(void *out, size_t size, uint32_t number, const struct pvg_wire_render_pipeline *pipeline)
{
   struct pvg_wire wire;
   struct settings settings;
   unsigned attributes = 0, colours = 0, vertex_slot = 0, colour_slot = 0;

   if (size < 16)
      return 0;
   /* The descriptor's own bytes start behind the operation's head: places in it count from there. */
   pvg_wire_init(&wire, (uint8_t *)out + 16, size - 16);
   for (unsigned i = 0; i < PVG_WIRE_MOST_ATTRIBUTES; ++i)
      attributes += pipeline->attributes[i].format != 0;
   for (unsigned i = 0; i < PVG_WIRE_MOST_COLOURS; ++i)
      colours += colour_is_set(&pipeline->colours[i]);

   settings_open(&settings, &wire,
                 (attributes != 0) + (pipeline->raster_sample_count != 0) + (colours != 0) +
                    (pipeline->depth_format != 0) +
                    (pipeline->stencil_format != 0) + (pipeline->vertex_function != 0) +
                    (pipeline->fragment_function != 0));
   if (attributes)
      vertex_slot = settings_put(&settings, RENDER_VERTEX_DESCRIPTOR, 0);
   if (pipeline->raster_sample_count)
      settings_put(&settings, RENDER_RASTER_SAMPLE_COUNT, pipeline->raster_sample_count);
   if (colours)
      colour_slot = settings_put(&settings, RENDER_COLOURS, 0);
   if (pipeline->depth_format)
      settings_put(&settings, RENDER_DEPTH_FORMAT, pipeline->depth_format);
   if (pipeline->stencil_format)
      settings_put(&settings, RENDER_STENCIL_FORMAT, pipeline->stencil_format);
   if (pipeline->vertex_function)
      settings_put(&settings, RENDER_VERTEX_FUNCTION, pipeline->vertex_function);
   if (pipeline->fragment_function)
      settings_put(&settings, RENDER_FRAGMENT_FUNCTION, pipeline->fragment_function);
   if (attributes) {
      settings_point(&settings, vertex_slot);
      vertex_descriptor(&wire, pipeline, attributes);
   }
   if (colours) {
      settings_point(&settings, colour_slot);
      colour_attachments(&wire, pipeline, colours);
   }
   return pipeline_end(&wire, OP_RENDER_PIPELINE, number);
}

size_t
pvg_wire_compute_pipeline(void *out, size_t size, uint32_t number, uint32_t function)
{
   struct pvg_wire wire;
   struct settings settings;

   if (size < 16)
      return 0;
   pvg_wire_init(&wire, (uint8_t *)out + 16, size - 16);
   settings_open(&settings, &wire, 1);
   settings_put(&settings, COMPUTE_FUNCTION, function);
   return pipeline_end(&wire, OP_COMPUTE_PIPELINE, number);
}

/*
 * A segment: its length with the header counted, then a byte for its kind,
 * one that says it goes on from the segment before, and two more.
 */
enum { SEGMENT_RENDER = 0, SEGMENT_COMPUTE = 1, SEGMENT_BLIT = 2 };

static void
begin(struct pvg_wire *wire, unsigned kind)
{
   uint8_t *at;

   /* A segment begun inside another is the caller's mistake. */
   if (wire->open)
      wire->failed = true;
   wire->segment = wire->used;
   at = take(wire, 8);
   if (!at)
      return;
   at[4] = (uint8_t)kind;
   wire->open = true;
}

size_t
pvg_wire_end(struct pvg_wire *wire)
{
   size_t length = wire->used - wire->segment;

   if (wire->failed || !wire->open)
      return 0;
   put32(wire->bytes + wire->segment, (uint32_t)length);
   wire->open = false;
   return length;
}

/* A command: its number, its length with these two words counted, then its fields. */
static uint8_t *
command(struct pvg_wire *wire, uint32_t number, uint32_t length)
{
   uint8_t *at;

   if (!wire->open)
      wire->failed = true;
   at = take(wire, length);
   if (!at)
      return NULL;
   put32(at, number);
   put32(at + 4, length);
   return at;
}

/* A command of one object's number. */
static void
command_ref(struct pvg_wire *wire, uint32_t number, uint32_t object)
{
   uint8_t *at = command(wire, number, 12);

   if (at)
      put32(at + 8, object);
}

/*
 * The commands that bind. Each has a first index and a count, here one,
 * and then its entries: a buffer with an offset, or an object alone.
 */
static void
bind_buffer(struct pvg_wire *wire, uint32_t number, uint32_t index, uint32_t buffer, uint64_t offset)
{
   uint8_t *at = command(wire, number, 28);

   if (!at)
      return;
   put32(at + 8, index);
   put32(at + 12, 1);
   put32(at + 16, buffer);
   put64(at + 20, offset);
}

static void
bind_object(struct pvg_wire *wire, uint32_t number, uint32_t index, uint32_t object)
{
   uint8_t *at = command(wire, number, 20);

   if (!at)
      return;
   put32(at + 8, index);
   put32(at + 12, 1);
   put32(at + 16, object);
}

static void
bind_offset(struct pvg_wire *wire, uint32_t number, uint32_t index, uint64_t offset)
{
   uint8_t *at = command(wire, number, 20);

   if (!at)
      return;
   put32(at + 8, index);
   put64(at + 12, offset);
}

void
pvg_wire_pass_init(struct pvg_wire_pass *pass)
{
   memset(pass, 0, sizeof *pass);
   /* Metal's defaults: a colour attachment is stored, depth and stencil are cleared and not kept. */
   for (unsigned i = 0; i < PVG_WIRE_MOST_COLOURS; ++i) {
      pass->colours[i].store = 1;
      pass->colours[i].clear[3] = 1.0;
   }
   pass->depth.load = pass->stencil.load = 2;
   pass->depth.clear_depth = 1.0;
}

/* What every attachment starts with, 28 bytes. */
static void
attachment(uint8_t *at, const struct pvg_wire_attachment *attachment)
{
   put32(at, attachment->texture);
   put32(at + 4, attachment->resolve_texture);
   put16(at + 8, attachment->level);
   put16(at + 10, attachment->slice);
   put16(at + 12, attachment->depth_plane);
   put16(at + 14, attachment->resolve_level);
   put16(at + 16, attachment->resolve_slice);
   put16(at + 18, attachment->resolve_depth_plane);
   put16(at + 20, attachment->load);
   put16(at + 22, attachment->store);
   put32(at + 24, attachment->store_options);
}

void
pvg_wire_begin_render(struct pvg_wire *wire, const struct pvg_wire_pass *pass)
{
   uint8_t *at;

   begin(wire, SEGMENT_RENDER);
   /* Render command 26, the whole pass: depth, stencil, eight colour attachments, and its size. */
   at = command(wire, 26, 592);
   if (!at)
      return;
   attachment(at + 8, &pass->depth);
   put_double(at + 8 + 28, pass->depth.clear_depth);
   put32(at + 8 + 36, pass->depth.resolve_filter);
   attachment(at + 48, &pass->stencil);
   put32(at + 48 + 28, pass->stencil.clear_stencil);
   put32(at + 48 + 32, pass->stencil.resolve_filter);
   for (unsigned i = 0; i < PVG_WIRE_MOST_COLOURS; ++i) {
      uint8_t *colour = at + 84 + 60 * i;

      attachment(colour, &pass->colours[i]);
      for (unsigned c = 0; c < 4; ++c)
         put_double(colour + 28 + 8 * c, pass->colours[i].clear[c]);
   }
   put32(at + 564, pass->visibility_buffer);
   put64(at + 568, pass->array_length);
   put64(at + 576, pass->width);
   put64(at + 584, pass->height);
}

void
pvg_wire_begin_compute(struct pvg_wire *wire)
{
   begin(wire, SEGMENT_COMPUTE);
}

void
pvg_wire_begin_blit(struct pvg_wire *wire)
{
   begin(wire, SEGMENT_BLIT);
}

void
pvg_wire_set_render_pipeline(struct pvg_wire *wire, uint32_t pipeline)
{
   command_ref(wire, 116, pipeline);
}

void
pvg_wire_set_depth_stencil(struct pvg_wire *wire, uint32_t state)
{
   command_ref(wire, 104, state);
}

void
pvg_wire_set_vertex_buffer(struct pvg_wire *wire, uint32_t index, uint32_t buffer, uint64_t offset)
{
   bind_buffer(wire, 125, index, buffer, offset);
}

void
pvg_wire_set_fragment_buffer(struct pvg_wire *wire, uint32_t index, uint32_t buffer, uint64_t offset)
{
   bind_buffer(wire, 110, index, buffer, offset);
}

void
pvg_wire_set_fragment_texture(struct pvg_wire *wire, uint32_t index, uint32_t texture)
{
   bind_object(wire, 114, index, texture);
}

void
pvg_wire_set_fragment_sampler(struct pvg_wire *wire, uint32_t index, uint32_t sampler)
{
   bind_object(wire, 112, index, sampler);
}

/* The short commands hold their numbers in 16 bits. */
static bool
is_short(uint64_t value)
{
   return value < 65536;
}

void
pvg_wire_draw(struct pvg_wire *wire, uint32_t primitive, uint64_t start, uint64_t count)
{
   uint8_t *at;

   if (is_short(start) && is_short(count)) {
      at = command(wire, 1, 16);
      if (!at)
         return;
      put16(at + 8, primitive);
      put16(at + 12, (uint32_t)start);
      put16(at + 14, (uint32_t)count);
   } else {
      at = command(wire, 0, 28);
      if (!at)
         return;
      put16(at + 8, primitive);
      put64(at + 12, start);
      put64(at + 20, count);
   }
}

void
pvg_wire_draw_indexed(struct pvg_wire *wire, uint32_t primitive, uint64_t count, uint32_t index_type,
                      uint32_t buffer, uint64_t offset)
{
   uint8_t *at;

   if (is_short(count) && is_short(offset)) {
      at = command(wire, 7, 20);
      if (!at)
         return;
      put16(at + 16, (uint32_t)count);
      put16(at + 18, (uint32_t)offset);
   } else {
      at = command(wire, 6, 32);
      if (!at)
         return;
      put64(at + 16, count);
      put64(at + 24, offset);
   }
   put16(at + 8, primitive);
   put16(at + 10, index_type);
   put32(at + 12, buffer);
}

void
pvg_wire_set_compute_pipeline(struct pvg_wire *wire, uint32_t pipeline)
{
   command_ref(wire, 208, pipeline);
}

void
pvg_wire_set_buffer(struct pvg_wire *wire, uint32_t index, uint32_t buffer, uint64_t offset)
{
   bind_buffer(wire, 203, index, buffer, offset);
}

void
pvg_wire_set_buffer_offset(struct pvg_wire *wire, uint32_t index, uint64_t offset)
{
   bind_offset(wire, 207, index, offset);
}

void
pvg_wire_set_texture(struct pvg_wire *wire, uint32_t index, uint32_t texture)
{
   bind_object(wire, 206, index, texture);
}

void
pvg_wire_set_sampler(struct pvg_wire *wire, uint32_t index, uint32_t sampler)
{
   bind_object(wire, 204, index, sampler);
}

void
pvg_wire_dispatch(struct pvg_wire *wire, const uint64_t groups[3], const uint64_t threads_per_group[3])
{
   uint8_t *at = command(wire, 200, 56);

   if (!at)
      return;
   put64s(at + 8, groups);
   put64s(at + 32, threads_per_group);
}

void
pvg_wire_set_stage_in_region(struct pvg_wire *wire, const uint64_t origin[3], const uint64_t size[3])
{
   uint8_t *at = command(wire, 209, 56);

   if (!at)
      return;
   /* The size stands before the origin. */
   put64s(at + 8, size);
   put64s(at + 32, origin);
}

void
pvg_wire_set_threadgroup_memory_length(struct pvg_wire *wire, uint32_t index, uint64_t length)
{
   uint8_t *at = command(wire, 211, 20);

   if (!at)
      return;
   put64(at + 8, length);
   put32(at + 16, index);
}

void
pvg_wire_memory_barrier(struct pvg_wire *wire, uint32_t scope)
{
   uint8_t *at = command(wire, 215, 12);

   if (at)
      put32(at + 8, scope);
}

void
pvg_wire_copy_buffer(struct pvg_wire *wire, uint32_t source, uint64_t source_offset, uint32_t destination,
                     uint64_t destination_offset, uint64_t length)
{
   uint8_t *at = command(wire, 301, 40);

   if (!at)
      return;
   put32(at + 8, source);
   put32(at + 12, destination);
   put64(at + 16, source_offset);
   put64(at + 24, destination_offset);
   put64(at + 32, length);
}

void
pvg_wire_copy_buffer_to_texture(struct pvg_wire *wire, uint32_t buffer, uint64_t offset, uint64_t bytes_per_row,
                                uint64_t bytes_per_image, const uint64_t size[3], uint32_t texture, uint32_t slice,
                                uint32_t level, const uint64_t origin[3])
{
   uint8_t *at = command(wire, 300, 96);

   if (!at)
      return;
   put32(at + 8, buffer);
   put32(at + 12, texture);
   put64(at + 16, offset);
   put64(at + 24, bytes_per_row);
   put64(at + 32, bytes_per_image);
   put64s(at + 40, size);
   put64s(at + 64, origin);
   put16(at + 88, slice);
   put16(at + 90, level);
}

void
pvg_wire_copy_texture_to_buffer(struct pvg_wire *wire, uint32_t texture, uint32_t slice, uint32_t level,
                                const uint64_t origin[3], const uint64_t size[3], uint32_t buffer, uint64_t offset,
                                uint64_t bytes_per_row, uint64_t bytes_per_image)
{
   uint8_t *at = command(wire, 302, 96);

   if (!at)
      return;
   put32(at + 8, texture);
   put32(at + 12, buffer);
   put64s(at + 16, origin);
   put64s(at + 40, size);
   put64(at + 64, offset);
   put64(at + 72, bytes_per_row);
   put64(at + 80, bytes_per_image);
   put16(at + 88, slice);
   put16(at + 90, level);
}

void
pvg_wire_fill_buffer(struct pvg_wire *wire, uint32_t buffer, uint64_t offset, uint64_t length, uint8_t value)
{
   uint8_t *at = command(wire, 306, 32);

   if (!at)
      return;
   put32(at + 8, buffer);
   put64(at + 12, offset);
   put64(at + 20, length);
   at[28] = value;
}

void
pvg_wire_synchronize(struct pvg_wire *wire, uint32_t resource)
{
   command_ref(wire, 314, resource);
}

/* A command of one 64-bit number. */
static void
command_u64(struct pvg_wire *wire, uint32_t number, uint64_t value)
{
   uint8_t *at = command(wire, number, 16);

   if (at)
      put64(at + 8, value);
}

void
pvg_wire_set_vertex_buffer_offset(struct pvg_wire *wire, uint32_t index, uint64_t offset)
{
   bind_offset(wire, 126, index, offset);
}

void
pvg_wire_set_viewport(struct pvg_wire *wire, double x, double y, double width, double height, double near,
                      double far)
{
   const double fields[6] = { x, y, width, height, near, far };
   uint8_t *at = command(wire, 130, 56);

   if (!at)
      return;
   for (unsigned i = 0; i < 6; ++i)
      put_double(at + 8 + 8 * i, fields[i]);
}

void
pvg_wire_set_scissor(struct pvg_wire *wire, uint64_t x, uint64_t y, uint64_t width, uint64_t height)
{
   uint8_t *at = command(wire, 117, 40);

   if (!at)
      return;
   put64(at + 8, x);
   put64(at + 16, y);
   put64(at + 24, width);
   put64(at + 32, height);
}

void
pvg_wire_set_cull_mode(struct pvg_wire *wire, uint32_t mode)
{
   command_u64(wire, 107, mode);
}

void
pvg_wire_set_front_facing(struct pvg_wire *wire, uint32_t winding)
{
   command_u64(wire, 115, winding);
}

void
pvg_wire_copy_texture(struct pvg_wire *wire, uint32_t source, uint32_t source_slice, uint32_t source_level,
                      const uint64_t source_origin[3], const uint64_t size[3], uint32_t destination,
                      uint32_t destination_slice, uint32_t destination_level, const uint64_t destination_origin[3])
{
   uint8_t *at = command(wire, 303, 96);

   if (!at)
      return;
   put32(at + 8, source);
   put32(at + 12, destination);
   put64s(at + 16, source_origin);
   put64s(at + 40, size);
   put64s(at + 64, destination_origin);
   put16(at + 88, source_slice);
   put16(at + 90, source_level);
   put16(at + 92, destination_slice);
   put16(at + 94, destination_level);
}

void
pvg_wire_generate_mipmaps(struct pvg_wire *wire, uint32_t texture)
{
   command_ref(wire, 307, texture);
}

void
pvg_wire_use_resources(struct pvg_wire *wire, const uint32_t *resources, uint32_t count, uint32_t usage,
                       uint32_t stages)
{
   uint8_t *at = command(wire, 137, 16 + 4 * count);

   if (!at)
      return;
   put32(at + 8, count);
   put16(at + 12, usage);
   put16(at + 14, stages);
   for (uint32_t i = 0; i < count; ++i)
      put32(at + 16 + 4 * i, resources[i]);
}

size_t
pvg_wire_texture_view(void *out, size_t size, uint32_t number, uint32_t base, uint32_t format)
{
   uint8_t *at = out;

   if (size < 20)
      return 0;
   memset(at, 0, 20);
   operation(at, OP_TEXTURE_VIEW, 20, number);
   put32(at + 12, base);
   put16(at + 16, format);
   return 20;
}

void
pvg_wire_set_fragment_buffer_offset(struct pvg_wire *wire, uint32_t index, uint64_t offset)
{
   bind_offset(wire, 111, index, offset);
}

void
pvg_wire_set_blend_colour(struct pvg_wire *wire, float red, float green, float blue, float alpha)
{
   uint8_t *at = command(wire, 101, 24);

   if (!at)
      return;
   put_float(at + 8, red);
   put_float(at + 12, green);
   put_float(at + 16, blue);
   put_float(at + 20, alpha);
}

void
pvg_wire_set_stencil_reference(struct pvg_wire *wire, uint32_t front, uint32_t back)
{
   uint8_t *at = command(wire, 119, 16);

   if (!at)
      return;
   put32(at + 8, front);
   put32(at + 12, back);
}

void
pvg_wire_draw_instanced(struct pvg_wire *wire, uint32_t primitive, uint64_t start, uint64_t count,
                        uint64_t instances)
{
   uint8_t *at;

   if (is_short(start) && is_short(count) && is_short(instances)) {
      at = command(wire, 3, 16);
      if (!at)
         return;
      put16(at + 8, (uint32_t)start);
      put16(at + 10, (uint32_t)count);
      put16(at + 12, (uint32_t)instances);
      put16(at + 14, primitive);
   } else {
      at = command(wire, 2, 36);
      if (!at)
         return;
      put64(at + 8, start);
      put64(at + 16, count);
      put64(at + 24, instances);
      put16(at + 32, primitive);
   }
}

void
pvg_wire_draw_indexed_instanced(struct pvg_wire *wire, uint32_t primitive, uint64_t count, uint32_t index_type,
                                uint32_t buffer, uint64_t offset, uint64_t instances)
{
   uint8_t *at;

   if (is_short(count) && is_short(offset) && is_short(instances)) {
      at = command(wire, 9, 24);
      if (!at)
         return;
      put16(at + 16, (uint32_t)count);
      put16(at + 18, (uint32_t)offset);
      put16(at + 20, (uint32_t)instances);
   } else {
      at = command(wire, 8, 40);
      if (!at)
         return;
      put64(at + 16, count);
      put64(at + 24, offset);
      put64(at + 32, instances);
   }
   put16(at + 8, primitive);
   put16(at + 10, index_type);
   put32(at + 12, buffer);
}

void
pvg_wire_draw_indexed_base(struct pvg_wire *wire, uint32_t primitive, uint64_t count, uint32_t index_type,
                           uint32_t buffer, uint64_t offset, uint64_t instances, int64_t base_vertex,
                           uint64_t base_instance)
{
   uint8_t *at;

   /*
    * 28 and 29 are what the serializer writes for a reader of version 8 or
    * later. For an older one it writes 10 and 11, with a base vertex cut to
    * 16 bits whatever its sign.
    */
   if (is_short(count) && is_short(offset) && is_short(instances) && is_short(base_instance) &&
       base_vertex >= -32768 && base_vertex <= 32767) {
      at = command(wire, 29, 28);
      if (!at)
         return;
      put16(at + 16, (uint32_t)offset);
      put16(at + 18, (uint32_t)count);
      put16(at + 20, (uint32_t)instances);
      put16(at + 22, (uint32_t)base_vertex);
      put16(at + 24, (uint32_t)base_instance);
   } else {
      at = command(wire, 28, 56);
      if (!at)
         return;
      put64(at + 16, offset);
      put64(at + 24, count);
      put64(at + 32, instances);
      put64(at + 40, (uint64_t)base_vertex);
      put64(at + 48, base_instance);
   }
   put16(at + 8, primitive);
   put16(at + 10, index_type);
   put32(at + 12, buffer);
}

size_t
pvg_wire_texture_view_range(void *out, size_t size, uint32_t number, uint32_t base, uint32_t format, uint32_t type,
                            uint64_t first_level, uint64_t levels, uint64_t first_slice, uint64_t slices,
                            const uint8_t *swizzle)
{
   size_t length = swizzle ? 56 : 52;
   uint8_t *at = out;

   if (size < length)
      return 0;
   memset(at, 0, length);
   operation(at, swizzle ? OP_TEXTURE_VIEW_SWIZZLE : OP_TEXTURE_VIEW_RANGE, (uint32_t)length, number);
   put32(at + 12, base);
   put16(at + 16, format);
   put16(at + 18, type);
   put64(at + 20, first_level);
   put64(at + 28, levels);
   put64(at + 36, first_slice);
   put64(at + 44, slices);
   /*
    * Red, green, blue, alpha: what the serializer writes for a reader of
    * version 9 or later, and how the helper reads them. For an older reader
    * it writes green and blue the other way round.
    */
   if (swizzle)
      memcpy(at + 52, swizzle, 4);
   return length;
}
