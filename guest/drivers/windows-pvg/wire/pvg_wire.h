/* SPDX-License-Identifier: MIT */
/*
 * Writes what a guest sends to the Mac's paravirtual graphics device for
 * Metal's work: the segments of a command buffer with their commands, the
 * descriptions of a task's objects, and a pipeline's serialized descriptor.
 *
 * It writes bytes into memory the caller gives it and knows nothing of a
 * driver: no ring, no task, no pages. Every layout here is from the tables in
 * the device's command tables as they were seen on macOS
 * 27.2.1 build 26B131. README.md says for every call what it was held
 * against: the framework's serializer, and its helper process, which
 * has taken every call here.
 *
 * Numbers that Metal defines (pixel formats, primitive types, load actions
 * and so on) are passed through as Metal's public headers number them.
 *
 * All multi-byte numbers on the wire are little-endian. A 64-bit number or a
 * double is not aligned to 8 there.
 */
#ifndef PVG_WIRE_H
#define PVG_WIRE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The device's page, which a buffer's and a texture's memory is counted in. */
#define PVG_WIRE_PAGE 0x4000u

/*
 * A task's object list. One entry of 12 bytes for each object number, the
 * entry for number n at 12 times n; number 0 is no object. An entry names a
 * kind and where in the task the object's description lies. The placement
 * list has 16 bytes for each entry of the object list, written by the host.
 */
#define PVG_WIRE_OBJECT_ENTRY 12u
#define PVG_WIRE_PLACEMENT_ENTRY 16u

enum pvg_wire_kind {
   PVG_WIRE_KIND_BUFFER = 1,          /* pvg_wire_buffer */
   PVG_WIRE_KIND_TEXTURE = 2,         /* pvg_wire_texture */
   PVG_WIRE_KIND_FUNCTION = 6,        /* pvg_wire_function */
   PVG_WIRE_KIND_STATE = 7,           /* a sampler, a depth and stencil state, a pipeline */
   PVG_WIRE_KIND_TEXTURE_VIEW = 8,    /* pvg_wire_texture_view */
   PVG_WIRE_KIND_SURFACE_TEXTURE = 11 /* pvg_wire_surface_texture */
};

/* Writes the object list's entry for an object whose description of `length` bytes lies at `address` in the task. */
void pvg_wire_object_entry(void *entry, enum pvg_wire_kind kind, uint32_t length, uint64_t address);

/*
 * What command 67 carries after the task's number and its two counts: 8
 * bytes for each resource the command buffers use, then 16 for each command
 * buffer. A resource is a buffer or a texture. Functions, pipelines and
 * state objects are not resources and must not be listed. A resource the GPU
 * touches has to be listed.
 */
#define PVG_WIRE_RESOURCE_ENTRY 8u
#define PVG_WIRE_COMMAND_BUFFER_ENTRY 16u
#define PVG_WIRE_RESOURCE_WRITTEN 2u /* the command buffers write the resource */

void pvg_wire_resource_entry(void *entry, uint32_t object, uint32_t flags);
void pvg_wire_command_buffer_entry(void *entry, uint64_t address, uint64_t length);

/*
 * The descriptions of objects. Each call writes one description into `out`
 * and returns its length in bytes, or 0 when `size` is too small or the
 * arguments describe something this library does not write. The description
 * goes into the task's memory, and the object list's entry names its kind,
 * length and place.
 */

/* The longest description: a texture's with 15 levels. */
#define PVG_WIRE_MOST_DESCRIPTION 620u

/*
 * A description's first bytes say where the object's memory is: a buffer's
 * whole 16, a texture's length and page, a function's address and length, a
 * surface texture's mapping. A driver that writes those itself, from what it
 * knows of the memory, takes the description from there on: the rest names
 * no place in the task. A state object's and a view's have no such head.
 */
#define PVG_WIRE_HEAD_BUFFER 16u
#define PVG_WIRE_HEAD_TEXTURE 12u
#define PVG_WIRE_HEAD_FUNCTION 12u
#define PVG_WIRE_HEAD_SURFACE_TEXTURE 8u

/*
 * A buffer, kind 1: `length` bytes of the task's memory from page `page` on.
 * A private buffer's memory is the host's: the guest's is not read or
 * written.
 */
