/* SPDX-License-Identifier: MIT */
/*
 * DXMT's blit and compute commands to pvg_wire's bytes; translate.c has
 * the render side and the helpers both use. See translate.h for the rules.
 */
#include <stdio.h>
#include <string.h>

#include "translate.h"
#include "translate_internal.h"
#include "pvg_wire_more.h"

void pvgx_blit_commands(struct pvgx_encoder *encoder, const struct wmtcmd_base *cmd)
{
	struct pvgx_info info;

	for (; cmd && !encoder->failed; cmd = NEXT(cmd)) {
		switch (cmd->type) {
		case WMTBlitCommandNop:
			break;
		case WMTBlitCommandCopyFromTextureToBuffer:
		case WMTBlitCommandCopyFromTextureToBufferWithBlitOption: {
			const struct wmtcmd_blit_copy_from_texture_to_buffer *c = (const void *)cmd;
			uint64_t origin[3] = { c->origin.x, c->origin.y, c->origin.z };
			uint64_t size[3] = { c->size.width, c->size.height, c->size.depth };
			unsigned level = c->level, slice = c->slice;
			uint32_t buffer, number;
			uint64_t base;

			if (cmd->type == WMTBlitCommandCopyFromTextureToBufferWithBlitOption) {
				const struct wmtcmd_blit_copy_from_texture_to_buffer_withblitoption *o =
					(const void *)cmd;

				if (o->options) {
					pvgx_fail(encoder, "a copy with blit options", o->options);
					break;
				}
				level = o->level;
			}
			if (!pvgx_look(encoder, c->src, PVGX_TEXTURE, &info) || pvgx_level_missing(encoder, &info, level))
				break;
			buffer = pvgx_buffer_number(encoder, c->dst, &base);
			if (!buffer)
				break;
			pvgx_use(encoder->resources, pvgx_listed(&info), 0);
			pvgx_use(encoder->resources, buffer, PVGX_RESOURCE_WRITTEN);
			/* pvgtri and pvgcube: the copy into the read-back buffer. */
			number = pvgx_texture_at(&info, &level, &slice);
			pvg_wire_copy_texture_to_buffer(encoder->wire, number, slice, level, origin, size,
							buffer, base + c->offset, c->bytes_per_row, c->bytes_per_image);
			encoder->commands++;
			break;
		}
		case WMTBlitCommandFillBuffer: {
			const struct wmtcmd_blit_fillbuffer *c = (const void *)cmd;
			uint32_t number;
			uint64_t base;

			/*
			 * DXMT zeroes a buffer it has just made. The driver gives
			 * buffer memory zeroed, so for a buffer nothing has
			 * written yet that has no effect; any other fill is the
			 * blit command for it.
			 */
			if (!pvgx_look(encoder, c->buffer, PVGX_BUFFER, &info))
				break;
			if (c->value == 0 && info.fresh) {
				pvgx_drop(encoder, "zeroing a fresh buffer, already zero");
				break;
			}
			number = pvgx_buffer_number(encoder, c->buffer, &base);
			pvgx_use(encoder->resources, number, PVGX_RESOURCE_WRITTEN);
			pvg_wire_fill_buffer(encoder->wire, number, base + c->offset, c->length, c->value);
			encoder->commands++;
			break;
		}
		case WMTBlitCommandCopyFromBufferToBuffer: {
			const struct wmtcmd_blit_copy_from_buffer_to_buffer *c = (const void *)cmd;
			uint64_t src_base, dst_base;
			uint32_t src = pvgx_buffer_number(encoder, c->src, &src_base);
			uint32_t dst = pvgx_buffer_number(encoder, c->dst, &dst_base);

			if (!src || !dst)
				break;
			pvgx_use(encoder->resources, dst, PVGX_RESOURCE_WRITTEN);
			pvg_wire_copy_buffer(encoder->wire, src, src_base + c->src_offset, dst, dst_base + c->dst_offset,
					     c->copy_length);
			encoder->commands++;
			break;
		}
		case WMTBlitCommandCopyFromBufferToTexture: {
			const struct wmtcmd_blit_copy_from_buffer_to_texture *c = (const void *)cmd;
			uint64_t origin[3] = { c->origin.x, c->origin.y, c->origin.z };
			uint64_t size[3] = { c->size.width, c->size.height, c->size.depth };
			uint64_t base;
			uint32_t buffer = pvgx_buffer_number(encoder, c->src, &base), number;
			unsigned level = c->level, slice = c->slice;

			if (!buffer || !pvgx_look(encoder, c->dst, PVGX_TEXTURE, &info) ||
			    pvgx_level_missing(encoder, &info, level))
				break;
			pvgx_use(encoder->resources, pvgx_listed(&info), PVGX_RESOURCE_WRITTEN);
			number = pvgx_texture_at(&info, &level, &slice);
			pvg_wire_copy_buffer_to_texture(encoder->wire, buffer, base + c->src_offset, c->bytes_per_row,
							c->bytes_per_image, size, number, slice, level, origin);
			encoder->commands++;
			break;
		}
		case WMTBlitCommandCopyFromTextureToTexture: {
			const struct wmtcmd_blit_copy_from_texture_to_texture *c = (const void *)cmd;
			uint64_t src_origin[3] = { c->src_origin.x, c->src_origin.y, c->src_origin.z };
			uint64_t dst_origin[3] = { c->dst_origin.x, c->dst_origin.y, c->dst_origin.z };
			uint64_t size[3] = { c->src_size.width, c->src_size.height, c->src_size.depth };
			struct pvgx_info dst;
			unsigned src_level = c->src_level, dst_level = c->dst_level;
			unsigned src_slice = c->src_slice, dst_slice = c->dst_slice;
			uint32_t src_number, dst_number;

			if (!pvgx_look(encoder, c->src, PVGX_TEXTURE, &info) || !pvgx_look(encoder, c->dst, PVGX_TEXTURE, &dst) ||
			    pvgx_level_missing(encoder, &info, src_level) || pvgx_level_missing(encoder, &dst, dst_level))
				break;
			pvgx_use(encoder->resources, pvgx_listed(&info), 0);
			pvgx_use(encoder->resources, pvgx_listed(&dst), PVGX_RESOURCE_WRITTEN);
			src_number = pvgx_texture_at(&info, &src_level, &src_slice);
			dst_number = pvgx_texture_at(&dst, &dst_level, &dst_slice);
			pvg_wire_copy_texture(encoder->wire, src_number, src_slice, src_level, src_origin, size,
					      dst_number, dst_slice, dst_level, dst_origin);
			encoder->commands++;
			break;
		}
		case WMTBlitCommandGenerateMipmaps: {
			const struct wmtcmd_blit_generate_mipmaps *c = (const void *)cmd;

			if (!pvgx_look(encoder, c->texture, PVGX_TEXTURE, &info))
				break;
			pvgx_use(encoder->resources, pvgx_listed(&info), PVGX_RESOURCE_WRITTEN);
			pvg_wire_generate_mipmaps(encoder->wire, info.number);
			encoder->commands++;
			break;
		}
		case WMTBlitCommandWaitForFence:
		case WMTBlitCommandUpdateFence:
			pvgx_drop(encoder, "a fence in a blit encoder");
			break;
		default:
			pvgx_fail(encoder, "a blit command not sent by pvgtri or pvgcube", cmd->type);
			break;
		}
	}
}

