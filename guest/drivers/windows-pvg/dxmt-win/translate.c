/* SPDX-License-Identifier: MIT */
/*
 * DXMT's encoded commands to pvg_wire's bytes. See translate.h for the
 * rules. Each case says which program has sent the command it writes.
 */
#include <stdio.h>
#include <string.h>

#include "translate.h"
#include "translate_internal.h"

void pvgx_encoder_init(struct pvgx_encoder *encoder, struct pvg_wire *wire, pvgx_lookup lookup,
		       pvgx_note note, void *context, struct pvgx_resources *resources)
{
	memset(encoder, 0, sizeof(*encoder));
	encoder->wire = wire;
	encoder->lookup = lookup;
	encoder->note = note;
	encoder->context = context;
	encoder->resources = resources;
}

void pvgx_use(struct pvgx_resources *resources, uint32_t id, uint32_t flags)
{
	uint32_t i;

	for (i = 0; i < resources->count; i++) {
		if (resources->ids[i] == id) {
			resources->flags[i] |= flags;
			return;
		}
	}
	if (resources->count == PVGX_MOST_RESOURCES) {
		resources->overflow = true;
		return;
	}
	resources->ids[resources->count] = id;
	resources->flags[resources->count] = flags;
	resources->count++;
}

void pvgx_fail(struct pvgx_encoder *encoder, const char *what, unsigned value)
{
	if (encoder->failed)
		return;
	encoder->failed = true;
	snprintf(encoder->why, sizeof(encoder->why), "%s (%u)", what, value);
}

/*
 * What command 67 lists for an object: itself, or for a texture that is no
 * resource (a texture over a buffer, kind 8) the resource it lies over.
 */
uint32_t pvgx_listed(const struct pvgx_info *info)
{
	return info->listed ? info->listed : info->number;
}

/*
 * The texture a pass draws into or a blit names, its level and slice: for
 * a view the texture it views at the view's level and slice, as Metal means
 * it and as the helper was seen to do (level-pass, array-pass, cube-pass);
 * a view as an attachment or in a blit was never sent.
 */
uint32_t pvgx_texture_at(const struct pvgx_info *info, unsigned *level, unsigned *slice)
{
	if (!info->base)
		return info->number;
	*level += info->base_level;
	*slice += info->base_slice;
	return info->base;
}

/*
 * A compressed texture has on the device only the levels of a whole block
 * or more (wm_resources.c): a copy into or out of a level past them is
 * dropped, said once.
 */
bool pvgx_level_missing(struct pvgx_encoder *encoder, const struct pvgx_info *info, unsigned level)
{
	if (!info->levels || level < info->levels)
		return false;
	pvgx_drop(encoder, "a copy of a compressed texture's level smaller than a block");
	return true;
}

void pvgx_drop(struct pvgx_encoder *encoder, const char *what)
{
	if (encoder->note)
		encoder->note(encoder->context, what);
}

bool pvgx_look(struct pvgx_encoder *encoder, obj_handle_t handle, enum pvgx_kind kind,
		 struct pvgx_info *info)
{
	memset(info, 0, sizeof(*info));
	if (!handle || !encoder->lookup(encoder->context, handle, info) || info->kind != kind) {
		pvgx_fail(encoder, kind == PVGX_TEXTURE ? "not a texture of ours" :
			      kind == PVGX_BUFFER ? "not a buffer of ours" :
			      kind == PVGX_RENDER_PIPELINE ? "not a render pipeline of ours (a placeholder?)" :
			      kind == PVGX_DEPTH_STENCIL ? "not a depth and stencil state of ours" :
			      "not an object of ours", info->kind);
		return false;
	}
	return true;
}

/* A buffer is a range of a chunk's object: its number, and in *base where
 * the range starts in it. */
uint32_t pvgx_buffer_number(struct pvgx_encoder *encoder, obj_handle_t handle, uint64_t *base)
{
	struct pvgx_info info;

	*base = 0;
	if (!pvgx_look(encoder, handle, PVGX_BUFFER, &info))
		return 0;
	pvgx_use(encoder->resources, pvgx_listed(&info), 0);
	*base = info.offset;
	return info.number;
}

