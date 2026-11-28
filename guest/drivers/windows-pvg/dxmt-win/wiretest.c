// SPDX-License-Identifier: MIT
//
// wiretest: runs on the Mac. Feeds translate.c the render pass and command
// lists DXMT encodes for the triangle (as winemetal.h lays them out) and
// checks that the bytes it writes are exactly the bytes pvgtri writes for
// the same triangle with pvg_wire. Also checks that the pipeline's
// description winemetal_pvg.c makes from DXMT's pipeline is pvgtri's, and
// that what this slice does not carry fails instead of being written.
//
//   wiretest     (exit status 0 when every check passes)

#include <stdio.h>
#include <string.h>

#include "translate.h"

enum { TARGET = 1, READBACK, VERTEX, FRAGMENT, PIPELINE };   // pvgtri's numbers
enum { H_TARGET = 11, H_READBACK, H_PIPELINE, H_PLACEHOLDER }; // handles DXMT would hold

static bool lookup(void *context, obj_handle_t handle, struct pvgx_info *info)
{
	(void)context;
	memset(info, 0, sizeof(*info));
	switch (handle) {
	case H_TARGET:
		info->kind = PVGX_TEXTURE;
		info->number = TARGET;
		info->width = info->height = 256;
		return true;
	case H_READBACK:
		info->kind = PVGX_BUFFER;
		info->number = READBACK;
		return true;
	case H_PIPELINE:
		info->kind = PVGX_RENDER_PIPELINE;
		info->number = PIPELINE;
		return true;
	case H_PLACEHOLDER:
		info->kind = PVGX_PLACEHOLDER;
		return true;
	default:
		return false;
	}
}

static unsigned notes;

static void note(void *context, const char *what)
{
	(void)context;
	printf("  dropped: %s\n", what);
	notes++;
}

static int failures;

static void check(bool ok, const char *what)
{
	printf("%s: %s\n", ok ? "pass" : "FAIL", what);
	if (!ok)
		failures++;
}

static uint8_t ours[4096], theirs[4096];

// pvgtri's two segments, written as pvgtri writes them.
static size_t pvgtri_bytes(size_t *first)
{
	static struct pvg_wire_pass pass;
	struct pvg_wire wire;
	const uint64_t origin[3] = { 0, 0, 0 }, size[3] = { 256, 256, 1 };

	pvg_wire_init(&wire, theirs, sizeof(theirs));
	pvg_wire_pass_init(&pass);
	pass.colours[0].texture = TARGET;
	pass.colours[0].load = 2;
	pass.colours[0].store = 1;
	pass.colours[0].clear[0] = pass.colours[0].clear[1] = pass.colours[0].clear[2] = 0.25;
	pass.colours[0].clear[3] = 1.0;
	pvg_wire_begin_render(&wire, &pass);
	pvg_wire_set_render_pipeline(&wire, PIPELINE);
	pvg_wire_draw(&wire, 3, 0, 3);
	*first = pvg_wire_end(&wire);
	pvg_wire_begin_blit(&wire);
	pvg_wire_copy_texture_to_buffer(&wire, TARGET, 0, 0, origin, size, READBACK, 0, 256 * 4, 256 * 256 * 4);
	pvg_wire_end(&wire);
	return wire.failed ? 0 : wire.used;
}

#define LINK(a, b) ((a).next.ptr = (void *)&(b))

