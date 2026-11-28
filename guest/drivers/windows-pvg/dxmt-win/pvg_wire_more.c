/* SPDX-License-Identifier: MIT */
/*
 * See pvg_wire_more.h: a texture over a buffer's memory, and the compute
 * segment's use of a resource.
 */
#include "pvg_wire_more.h"

/* Declared here and not through <string.h>, as pvg_wire.c does. */
void *memset(void *destination, int value, size_t length);

enum { OVER_BUFFER_BYTES = 64, DESCRIPTOR_AT = 32 };

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

/* The texture descriptor, 32 bytes: the same as pvg_wire.c writes for a texture. */
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
   put16(at + 22, (texture->storage & 15) << 4);
}

size_t
pvg_wire_texture_over_buffer(void *out, size_t size, uint32_t number, uint32_t buffer, uint64_t offset,
                             uint64_t bytes_per_row, const struct pvg_wire_texture *texture)
{
   uint8_t *at = out;

   if (size < OVER_BUFFER_BYTES)
      return 0;
   memset(at, 0, OVER_BUFFER_BYTES);
   put32(at, PVG_WIRE_OP_TEXTURE_OVER_BUFFER);
   put32(at + 4, OVER_BUFFER_BYTES);
   put32(at + 8, number);
   put32(at + 12, buffer);
   put64(at + 16, offset);
   put64(at + 24, bytes_per_row);
   texture_descriptor(at + DESCRIPTOR_AT, texture);
   return OVER_BUFFER_BYTES;
}

enum { COMPUTE_USE_RESOURCE_BYTES = 20 };

void
pvg_wire_compute_use_resource(struct pvg_wire *wire, uint32_t object, uint32_t usage)
{
   uint8_t *at;

   /* As pvg_wire.c's command(): inside a segment, room or a failed writer. */
   if (!wire->open || wire->failed || COMPUTE_USE_RESOURCE_BYTES > wire->size - wire->used) {
      wire->failed = true;
      return;
   }
   at = wire->bytes + wire->used;
   memset(at, 0, COMPUTE_USE_RESOURCE_BYTES);
   wire->used += COMPUTE_USE_RESOURCE_BYTES;
   put32(at, PVG_WIRE_COMPUTE_USE_RESOURCE);
   put32(at + 4, COMPUTE_USE_RESOURCE_BYTES);
   put32(at + 8, 1);
   put32(at + 12, usage);
   put32(at + 16, object);
}