size_t pvg_wire_buffer(void *out, size_t size, uint64_t length, uint32_t page, bool private_storage);

/* A texture as Metal's descriptor has it. pvg_wire_texture_init sets Metal's defaults. */
struct pvg_wire_texture {
   uint32_t type;         /* MTLTextureType; 2 is 2D */
   uint32_t format;       /* MTLPixelFormat */
   uint32_t width, height, depth;
   uint32_t levels;       /* mipmap levels */
   uint32_t samples;
   uint32_t array_length;
   uint32_t usage;        /* MTLTextureUsage */
   uint32_t storage;      /* MTLStorageMode: 0 shared, 2 private */
   bool gpu_optimized;    /* allowGPUOptimizedContents */
};

void pvg_wire_texture_init(struct pvg_wire_texture *texture);

/*
 * How many bytes of the task's memory a texture's content takes, whole
 * pages: the levels one after the other, each row `width` pixels long.
 */
uint64_t pvg_wire_texture_bytes(const struct pvg_wire_texture *texture, uint32_t bytes_per_pixel);

/* A slice's bytes, not rounded: an array's element, a cube's face. Slices lie one after the other. */
uint64_t pvg_wire_texture_slice_bytes(const struct pvg_wire_texture *texture, uint32_t bytes_per_pixel);

/*
 * A texture, kind 2. Its content is the host's. The task's memory from page
 * `page` on is where it is loaded from at first use and written back to by
 * command 53 when a command buffer marked it as written; it has to be
 * pvg_wire_texture_bytes long. `number` is the object's number. Written for
 * the kinds README.md says a helper scene held; 0 for the others.
 */
size_t pvg_wire_texture(void *out, size_t size, uint32_t number, const struct pvg_wire_texture *texture,
                        uint32_t page, uint32_t bytes_per_pixel);

/*
 * A texture of a compressed format: the levels word's bit 15, a block of
 * `block_width` x `block_height` pixels of `block_bytes`, rows of blocks,
 * each level's size in pixels. Its content is
 * pvg_wire_compressed_texture_bytes long.
 */
size_t pvg_wire_compressed_texture(void *out, size_t size, uint32_t number, const struct pvg_wire_texture *texture,
                                   uint32_t page, uint32_t block_width, uint32_t block_height, uint32_t block_bytes);
uint64_t pvg_wire_compressed_texture_bytes(const struct pvg_wire_texture *texture, uint32_t block_width,
                                           uint32_t block_height, uint32_t block_bytes);
uint64_t pvg_wire_compressed_texture_slice_bytes(const struct pvg_wire_texture *texture, uint32_t block_width,
                                                 uint32_t block_height, uint32_t block_bytes);

/*
 * A view of another texture with a pixel format of its own, kind 8
 * (PVG_WIRE_KIND_TEXTURE_VIEW). It is no resource: it stays out of command
 * 67's list, and its base texture is listed.
 */
size_t pvg_wire_texture_view(void *out, size_t size, uint32_t number, uint32_t base, uint32_t format);

/*
 * A view of some levels and slices of a texture, as a texture of `type`
 * (MTLTextureType), kind 8. With `swizzle`, four of Metal's
 * MTLTextureSwizzle for red, green, blue and alpha, the view also reorders
 * the channels; NULL for none.
 */
size_t pvg_wire_texture_view_range(void *out, size_t size, uint32_t number, uint32_t base, uint32_t format,
                                   uint32_t type, uint64_t first_level, uint64_t levels, uint64_t first_slice,
                                   uint64_t slices, const uint8_t *swizzle);

/* A texture over the surface of a mapping of the IOSurface mapper, kind 11. Rendering into it writes the surface. */
size_t pvg_wire_surface_texture(void *out, size_t size, uint32_t number, const struct pvg_wire_texture *texture,
                                uint64_t mapping, uint32_t plane);

/*
 * A function, kind 6. Its code is a Metal library file with this one
 * function, `length` bytes at `address` in the task's memory.
 */
size_t pvg_wire_function(void *out, size_t size, uint32_t number, uint64_t address, uint32_t length);