/* A compute buffer binding: listed (written, as compute may write it),
 * bound, and its base kept for later offsets. */
static void compute_buffer(struct pvgx_encoder *encoder, uint32_t number, uint64_t base, uint64_t offset,
			   unsigned index)
{
	if (index >= PVGX_BINDINGS) {
		pvgx_fail(encoder, "a compute buffer index", index);
		return;
	}
	pvgx_use(encoder->resources, number, PVGX_RESOURCE_WRITTEN);
	pvg_wire_set_buffer(encoder->wire, index, number, base + offset);
	encoder->compute_buffers[index] = number;
	encoder->compute_bases[index] = base;
	encoder->commands++;
}

void pvgx_compute_commands(struct pvgx_encoder *encoder, const struct wmtcmd_base *cmd)
{
	struct pvgx_info info;

	for (; cmd && !encoder->failed; cmd = NEXT(cmd)) {
		switch (cmd->type) {
		case WMTComputeCommandNop:
			break;
		case WMTComputeCommandSetPSO: {
			const struct wmtcmd_compute_setpso *c = (const void *)cmd;

			if (!pvgx_look(encoder, c->pso, PVGX_COMPUTE_PIPELINE, &info))
				break;
			pvg_wire_set_compute_pipeline(encoder->wire, info.number);
			encoder->group_threads[0] = c->threadgroup_size.width;
			encoder->group_threads[1] = c->threadgroup_size.height;
			encoder->group_threads[2] = c->threadgroup_size.depth;
			encoder->commands++;
			break;
		}
		case WMTComputeCommandSetBuffer: {
			const struct wmtcmd_compute_setbuffer *c = (const void *)cmd;
			uint64_t base;
			uint32_t number = pvgx_buffer_number(encoder, c->buffer, &base);

			if (number)
				compute_buffer(encoder, number, base, c->offset, c->index);
			break;
		}
		case WMTComputeCommandSetBufferOffset: {
			const struct wmtcmd_compute_setbufferoffset *c = (const void *)cmd;

			if (c->index >= PVGX_BINDINGS || !encoder->compute_buffers[c->index]) {
				pvgx_fail(encoder, "an offset for a compute buffer never set", c->index);
				break;
			}
			pvg_wire_set_buffer_offset(encoder->wire, c->index, encoder->compute_bases[c->index] + c->offset);
			encoder->commands++;
			break;
		}
		case WMTComputeCommandSetBytes: {
			const struct wmtcmd_compute_setbytes *c = (const void *)cmd;
			uint32_t number;
			uint64_t offset;

			if (!encoder->inline_bytes ||
			    !encoder->inline_bytes(encoder->context, c->bytes.ptr, c->length, &number, &offset)) {
				pvgx_fail(encoder, "compute bytes with no room for them", (unsigned)c->length);
				break;
			}
			compute_buffer(encoder, number, 0, offset, c->index);
			break;
		}
		case WMTComputeCommandSetTexture: {
			const struct wmtcmd_compute_settexture *c = (const void *)cmd;

			if (!pvgx_look(encoder, c->texture, PVGX_TEXTURE, &info))
				break;
			pvgx_use(encoder->resources, pvgx_listed(&info), PVGX_RESOURCE_WRITTEN);
			pvg_wire_set_texture(encoder->wire, c->index, info.number);
			encoder->commands++;
			break;
		}
		case WMTComputeCommandUseResource: {
			const struct wmtcmd_compute_useresource *c = (const void *)cmd;

			/* As in a render pass: a resource never made is left out. */
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
			pvg_wire_compute_use_resource(encoder->wire, info.number, c->usage);
			if (info.view)
				pvg_wire_compute_use_resource(encoder->wire, info.view, c->usage);
			if (info.listed)
				pvg_wire_compute_use_resource(encoder->wire, info.listed, c->usage);
			encoder->commands++;
			break;
		}
		case WMTComputeCommandDispatch: {
			const struct wmtcmd_compute_dispatch *c = (const void *)cmd;
			const uint64_t groups[3] = { c->size.width, c->size.height, c->size.depth };

			if (!encoder->group_threads[0]) {
				pvgx_fail(encoder, "a dispatch with no pipeline set", 0);
				break;
			}
			pvg_wire_dispatch(encoder->wire, groups, encoder->group_threads);
			encoder->commands++;
			break;
		}
		case WMTComputeCommandMemoryBarrier:
			/* This encoder runs its dispatches one after another. */
			pvgx_drop(encoder, "a memory barrier in a serial compute encoder");
			break;
		case WMTComputeCommandWaitForFence:
		case WMTComputeCommandUpdateFence:
			pvgx_drop(encoder, "a fence in a compute encoder");
			break;
		default:
			/* Dispatch by threads, indirect dispatch, indirect commands. */
			pvgx_fail(encoder, "a compute command not carried yet", cmd->type);
			break;
		}
	}
}