static void attachment(struct pvgx_encoder *encoder, struct pvg_wire_attachment *to, obj_handle_t texture,
		       unsigned load, unsigned store, unsigned level, unsigned slice, unsigned plane)
{
	struct pvgx_info info;

	if (!pvgx_look(encoder, texture, PVGX_TEXTURE, &info))
		return;
	to->texture = pvgx_texture_at(&info, &level, &slice);
	to->load = (uint16_t)load;
	to->store = (uint16_t)store;
	to->level = (uint16_t)level;
	to->slice = (uint16_t)slice;
	to->depth_plane = (uint16_t)plane;
	pvgx_use(encoder->resources, pvgx_listed(&info), PVGX_RESOURCE_WRITTEN);
	if (!encoder->width) {
		encoder->width = info.width;
		encoder->height = info.height;
	}
}

// Where a multisample colour attachment resolves to, with the store action
// the pass gives (helper scenes msaa-small-clear and msaa-small).
static void resolve(struct pvgx_encoder *encoder, struct pvg_wire_attachment *to,
		    const struct WMTColorAttachmentInfo *c)
{
	struct pvgx_info info;
	unsigned level = c->resolve_level, slice = c->resolve_slice;

	if (!pvgx_look(encoder, c->resolve_texture, PVGX_TEXTURE, &info))
		return;
	to->resolve_texture = pvgx_texture_at(&info, &level, &slice);
	to->resolve_level = (uint16_t)level;
	to->resolve_slice = (uint16_t)slice;
	to->resolve_depth_plane = (uint16_t)c->resolve_depth_plane;
	pvgx_use(encoder->resources, pvgx_listed(&info), PVGX_RESOURCE_WRITTEN);
}

void pvgx_begin_render(struct pvgx_encoder *encoder, const struct WMTRenderPassInfo *info)
{
	/* Too large for a small stack, and pvg_wire_pass_init sets all of it. */
	static struct pvg_wire_pass pass;
	unsigned i;

	pvg_wire_pass_init(&pass);
	for (i = 0; i < PVG_WIRE_MOST_COLOURS; i++) {
		const struct WMTColorAttachmentInfo *c = &info->colors[i];

		if (!c->texture)
			continue;
		attachment(encoder, &pass.colours[i], c->texture, c->load_action, c->store_action, c->level,
			   c->slice, c->depth_plane);
		if (c->resolve_texture)
			resolve(encoder, &pass.colours[i], c);
		pass.colours[i].clear[0] = c->clear_color.r;
		pass.colours[i].clear[1] = c->clear_color.g;
		pass.colours[i].clear[2] = c->clear_color.b;
		pass.colours[i].clear[3] = c->clear_color.a;
	}
	if (info->depth.texture) {
		attachment(encoder, &pass.depth, info->depth.texture, info->depth.load_action,
			   info->depth.store_action, info->depth.level, info->depth.slice,
			   info->depth.depth_plane);
		pass.depth.clear_depth = info->depth.clear_depth;
	}
	if (info->stencil.texture) {
		struct pvgx_info stencil;

		attachment(encoder, &pass.stencil, info->stencil.texture, info->stencil.load_action,
			   info->stencil.store_action, info->stencil.level, info->stencil.slice,
			   info->stencil.depth_plane);
		/* A depth-and-stencil texture is two on the device: the
		 * stencil attachment is its stencil texture (wm_resources.c). */
		if (pvgx_look(encoder, info->stencil.texture, PVGX_TEXTURE, &stencil) && stencil.stencil) {
			pass.stencil.texture = stencil.stencil;
			pvgx_use(encoder->resources, stencil.stencil, PVGX_RESOURCE_WRITTEN);
		}
		pass.stencil.clear_stencil = info->stencil.clear_stencil;
	}
	if (info->visibility_buffer)
		pvgx_fail(encoder, "a visibility buffer", 0);
	if (info->default_raster_sample_count > 1)
		pvgx_fail(encoder, "a raster sample count", info->default_raster_sample_count);
	/*
	 * The pass's own size and layer count: with an attachment Metal takes
	 * them from it, so a size equal to the attachment's is the default.
	 */
	if (info->render_target_width || info->render_target_height) {
		if (encoder->width && info->render_target_width == encoder->width &&
		    info->render_target_height == encoder->height) {
			pvgx_drop(encoder, "the pass's size, equal to its attachment's");
		} else {
			/* A pass with no attachment, or one that draws into part
			 * of it: the pass's own size, a field of command 26. */
			pass.width = info->render_target_width;
			pass.height = info->render_target_height;
			if (!encoder->width) {
				encoder->width = info->render_target_width;
				encoder->height = info->render_target_height;
			}
		}
	}
	if (info->render_target_array_length > 1)
		pvgx_fail(encoder, "a layered pass", info->render_target_array_length);
	/*
	 * A pass with no attachment at all is DXMT's for a target that was
	 * never made; the device refuses it, and a refusal spoils the task's
	 * next commands (run rn16: 40 such, after two shared textures could
	 * not be had). Not sent.
	 */
	if (!pass.depth.texture && !pass.stencil.texture) {
		for (i = 0; i < PVG_WIRE_MOST_COLOURS && !pass.colours[i].texture; i++)
			;
		if (i == PVG_WIRE_MOST_COLOURS)
			pvgx_fail(encoder, "a pass with no attachment", 0);
	}
	if (encoder->failed)
		return;
	/* pvgtri and pvgcube: render command 26. */
	pvg_wire_begin_render(encoder->wire, &pass);
}


