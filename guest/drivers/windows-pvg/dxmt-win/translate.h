/* SPDX-License-Identifier: MIT */
/*
 * The part of winemetal over ruxnpvg that turns DXMT's encoded commands
 * (winemetal.h's wmtcmd lists and render pass) into pvg_wire's bytes. It
 * knows nothing of Windows or the driver, so the Mac can build it and check
 * its bytes (wiretest.c).
 *
 * Only the commands pvgtri and pvgcube send are written, each by an existing
 * pvg_wire call. State that equals Metal's default, or has no effect with the
 * pipeline bound, is dropped and noted once. Everything else fails the
 * encoder: the command buffer is then not submitted.
 */
#ifndef PVG_TRANSLATE_H
#define PVG_TRANSLATE_H

#include <stdbool.h>
#include <stdint.h>

#include "winemetal.h"
#include "pvg_wire.h"

enum pvgx_kind {
	PVGX_NONE,
	PVGX_BUFFER,
	PVGX_TEXTURE,
	PVGX_RENDER_PIPELINE,
	PVGX_COMPUTE_PIPELINE,
	PVGX_DEPTH_STENCIL,
	PVGX_SAMPLER,
	PVGX_PLACEHOLDER, /* from DXMT's helper library, which is not there */
	PVGX_OTHER,
};

/* What the encoder needs to know of an object DXMT names by its handle. */
struct pvgx_info {
	enum pvgx_kind kind;
	uint32_t number;          /* the object's number in the task */
	uint64_t offset;          /* a buffer's: where it starts in that object */
	uint32_t width, height;   /* a texture's */
	uint32_t view;            /* a texture over a surface: the view shaders reach it by, or 0 */
	uint32_t listed;          /* what command 67 lists for it, if not itself: a texture over a buffer's buffer */
	uint32_t base;            /* a view of one level: the texture it views, and that level */
	uint32_t base_level;
	uint32_t base_slice;      /* a view of slices: its first slice in the texture it views */
	uint32_t levels;          /* the levels the device has; a copy into a level past them is dropped */
	bool blending;            /* a render pipeline's: any attachment blends */
	bool fresh;               /* a buffer nothing has written since it was made */
	uint32_t stencil;         /* a depth-and-stencil texture's stencil, a texture of its own (wm_resources.c) */
};

typedef bool (*pvgx_lookup)(void *context, obj_handle_t handle, struct pvgx_info *info);
/* Room for bytes a command carries (setFragmentBytes): copied into memory
 * the command buffer owns, which is then bound as a buffer by its object's
 * number and offset. NULL where there is none (wiretest). */
typedef bool (*pvgx_inline)(void *context, const void *bytes, uint64_t length, uint32_t *number,
			    uint64_t *offset);
typedef void (*pvgx_note)(void *context, const char *what);

#define PVGX_MOST_RESOURCES 256   /* the driver takes 256 a submit */
#define PVGX_RESOURCE_WRITTEN PVG_WIRE_RESOURCE_WRITTEN

/* The resources one command buffer's encoders reach, for command 67's list. */
struct pvgx_resources {
	uint32_t ids[PVGX_MOST_RESOURCES];
	uint32_t flags[PVGX_MOST_RESOURCES];
	uint32_t count;
	bool overflow;
};

#define PVGX_BINDINGS 31

struct pvgx_encoder {
	struct pvg_wire *wire;
	pvgx_lookup lookup;
	pvgx_note note;
	void *context;
	struct pvgx_resources *resources;
	pvgx_inline inline_bytes;

	/* Set by pvgx_begin_render: the size of the first attachment. */
	uint32_t width, height;
	/* The render pipeline bound last, and the buffers bound at each index. */
	struct pvgx_info pipeline;
	uint32_t vertex_buffers[PVGX_BINDINGS], fragment_buffers[PVGX_BINDINGS];
	/* A compute segment's: its buffers and their bases, and the threads a
	 * group of the pipeline set last (DXMT gives it with the pipeline). */
	uint32_t compute_buffers[PVGX_BINDINGS];
	uint64_t compute_bases[PVGX_BINDINGS];
	uint64_t group_threads[3];
	uint64_t vertex_bases[PVGX_BINDINGS], fragment_bases[PVGX_BINDINGS];

	/* State set away from Metal's default in this encoder: a later
	 * command back to the default must then be sent, not dropped (run
	 * rn58: a cull mode, scissor or viewport left from an earlier draw
	 * clipped later ones). */
	bool culling, viewport_set, scissor_set, stencil_reference_set;

	unsigned commands;  /* commands written since the segment began */
	bool failed;
	char why[160];
};

void pvgx_encoder_init(struct pvgx_encoder *encoder, struct pvg_wire *wire, pvgx_lookup lookup,
		       pvgx_note note, void *context, struct pvgx_resources *resources);

/* Render command 26 with the pass, as pvg_wire_begin_render writes it. */
void pvgx_begin_render(struct pvgx_encoder *encoder, const struct WMTRenderPassInfo *info);
void pvgx_render_commands(struct pvgx_encoder *encoder, const struct wmtcmd_base *head);
void pvgx_blit_commands(struct pvgx_encoder *encoder, const struct wmtcmd_base *head);
void pvgx_compute_commands(struct pvgx_encoder *encoder, const struct wmtcmd_base *head);

/* Adds a resource to the list, or its written flag to it. */
void pvgx_use(struct pvgx_resources *resources, uint32_t id, uint32_t flags);

#endif