int main(void)
{
	static struct WMTRenderPassInfo pass;
	struct wmtcmd_render_setviewport viewport;
	struct wmtcmd_render_setscissorrect scissor;
	struct wmtcmd_render_setrasterizerstate raster;
	struct wmtcmd_render_setpso pso;
	struct wmtcmd_render_draw draw;
	struct wmtcmd_blit_copy_from_texture_to_buffer copy;
	struct pvgx_resources resources;
	struct pvgx_encoder e;
	struct pvg_wire wire;
	size_t first, theirs_length, mark;

	theirs_length = pvgtri_bytes(&first);
	check(theirs_length > 0, "pvgtri's bytes written");

	// The render pass DXMT makes for ClearRenderTargetView and Draw.
	memset(&pass, 0, sizeof(pass));
	pass.colors[0].texture = H_TARGET;
	pass.colors[0].load_action = WMTLoadActionClear;
	pass.colors[0].store_action = WMTStoreActionStore;
	pass.colors[0].clear_color = (struct WMTClearColor){ 0.25, 0.25, 0.25, 1.0 };
	pass.render_target_width = 256;
	pass.render_target_height = 256;
	pass.render_target_array_length = 0;

	memset(&viewport, 0, sizeof(viewport));
	viewport.type = WMTRenderCommandSetViewport;
	viewport.viewport = (struct WMTViewport){ 0, 0, 256, 256, 0, 1 };
	memset(&scissor, 0, sizeof(scissor));
	scissor.type = WMTRenderCommandSetScissorRect;
	scissor.scissor_rect = (struct WMTScissorRect){ 0, 0, 256, 256 };
	memset(&raster, 0, sizeof(raster));
	raster.type = WMTRenderCommandSetRasterizerState;
	raster.cull_mode = WMTCullModeNone;
	raster.winding = WMTWindingCounterClockwise;
	memset(&pso, 0, sizeof(pso));
	pso.type = WMTRenderCommandSetPSO;
	pso.pso = H_PIPELINE;
	memset(&draw, 0, sizeof(draw));
	draw.type = WMTRenderCommandDraw;
	draw.primitive_type = WMTPrimitiveTypeTriangle;
	draw.vertex_start = 0;
	draw.vertex_count = 3;
	draw.instance_count = 1;
	LINK(viewport, scissor);
	LINK(scissor, raster);
	LINK(raster, pso);
	LINK(pso, draw);

	memset(&copy, 0, sizeof(copy));
	copy.type = WMTBlitCommandCopyFromTextureToBuffer;
	copy.src = H_TARGET;
	copy.size = (struct WMTSize){ 256, 256, 1 };
	copy.dst = H_READBACK;
	copy.bytes_per_row = 256 * 4;
	copy.bytes_per_image = 256 * 256 * 4;

	memset(&resources, 0, sizeof(resources));
	pvg_wire_init(&wire, ours, sizeof(ours));
	pvgx_encoder_init(&e, &wire, lookup, note, NULL, &resources);
	pvgx_begin_render(&e, &pass);
	pvgx_render_commands(&e, (const struct wmtcmd_base *)&viewport);
	check(!e.failed, e.failed ? e.why : "the render encoder takes DXMT's triangle");
	check(pvg_wire_end(&wire) == first, "the render segment is as long as pvgtri's");
	pvgx_encoder_init(&e, &wire, lookup, note, NULL, &resources);
	pvg_wire_begin_blit(&wire);
	pvgx_blit_commands(&e, (const struct wmtcmd_base *)&copy);
	check(!e.failed, e.failed ? e.why : "the blit encoder takes DXMT's copy");
	pvg_wire_end(&wire);
	check(!wire.failed && wire.used == theirs_length && !memcmp(ours, theirs, theirs_length),
	      "both segments are byte for byte pvgtri's");
	check(notes == 4, "four commands dropped as Metal's defaults (pass size, viewport, scissor, rasterizer)");
	check(resources.count == 2 && resources.ids[0] == TARGET && resources.flags[0] == PVGX_RESOURCE_WRITTEN &&
		      resources.ids[1] == READBACK && resources.flags[1] == PVGX_RESOURCE_WRITTEN,
	      "command 67's resources are pvgtri's: the target and the read-back buffer, both written");

	// The pipeline description: DXMT's write mask of all is pvg_wire's default.
	{
		static uint8_t a[PVG_WIRE_MOST_DESCRIPTION], b[PVG_WIRE_MOST_DESCRIPTION];
		static struct pvg_wire_render_pipeline p, q;
		size_t la, lb;

		pvg_wire_render_pipeline_init(&p);
		p.vertex_function = VERTEX;
		p.fragment_function = FRAGMENT;
		p.colours[0].format = 80;
		la = pvg_wire_render_pipeline(a, sizeof(a), PIPELINE, &p);
		pvg_wire_render_pipeline_init(&q);
		q.vertex_function = VERTEX;
		q.fragment_function = FRAGMENT;
		q.colours[0].format = WMTPixelFormatBGRA8Unorm;
		q.colours[0].write_mask = WMTColorWriteMaskAll;
		q.depth_format = WMTPixelFormatInvalid;
		q.stencil_format = WMTPixelFormatInvalid;
		lb = pvg_wire_render_pipeline(b, sizeof(b), PIPELINE, &q);
		check(la && la == lb && !memcmp(a, b, la), "the pipeline's description is pvgtri's");
	}

	// What the triangle does not set is written as its own command; what
	// the wire has no command for fails and writes nothing.
	viewport.viewport.width = 128;
	viewport.next.ptr = NULL;
	pvg_wire_init(&wire, ours, sizeof(ours));
	pvgx_encoder_init(&e, &wire, lookup, note, NULL, &resources);
	pvgx_begin_render(&e, &pass);
	mark = wire.used;
	pvgx_render_commands(&e, (const struct wmtcmd_base *)&viewport);
	check(!e.failed && wire.used > mark, "a viewport other than the attachment's is written");

	raster.cull_mode = WMTCullModeBack;
	raster.next.ptr = NULL;
	pvgx_encoder_init(&e, &wire, lookup, note, NULL, &resources);
	pvgx_render_commands(&e, (const struct wmtcmd_base *)&raster);
	check(!e.failed, "culling is written");

	pso.pso = H_PLACEHOLDER;
	pso.next.ptr = NULL;
	pvgx_encoder_init(&e, &wire, lookup, note, NULL, &resources);
	pvgx_render_commands(&e, (const struct wmtcmd_base *)&pso);
	check(e.failed, "a placeholder pipeline (DXMT's helper library) fails");

	draw.instance_count = 2;
	draw.next.ptr = NULL;
	pvgx_encoder_init(&e, &wire, lookup, note, NULL, &resources);
	pvgx_render_commands(&e, (const struct wmtcmd_base *)&draw);
	check(!e.failed, "an instanced draw is written");

	{
		struct wmtcmd_render_nop odd;

		memset(&odd, 0, sizeof(odd));
		odd.type = WMTRenderCommandDrawIndirect;
		pvgx_encoder_init(&e, &wire, lookup, note, NULL, &resources);
		pvgx_render_commands(&e, (const struct wmtcmd_base *)&odd);
		check(e.failed, "an indirect draw fails");
	}

	printf(failures ? "wiretest: %d checks failed\n" : "wiretest: all checks pass\n", failures);
	return failures ? 1 : 0;
}