/* Metal's defaults: the whole attachment and nothing culled. A
 * command equal to them is dropped, so that a draw that sets nothing else
 * writes pvgtri's bytes exactly. */
static bool viewport_is_default(const struct pvgx_encoder *encoder, const struct WMTViewport *v)
{
	return v->originX == 0 && v->originY == 0 && v->width == (double)encoder->width &&
	       v->height == (double)encoder->height && v->znear == 0 && v->zfar == 1;
}

static bool scissor_is_default(const struct pvgx_encoder *encoder, const struct WMTScissorRect *r)
{
	return r->x == 0 && r->y == 0 && r->width == encoder->width && r->height == encoder->height;
}

static void set_buffer(struct pvgx_encoder *encoder, bool fragment, obj_handle_t buffer, uint64_t offset,
		       unsigned index)
{
	uint32_t number;
	uint64_t base;

	if (index >= PVGX_BINDINGS) {
		pvgx_fail(encoder, "a buffer index out of range", index);
		return;
	}
	if (!buffer) {
		pvgx_drop(encoder, "a buffer binding of nothing");
		return;
	}
	number = pvgx_buffer_number(encoder, buffer, &base);
	if (!number)
		return;
	/* pvgcube: render commands for vertex and fragment buffers. */
	if (fragment) {
		pvg_wire_set_fragment_buffer(encoder->wire, index, number, base + offset);
		encoder->fragment_buffers[index] = number;
		encoder->fragment_bases[index] = base;
	} else {
		pvg_wire_set_vertex_buffer(encoder->wire, index, number, base + offset);
		encoder->vertex_buffers[index] = number;
		encoder->vertex_bases[index] = base;
	}
	encoder->commands++;
}

/*
 * A new offset for the buffer bound at an index: written as the binding
 * again, which pvgcube sends, not as the offset command, which nobody has.
 */
static void set_buffer_offset(struct pvgx_encoder *encoder, bool fragment, uint64_t offset, unsigned index)
{
	uint32_t number;

	if (index >= PVGX_BINDINGS) {
		pvgx_fail(encoder, "a buffer index out of range", index);
		return;
	}
	number = fragment ? encoder->fragment_buffers[index] : encoder->vertex_buffers[index];
	if (!number) {
		pvgx_fail(encoder, "an offset for an index with no buffer", index);
		return;
	}
	if (fragment)
		pvg_wire_set_fragment_buffer(encoder->wire, index, number, encoder->fragment_bases[index] + offset);
	else
		pvg_wire_set_vertex_buffer(encoder->wire, index, number, encoder->vertex_bases[index] + offset);
	encoder->commands++;
}

