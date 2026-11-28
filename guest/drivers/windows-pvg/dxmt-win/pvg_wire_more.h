/* SPDX-License-Identifier: MIT */
/*
 * Calls of the wire library that the pinned copy (win/ruxnpvg/pvg_wire.c)
 * does not have yet. Written in its style and with its types, so that they
 * can be lifted into the shared wire library unchanged: each adds, none
 * changes what an existing call writes.
 */
#ifndef PVG_WIRE_MORE_H
#define PVG_WIRE_MORE_H

#include "pvg_wire.h"

/*
 * A texture over a buffer's memory, kind 8 (PVG_WIRE_KIND_TEXTURE_VIEW),
 * operation 9, 64 bytes: the operation's head, the buffer's number, the
 * offset, the bytes a row and the texture descriptor. The texture reads and writes the
 * buffer's bytes from `offset` on, `bytes_per_row` apart, as Metal's
 * -[MTLBuffer newTextureWithDescriptor:offset:bytesPerRow:] does. It is no
 * resource: the buffer is listed in command 67, the texture stays out.
 *
 *   0  operation 9      4  length 64      8  the texture's number
 *  12  the buffer's number
 *  16  offset, 64 bits                   24  bytes per row, 64 bits
 *  32  the texture descriptor, 32 bytes, as pvg_wire_texture's
 */
#define PVG_WIRE_OP_TEXTURE_OVER_BUFFER 9
size_t pvg_wire_texture_over_buffer(void *out, size_t size, uint32_t number, uint32_t buffer, uint64_t offset,
                                    uint64_t bytes_per_row, const struct pvg_wire_texture *texture);

/*
 * useResource:usage: in a compute segment: command 135, 20 bytes, a count
 * of one, the usage (MTLResourceUsage) and the object's number. A shader
 * that reaches a buffer or texture by address or resource id needs it, as
 * render command 137 in a render segment.
 */
#define PVG_WIRE_COMPUTE_USE_RESOURCE 135
void pvg_wire_compute_use_resource(struct pvg_wire *wire, uint32_t object, uint32_t usage);

#endif