/* A sampler, kind 7. pvg_wire_sampler_init sets Metal's defaults. */
struct pvg_wire_sampler {
   uint32_t min_filter, mag_filter; /* MTLSamplerMinMagFilter */
   uint32_t mip_filter;             /* MTLSamplerMipFilter */
   uint32_t address_s, address_t, address_r; /* MTLSamplerAddressMode */
   uint32_t border_colour;          /* MTLSamplerBorderColor */
   uint32_t max_anisotropy;         /* 1 to 16 */
   uint32_t compare;                /* MTLCompareFunction */
   bool normalized;
   bool lod_average;
   bool argument_buffers;           /* supportArgumentBuffers */
   float lod_min, lod_max;
};

void pvg_wire_sampler_init(struct pvg_wire_sampler *sampler);
size_t pvg_wire_sampler(void *out, size_t size, uint32_t number, const struct pvg_wire_sampler *sampler);

/* One face's stencil test. pvg_wire_depth_stencil_init sets Metal's defaults: always, keep, every bit. */
struct pvg_wire_stencil {
   uint32_t compare;                         /* MTLCompareFunction */
   uint32_t stencil_fail, depth_fail, pass;  /* MTLStencilOperation */
   uint32_t read_mask, write_mask;
};

/* A depth and stencil state, kind 7. pvg_wire_depth_stencil_init sets Metal's defaults. */
struct pvg_wire_depth_stencil {
   uint32_t compare; /* MTLCompareFunction; 7 is always */
   bool write;
   bool has_front, has_back; /* a face is written only when it is there */
   struct pvg_wire_stencil front, back;
};

void pvg_wire_depth_stencil_init(struct pvg_wire_depth_stencil *state);
size_t pvg_wire_depth_stencil(void *out, size_t size, uint32_t number, const struct pvg_wire_depth_stencil *state);

#define PVG_WIRE_MOST_ATTRIBUTES 31u
#define PVG_WIRE_MOST_LAYOUTS 31u
#define PVG_WIRE_MOST_COLOURS 8u

struct pvg_wire_attribute {
   uint32_t format; /* MTLVertexFormat; 0 for an attribute that is not used */
   uint32_t offset;
   uint32_t buffer;
};

struct pvg_wire_layout {
   uint32_t stride;
   uint32_t step_function; /* MTLVertexStepFunction; 1 is per vertex */
   uint32_t step_rate;
};

struct pvg_wire_colour {
   uint32_t format; /* MTLPixelFormat; 0 for an attachment that is not used */
   bool blending;
   uint32_t source_rgb, destination_rgb, rgb_operation;       /* MTLBlendFactor, MTLBlendOperation */
   uint32_t source_alpha, destination_alpha, alpha_operation;
   uint32_t write_mask; /* MTLColorWriteMask; 15 is all */
};

/*
 * A render pipeline, kind 7. Its functions are named by their object
 * numbers, 0 for none. pvg_wire_render_pipeline_init sets Metal's defaults.
 */
struct pvg_wire_render_pipeline {
   uint32_t vertex_function, fragment_function;
   struct pvg_wire_attribute attributes[PVG_WIRE_MOST_ATTRIBUTES];
   struct pvg_wire_layout layouts[PVG_WIRE_MOST_LAYOUTS];
   struct pvg_wire_colour colours[PVG_WIRE_MOST_COLOURS];
   uint32_t depth_format, stencil_format; /* MTLPixelFormat; 0 for none */
   /*
    * Samples a pixel, 0 for Metal's default of 1. Written as setting 4,
    * the place rasterSampleCount has in Metal's descriptor between the
    * vertex descriptor (3) and the colours (8); the helper's taking it
    * is scene msaa-resolve's question.
    */
   uint32_t raster_sample_count;
};

void pvg_wire_render_pipeline_init(struct pvg_wire_render_pipeline *pipeline);
size_t pvg_wire_render_pipeline(void *out, size_t size, uint32_t number,
                                const struct pvg_wire_render_pipeline *pipeline);

/* A compute pipeline, kind 7, of one function named by its object number. */
size_t pvg_wire_compute_pipeline(void *out, size_t size, uint32_t number, uint32_t function);