void pvgx_render_commands(struct pvgx_encoder *encoder, const struct wmtcmd_base *cmd)
{
	struct pvgx_info info;

	for (; cmd && !encoder->failed; cmd = NEXT(cmd)) {
		switch (cmd->type) {
		case WMTRenderCommandNop:
			break;
		case WMTRenderCommandUseResource: {
			const struct wmtcmd_render_useresource *c = (const void *)cmd;

			/*
			 * Says that shaders reach a resource through a number they
			 * read from memory (DXMT's tables hold every buffer's
			 * address and every texture's id). Render command 137,
			 * which the device needs for that: command 67's list alone
			 * leaves such a resource unseen (kosmickrisp-seam.md).
			 *
			 * A resource that was never made (handle 0: a texture
			 * over a buffer, a view not carried yet) is left out
			 * rather than failing the command buffer: a shader that
			 * reads through its number gets zeros, as one did through
			 * a number nothing named (seam study), and the rest of the
			 * frame is drawn (run rn11: Notepad lost 105 command
			 * buffers, dwm 461, all for this).
			 */
			if (!c->resource) {
				pvgx_drop(encoder, "use of a resource never made: left out, its reads give zeros");
				break;
			}
			if (!encoder->lookup(encoder->context, c->resource, &info) ||
			    (info.kind != PVGX_BUFFER && info.kind != PVGX_TEXTURE)) {
				pvgx_fail(encoder, "use of a resource not ours", 0);
				break;
			}
			pvgx_use(encoder->resources, pvgx_listed(&info),
				 (c->usage & WMTResourceUsageWrite) ? PVGX_RESOURCE_WRITTEN : 0);
			{
				/*
				 * A texture over a surface is reached by its
				 * view's resource id; the view is named too, as
				 * the one sampled through a view was (seam study,
				 * view-draw). It stays out of the list, its base
				 * is in it.
				 */
				uint32_t used[3] = { info.number };
				uint32_t count = 1;

				if (info.view)
					used[count++] = info.view;
				/* A texture over a buffer reads the buffer's memory. */
				if (info.listed)
					used[count++] = info.listed;
				pvg_wire_use_resources(encoder->wire, used, count, c->usage, c->stages);
			}
			encoder->commands++;
			break;
		}
		case WMTRenderCommandSetVertexBuffer:
		case WMTRenderCommandSetFragmentBuffer: {
			const struct wmtcmd_render_setbuffer *c = (const void *)cmd;

			set_buffer(encoder, cmd->type == WMTRenderCommandSetFragmentBuffer, c->buffer, c->offset,
				   c->index);
			break;
		}
		case WMTRenderCommandSetFragmentBytes: {
			const struct wmtcmd_render_setbytes *c = (const void *)cmd;
			uint32_t number;
			uint64_t offset;

			/*
			 * DXMT's clear helper hands its colour so (ClearView with
			 * rectangles). Metal copies such bytes into the command
			 * buffer; so does this, then binds them as a buffer.
			 */
			if (!encoder->inline_bytes || c->index >= PVGX_BINDINGS ||
			    !encoder->inline_bytes(encoder->context, c->bytes.ptr, c->length, &number, &offset)) {
				pvgx_fail(encoder, "fragment bytes with no room for them", (unsigned)c->length);
				break;
			}
			pvgx_use(encoder->resources, number, 0);
			pvg_wire_set_fragment_buffer(encoder->wire, c->index, number, offset);
			encoder->fragment_buffers[c->index] = number;
			encoder->fragment_bases[c->index] = offset;
			encoder->commands++;
			break;
		}
		case WMTRenderCommandSetVertexBufferOffset:
		case WMTRenderCommandSetFragmentBufferOffset: {
			const struct wmtcmd_render_setbufferoffset *c = (const void *)cmd;

			set_buffer_offset(encoder, cmd->type == WMTRenderCommandSetFragmentBufferOffset, c->offset,
					  c->index);
			break;
		}
		case WMTRenderCommandSetFragmentTexture: {
			const struct wmtcmd_render_settexture *c = (const void *)cmd;

			if (!pvgx_look(encoder, c->texture, PVGX_TEXTURE, &info))
				break;
			pvgx_use(encoder->resources, pvgx_listed(&info), 0);
			/* pvgcube: the fragment texture. */
			pvg_wire_set_fragment_texture(encoder->wire, c->index, info.number);
			encoder->commands++;
			break;
		}
		case WMTRenderCommandSetRasterizerState: {
			const struct wmtcmd_render_setrasterizerstate *c = (const void *)cmd;

			/*
			 * Cull mode and winding have render commands of their own;
			 * lines, depth clamping and depth bias have none written
			 * yet, so those fail.
			 */
			if (c->fill_mode != WMTTriangleFillModeFill || c->depth_clip_mode != WMTDepthClipModeClip ||
			    c->depth_bias != 0 || c->scole_scale != 0 || c->depth_bias_clamp != 0) {
				pvgx_fail(encoder, "rasterizer state with lines, depth clamping or depth bias",
				     c->fill_mode);
				break;
			}
			/* With nothing culled the winding has no effect. */
			if (c->cull_mode == WMTCullModeNone && !encoder->culling) {
				pvgx_drop(encoder, "rasterizer state equal to Metal's default");
				break;
			}
			pvg_wire_set_cull_mode(encoder->wire, c->cull_mode);
			encoder->commands++;
			encoder->culling = c->cull_mode != WMTCullModeNone;
			if (encoder->culling) {
				pvg_wire_set_front_facing(encoder->wire, c->winding);
				encoder->commands++;
			}
			break;
		}
		case WMTRenderCommandSetViewports:
		case WMTRenderCommandSetViewport: {
			const struct WMTViewport *v;

			if (cmd->type == WMTRenderCommandSetViewports) {
				const struct wmtcmd_render_setviewports *c = (const void *)cmd;

				if (c->viewport_count != 1) {
					pvgx_fail(encoder, "more than one viewport", c->viewport_count);
					break;
				}
				v = c->viewports.ptr;
			} else {
				v = &((const struct wmtcmd_render_setviewport *)(const void *)cmd)->viewport;
			}
			if (viewport_is_default(encoder, v) && !encoder->viewport_set) {
				pvgx_drop(encoder, "a viewport equal to the attachment");
				break;
			}
			pvg_wire_set_viewport(encoder->wire, v->originX, v->originY, v->width, v->height, v->znear,
					      v->zfar);
			encoder->commands++;
			encoder->viewport_set = !viewport_is_default(encoder, v);
			break;
		}
		case WMTRenderCommandSetScissorRects:
		case WMTRenderCommandSetScissorRect: {
			const struct WMTScissorRect *r;

			if (cmd->type == WMTRenderCommandSetScissorRects) {
				const struct wmtcmd_render_setscissorrects *c = (const void *)cmd;

				if (c->rect_count != 1) {
					pvgx_fail(encoder, "more than one scissor", c->rect_count);
					break;
				}
				r = c->scissor_rects.ptr;
			} else {
				r = &((const struct wmtcmd_render_setscissorrect *)(const void *)cmd)->scissor_rect;
			}
			if (scissor_is_default(encoder, r) && !encoder->scissor_set) {
				pvgx_drop(encoder, "a scissor equal to the attachment");
				break;
			}
			pvg_wire_set_scissor(encoder->wire, r->x, r->y, r->width, r->height);
			encoder->commands++;
			encoder->scissor_set = !scissor_is_default(encoder, r);
			break;
		}
		case WMTRenderCommandSetPSO: {
			const struct wmtcmd_render_setpso *c = (const void *)cmd;

			if (!pvgx_look(encoder, c->pso, PVGX_RENDER_PIPELINE, &encoder->pipeline))
				break;
			/* pvgtri and pvgcube: the render pipeline. */
			pvg_wire_set_render_pipeline(encoder->wire, encoder->pipeline.number);
			encoder->commands++;
			break;
		}
		case WMTRenderCommandSetDSSO:
		case WMTRenderCommandSetDepthStencilState: {
			obj_handle_t state;

			if (cmd->type == WMTRenderCommandSetDSSO) {
				const struct wmtcmd_render_setdsso *c = (const void *)cmd;

				if (c->stencil_ref || encoder->stencil_reference_set) {
					pvg_wire_set_stencil_reference(encoder->wire, c->stencil_ref, c->stencil_ref);
					encoder->commands++;
					encoder->stencil_reference_set = c->stencil_ref != 0;
				}
				state = c->dsso;
			} else {
				state = ((const struct wmtcmd_render_setdepthstencilstate *)(const void *)cmd)
						->depth_stencil_state;
			}
			if (!pvgx_look(encoder, state, PVGX_DEPTH_STENCIL, &info))
				break;
			/* pvgcube: the depth and stencil state. */
			pvg_wire_set_depth_stencil(encoder->wire, info.number);
			encoder->commands++;
			break;
		}
		case WMTRenderCommandSetBlendFactorAndStencilRef:
		case WMTRenderCommandSetBlendFactor: {
			const struct wmtcmd_render_setblendcolor *c = (const void *)cmd;

			/* A blend colour matters only to a pipeline that blends. */
			if (encoder->pipeline.kind == PVGX_RENDER_PIPELINE && !encoder->pipeline.blending) {
				pvgx_drop(encoder, "a blend colour, with blending off in the pipeline");
			} else {
				pvg_wire_set_blend_colour(encoder->wire, c->red, c->green, c->blue, c->alpha);
				encoder->commands++;
			}
			if (cmd->type == WMTRenderCommandSetBlendFactorAndStencilRef && c->stencil_ref) {
				pvg_wire_set_stencil_reference(encoder->wire, c->stencil_ref, c->stencil_ref);
				encoder->commands++;
			}
			break;
		}
		case WMTRenderCommandSetStencilRef: {
			const struct wmtcmd_render_setstencilref *c = (const void *)cmd;

			if (!c->stencil_ref) {
				pvgx_drop(encoder, "a stencil reference of 0, Metal's default");
				break;
			}
			pvg_wire_set_stencil_reference(encoder->wire, c->stencil_ref, c->stencil_ref);
			encoder->commands++;
			break;
		}
		case WMTRenderCommandSetVisibilityMode: {
			const struct wmtcmd_render_setvisibilitymode *c = (const void *)cmd;

			if (c->mode == WMTVisibilityResultModeDisabled)
				pvgx_drop(encoder, "visibility results off, Metal's default");
			else
				pvgx_fail(encoder, "visibility results", c->mode);
			break;
		}
		case WMTRenderCommandDraw: {
			const struct wmtcmd_render_draw *c = (const void *)cmd;

			if (c->base_instance) {
				pvgx_fail(encoder, "a draw with a base instance", (unsigned)c->base_instance);
				break;
			}
			/* pvgtri: the draw. */
			if (c->instance_count == 1)
				pvg_wire_draw(encoder->wire, c->primitive_type, c->vertex_start, c->vertex_count);
			else
				pvg_wire_draw_instanced(encoder->wire, c->primitive_type, c->vertex_start,
							c->vertex_count, c->instance_count);
			encoder->commands++;
			break;
		}
		case WMTRenderCommandDrawIndexed: {
			const struct wmtcmd_render_draw_indexed *c = (const void *)cmd;
			uint32_t number;
			uint64_t base;

			if (c->instance_count != 1 || c->base_instance || c->base_vertex) {
				number = pvgx_buffer_number(encoder, c->index_buffer, &base);
				if (number)
					pvg_wire_draw_indexed_base(encoder->wire, c->primitive_type, c->index_count,
								   c->index_type, number, base + c->index_buffer_offset,
								   c->instance_count, c->base_vertex, c->base_instance);
				encoder->commands++;
				break;
			}
			number = pvgx_buffer_number(encoder, c->index_buffer, &base);
			if (!number)
				break;
			/* pvgcube: the indexed draw. */
			pvg_wire_draw_indexed(encoder->wire, c->primitive_type, c->index_count, c->index_type,
					      number, base + c->index_buffer_offset);
			encoder->commands++;
			break;
		}
		case WMTRenderCommandWaitForFence:
		case WMTRenderCommandUpdateFence:
			/*
			 * Fences order encoders of one command buffer; the device
			 * runs them in order and Metal tracks these resources.
			 */
			pvgx_drop(encoder, "a fence in a render encoder");
			break;
		default:
			pvgx_fail(encoder, "a render command not sent by pvgtri or pvgcube", cmd->type);
			break;
		}
	}
}