/*
 * A command buffer is one segment: of several segments in one buffer only
 * the first acts. A writer lays segments one after the other into memory of
 * the caller's, and each is handed over as a command buffer of its own, by
 * its place and its length.
 *
 * A call that finds no room writes nothing and sets `failed`, and every
 * call after it does the same. Look at it once, after pvg_wire_end.
 */
struct pvg_wire {
   uint8_t *bytes;
   size_t size;
   size_t used;
   size_t segment; /* where the open segment starts */
   bool open;
   bool failed;
};

void pvg_wire_init(struct pvg_wire *wire, void *memory, size_t size);

/* One attachment of a render pass. The numbers are Metal's: MTLLoadAction, MTLStoreAction. */
struct pvg_wire_attachment {
   uint32_t texture, resolve_texture; /* object numbers; 0 for none */
   uint16_t level, slice, depth_plane;
   uint16_t resolve_level, resolve_slice, resolve_depth_plane;
   uint16_t load, store;
   uint32_t store_options;
   double clear[4];        /* a colour attachment's clear colour */
   double clear_depth;     /* the depth attachment's */
   uint32_t clear_stencil; /* the stencil attachment's */
   uint32_t resolve_filter; /* the depth and the stencil attachment's */
};

struct pvg_wire_pass {
   struct pvg_wire_attachment depth, stencil;
   struct pvg_wire_attachment colours[PVG_WIRE_MOST_COLOURS];
   uint32_t visibility_buffer;
   uint64_t array_length, width, height;
};

/* A pass as Metal makes it: nothing attached, colour stored, depth and stencil cleared to 1 and 0. */
void pvg_wire_pass_init(struct pvg_wire_pass *pass);

/* Begins a render segment with its pass, render command 26. */
void pvg_wire_begin_render(struct pvg_wire *wire, const struct pvg_wire_pass *pass);
void pvg_wire_begin_compute(struct pvg_wire *wire);
void pvg_wire_begin_blit(struct pvg_wire *wire);

/*
 * Ends the segment and returns its length in bytes, 0 if the writer failed.
 * The segment lies at `bytes + segment`.
 */
size_t pvg_wire_end(struct pvg_wire *wire);

/* Render commands. Objects are named by their numbers in the task's object list. */
void pvg_wire_set_render_pipeline(struct pvg_wire *wire, uint32_t pipeline);
void pvg_wire_set_depth_stencil(struct pvg_wire *wire, uint32_t state);
void pvg_wire_set_vertex_buffer(struct pvg_wire *wire, uint32_t index, uint32_t buffer, uint64_t offset);
void pvg_wire_set_vertex_buffer_offset(struct pvg_wire *wire, uint32_t index, uint64_t offset);
void pvg_wire_set_fragment_buffer(struct pvg_wire *wire, uint32_t index, uint32_t buffer, uint64_t offset);
void pvg_wire_set_fragment_buffer_offset(struct pvg_wire *wire, uint32_t index, uint64_t offset);
void pvg_wire_set_fragment_texture(struct pvg_wire *wire, uint32_t index, uint32_t texture);
void pvg_wire_set_fragment_sampler(struct pvg_wire *wire, uint32_t index, uint32_t sampler);
void pvg_wire_set_viewport(struct pvg_wire *wire, double x, double y, double width, double height, double near,
                           double far);
void pvg_wire_set_scissor(struct pvg_wire *wire, uint64_t x, uint64_t y, uint64_t width, uint64_t height);
void pvg_wire_set_cull_mode(struct pvg_wire *wire, uint32_t mode);  /* MTLCullMode */
void pvg_wire_set_front_facing(struct pvg_wire *wire, uint32_t winding); /* MTLWinding */
void pvg_wire_set_blend_colour(struct pvg_wire *wire, float red, float green, float blue, float alpha);
/* The value a stencil test compares with, for front and back faces. */
void pvg_wire_set_stencil_reference(struct pvg_wire *wire, uint32_t front, uint32_t back);

/*
 * Says that the draws after it reach these resources though no binding
 * names them, as a shader does that reads a resource's number from memory.
 * Command 67's list of resources is not enough for that. `usage` is Metal's
 * MTLResourceUsage, `stages` its MTLRenderStages.
 */
void pvg_wire_use_resources(struct pvg_wire *wire, const uint32_t *resources, uint32_t count, uint32_t usage,
                            uint32_t stages);

/*
 * Draws. `primitive` is Metal's MTLPrimitiveType, `index_type` its
 * MTLIndexType. Each writes the short command when every number is below
 * 65536 and the long one otherwise, as the serializer does.
 */
void pvg_wire_draw(struct pvg_wire *wire, uint32_t primitive, uint64_t start, uint64_t count);
void pvg_wire_draw_indexed(struct pvg_wire *wire, uint32_t primitive, uint64_t count, uint32_t index_type,
                           uint32_t buffer, uint64_t offset);
void pvg_wire_draw_instanced(struct pvg_wire *wire, uint32_t primitive, uint64_t start, uint64_t count,
                             uint64_t instances);
void pvg_wire_draw_indexed_instanced(struct pvg_wire *wire, uint32_t primitive, uint64_t count,
                                     uint32_t index_type, uint32_t buffer, uint64_t offset, uint64_t instances);

/*
 * An indexed draw whose indices count from `base_vertex` and whose instances
 * from `base_instance`: render 29, or 28 when a number does not fit 16 bits.
 */
void pvg_wire_draw_indexed_base(struct pvg_wire *wire, uint32_t primitive, uint64_t count, uint32_t index_type,
                                uint32_t buffer, uint64_t offset, uint64_t instances, int64_t base_vertex,
                                uint64_t base_instance);

/* Compute commands. */
void pvg_wire_set_compute_pipeline(struct pvg_wire *wire, uint32_t pipeline);
void pvg_wire_set_buffer(struct pvg_wire *wire, uint32_t index, uint32_t buffer, uint64_t offset);
void pvg_wire_set_buffer_offset(struct pvg_wire *wire, uint32_t index, uint64_t offset);
void pvg_wire_set_texture(struct pvg_wire *wire, uint32_t index, uint32_t texture);
void pvg_wire_set_sampler(struct pvg_wire *wire, uint32_t index, uint32_t sampler);
void pvg_wire_dispatch(struct pvg_wire *wire, const uint64_t groups[3], const uint64_t threads_per_group[3]);
void pvg_wire_set_stage_in_region(struct pvg_wire *wire, const uint64_t origin[3], const uint64_t size[3]);
void pvg_wire_set_threadgroup_memory_length(struct pvg_wire *wire, uint32_t index, uint64_t length);
void pvg_wire_memory_barrier(struct pvg_wire *wire, uint32_t scope); /* MTLBarrierScope */

/* Blit commands. An origin is x, y, z and a size is width, height, depth. */
void pvg_wire_copy_buffer(struct pvg_wire *wire, uint32_t source, uint64_t source_offset, uint32_t destination,
                          uint64_t destination_offset, uint64_t length);
void pvg_wire_copy_buffer_to_texture(struct pvg_wire *wire, uint32_t buffer, uint64_t offset, uint64_t bytes_per_row,
                                     uint64_t bytes_per_image, const uint64_t size[3], uint32_t texture,
                                     uint32_t slice, uint32_t level, const uint64_t origin[3]);
void pvg_wire_copy_texture_to_buffer(struct pvg_wire *wire, uint32_t texture, uint32_t slice, uint32_t level,
                                     const uint64_t origin[3], const uint64_t size[3], uint32_t buffer,
                                     uint64_t offset, uint64_t bytes_per_row, uint64_t bytes_per_image);
void pvg_wire_copy_texture(struct pvg_wire *wire, uint32_t source, uint32_t source_slice, uint32_t source_level,
                           const uint64_t source_origin[3], const uint64_t size[3], uint32_t destination,
                           uint32_t destination_slice, uint32_t destination_level,
                           const uint64_t destination_origin[3]);
void pvg_wire_fill_buffer(struct pvg_wire *wire, uint32_t buffer, uint64_t offset, uint64_t length, uint8_t value);
void pvg_wire_generate_mipmaps(struct pvg_wire *wire, uint32_t texture);
void pvg_wire_synchronize(struct pvg_wire *wire, uint32_t resource);


#ifdef __cplusplus
}
#endif

#endif
