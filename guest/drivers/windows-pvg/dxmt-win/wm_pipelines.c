// SPDX-License-Identifier: MIT
//
// winemetal over ruxnpvg: samplers and depth states, libraries, functions,
// pipelines, and events.

#include "wm_internal.h"

// ---------------------------------------------------------------------
// States.

obj_handle_t MTLDevice_newSamplerState(obj_handle_t dev, struct WMTSamplerInfo *info)
{
	static UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	struct pvg_wire_sampler s;
	obj_handle_t h = 0;
	uint32_t number;
	size_t length;

	CALLED();
	(void)dev;
	EnterCriticalSection(&lock);
	pvg_wire_sampler_init(&s);
	s.min_filter = info->min_filter;
	s.mag_filter = info->mag_filter;
	s.mip_filter = info->mip_filter;
	s.address_s = info->s_address_mode;
	s.address_t = info->t_address_mode;
	s.address_r = info->r_address_mode;
	s.border_colour = info->border_color;
	s.max_anisotropy = info->max_anisotroy ? info->max_anisotroy : 1;
	s.compare = info->compare_function;
	s.normalized = info->normalized_coords;
	s.lod_average = info->lod_average;
	s.argument_buffers = info->support_argument_buffers;
	s.lod_min = info->lod_min_clamp;
	s.lod_max = info->lod_max_clamp;
	number = wm_take_number();
	length = pvg_wire_sampler(description, sizeof(description), number, &s);
	if (number && length &&
	    create_object(number, RUXNPVG_OBJECT_STATE, NULL, 0, description, (ULONG)length)) {
		uint64_t id = 0;

		// A sampler's resource id is the host's once a command names it.
		if (!wm_place(number, WM_PLACE_SAMPLER, 0, &id))
			pvg_log("FAILED: the placement of sampler %u", number);
		h = new_object(K_SAMPLER);
		if (h) {
			objects[h].u.state.number = number;
			info->gpu_resource_id = id;
		}
	}
	LeaveCriticalSection(&lock);
	if (!h)
		pvg_log("FAILED: newSamplerState");
	return h;
}

static void stencil(struct pvg_wire_stencil *to, const struct WMTStencilInfo *from)
{
	to->compare = from->stencil_compare_function;
	to->stencil_fail = from->stencil_fail_op;
	to->depth_fail = from->depth_fail_op;
	to->pass = from->depth_stencil_pass_op;
	to->read_mask = from->read_mask;
	to->write_mask = from->write_mask;
}

obj_handle_t MTLDevice_newDepthStencilState(obj_handle_t dev, const struct WMTDepthStencilInfo *info)
{
	static UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	struct pvg_wire_depth_stencil s;
	obj_handle_t h = 0;
	uint32_t number;
	size_t length;

	CALLED();
	(void)dev;
	EnterCriticalSection(&lock);
	pvg_wire_depth_stencil_init(&s);
	s.compare = info->depth_compare_function;
	s.write = info->depth_write_enabled;
	if (info->front_stencil.enabled) {
		s.has_front = true;
		stencil(&s.front, &info->front_stencil);
	}
	if (info->back_stencil.enabled) {
		s.has_back = true;
		stencil(&s.back, &info->back_stencil);
	}
	number = wm_take_number();
	length = pvg_wire_depth_stencil(description, sizeof(description), number, &s);
	if (number && length &&
	    create_object(number, RUXNPVG_OBJECT_STATE, NULL, 0, description, (ULONG)length)) {
		h = new_object(K_DEPTH_STENCIL);
		if (h)
			objects[h].u.state.number = number;
	}
	LeaveCriticalSection(&lock);
	if (!h)
		pvg_log("FAILED: newDepthStencilState");
	return h;
}

// ---------------------------------------------------------------------
// Libraries, functions and pipelines.

obj_handle_t DispatchData_alloc_init(uint64_t native_ptr, uint64_t length)
{
	obj_handle_t h;
	void *copy = length ? malloc(length) : NULL;

	CALLED();
	if (length && !copy)
		return 0;
	if (length)
		memcpy(copy, (const void *)(ULONG_PTR)native_ptr, length);
	EnterCriticalSection(&lock);
	h = new_object(K_DATA);
	if (h) {
		objects[h].u.data.data = copy;
		objects[h].u.data.length = length;
	}
	LeaveCriticalSection(&lock);
	if (!h)
		free(copy);
	return h;
}

obj_handle_t MTLDevice_newLibrary(obj_handle_t dev, obj_handle_t data, obj_handle_t *err_out)
{
	struct object *d;
	struct memory m;
	obj_handle_t h = 0;

	CALLED();
	(void)dev;
	EnterCriticalSection(&lock);
	d = get(data, K_DATA);
	if (!d) {
		set_error(err_out, "newLibrary: no data");
	} else if (d->u.data.length == 0) {
		// DXMT's helper library (dxmt_command.metal) is left out of
		// this build: an empty one stands for it, and what is made from
		// it is a placeholder that fails if it is ever used.
		h = new_object(K_LIBRARY);
		if (h)
			objects[h].u.library.placeholder = true;
		note_once("DXMT's helper library: placeholders");
	} else if (wm_shader_standin() && !wm_shader_standin_library(d->u.data.data, d->u.data.length)) {
		set_error(err_out, "newLibrary: not one of the triangle's two libraries");
	} else if (allocate(d->u.data.length, WM_USE_LIBRARY, &m)) {
		memcpy(address(&m), d->u.data.data, d->u.data.length);
		h = new_object(K_LIBRARY);
		if (h)
			objects[h].u.library.memory = m;
	}
	LeaveCriticalSection(&lock);
	return h;
}

obj_handle_t MTLLibrary_newFunction(obj_handle_t library, const char *name)
{
	static UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	struct object *l;
	obj_handle_t h = 0;
	uint32_t number;
	size_t length;

	CALLED();
	EnterCriticalSection(&lock);
	l = get(library, K_LIBRARY);
	if (!l) {
		pvg_log("FAILED: newFunction %s: no library", name);
	} else if (l->u.library.placeholder) {
		// DXMT's helper library: a function of ours where we have one.
		uint32_t helper = wm_helper_function(name);

		h = new_object(K_FUNCTION);
		if (h && helper)
			objects[h].u.function.number = helper;
		else if (h)
			objects[h].u.function.placeholder = true;
	} else if (l->u.library.function) {
		h = new_object(K_FUNCTION);
		if (h)
			objects[h].u.function.number = l->u.library.function;
	} else {
		// A library holds one function; its name is not on the wire.
		number = wm_take_number();
		length = pvg_wire_function(description, sizeof(description), number, 0,
					   (uint32_t)l->u.library.memory.length);
		if (number && length &&
		    create_object(number, RUXNPVG_OBJECT_FUNCTION, &l->u.library.memory, 0,
				  description + PVG_WIRE_HEAD_FUNCTION, (ULONG)(length - PVG_WIRE_HEAD_FUNCTION))) {
			l->u.library.function = number;
			h = new_object(K_FUNCTION);
			if (h)
				objects[h].u.function.number = number;
		}
	}
	LeaveCriticalSection(&lock);
	return h;
}

static obj_handle_t placeholder(void)
{
	return new_object(K_PLACEHOLDER);
}

obj_handle_t MTLDevice_newComputePipelineState(obj_handle_t dev, const struct WMTComputePipelineInfo *info,
					       obj_handle_t *err_out)
{
	struct object *f;
	obj_handle_t h = 0;

	static UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	uint32_t number;
	size_t length;

	CALLED();
	(void)dev;
	EnterCriticalSection(&lock);
	f = get(info->compute_function, K_FUNCTION);
	if (f && f->u.function.placeholder) {
		h = placeholder();
	} else if (!f || !f->u.function.number) {
		set_error(err_out, "newComputePipelineState: no function");
	} else {
		// Operation 11 with the function's number, a state object as the
		// render pipeline is; the threads of a group come with each
		// dispatch's pipeline setting (translate_blit.c).
		number = wm_take_number();
		length = pvg_wire_compute_pipeline(description, sizeof(description), number, f->u.function.number);
		if (number && length &&
		    create_object(number, RUXNPVG_OBJECT_STATE, NULL, 0, description, (ULONG)length)) {
			h = new_object(K_COMPUTE_PIPELINE);
			if (h)
				objects[h].u.pipeline.number = number;
		} else {
			set_error(err_out, "newComputePipelineState: the device's object not made");
		}
	}
	LeaveCriticalSection(&lock);
	return h;
}

obj_handle_t MTLDevice_newRenderPipelineState(obj_handle_t dev, const struct WMTRenderPipelineInfo *info,
					      obj_handle_t *err_out)
{
	static UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	static struct pvg_wire_render_pipeline pipeline;
	struct object *vertex, *fragment;
	obj_handle_t h = 0;
	bool blending = false;
	uint32_t number;
	size_t length;
	unsigned i;

	CALLED();
	(void)dev;
	EnterCriticalSection(&lock);
	vertex = get(info->vertex_function, K_FUNCTION);
	fragment = info->fragment_function ? get(info->fragment_function, K_FUNCTION) : NULL;
	if (vertex && vertex->u.function.placeholder) {
		h = placeholder();
		goto out;
	}
	if (!vertex || !fragment || fragment->u.function.placeholder || !info->rasterization_enabled ||
	    (info->raster_sample_count > 1 && info->raster_sample_count != 2 && info->raster_sample_count != 4 &&
	     info->raster_sample_count != 8) ||
	    info->alpha_to_coverage_enabled || info->logic_operation_enabled) {
		set_error(err_out, "newRenderPipelineState: only a vertex and a fragment function of ours, "
				   "1, 2, 4 or 8 samples, no alpha to coverage or logic operation");
		goto out;
	}
	pvg_wire_render_pipeline_init(&pipeline);
	pipeline.vertex_function = vertex->u.function.number;
	pipeline.fragment_function = fragment->u.function.number;
	for (i = 0; i < PVG_WIRE_MOST_COLOURS; i++) {
		const struct WMTColorAttachmentBlendInfo *c = &info->colors[i];

		if (!c->pixel_format)
			continue;
		pipeline.colours[i].format = c->pixel_format;
		pipeline.colours[i].write_mask = c->write_mask;
		// Blending as the attachment describes it: Metal's factors and
		// operations, which winemetal.h numbers as Metal does.
		if (c->blending_enabled) {
			pipeline.colours[i].blending = true;
			pipeline.colours[i].source_rgb = c->src_rgb_blend_factor;
			pipeline.colours[i].destination_rgb = c->dst_rgb_blend_factor;
			pipeline.colours[i].rgb_operation = c->rgb_blend_operation;
			pipeline.colours[i].source_alpha = c->src_alpha_blend_factor;
			pipeline.colours[i].destination_alpha = c->dst_alpha_blend_factor;
			pipeline.colours[i].alpha_operation = c->alpha_blend_operation;
			blending = true;
		}
	}
	// Setting 4 of the descriptor (helper scene msaa-small); 1 is Metal's default.
	if (info->raster_sample_count > 1)
		pipeline.raster_sample_count = info->raster_sample_count;
	pipeline.depth_format = wm_depth_format(info->depth_pixel_format);
	pipeline.stencil_format = wm_stencil_format(info->stencil_pixel_format);
	number = wm_take_number();
	length = pvg_wire_render_pipeline(description, sizeof(description), number, &pipeline);
	if (number && length &&
	    create_object(number, RUXNPVG_OBJECT_STATE, NULL, 0, description, (ULONG)length)) {
		h = new_object(K_RENDER_PIPELINE);
		if (h) {
			objects[h].u.pipeline.number = number;
			objects[h].u.pipeline.blending = blending;
		}
	} else {
		set_error(err_out, "newRenderPipelineState: the object was not made");
	}
out:
	LeaveCriticalSection(&lock);
	return h;
}

// ---------------------------------------------------------------------
// Events. Commit waits for the GPU, so an event's value is set when the
// command buffer that signals it is committed.


struct wait waits[MOST_WAITS];
static unsigned wait_count;

void set_event_value(obj_handle_t event, uint64_t value)
{
	struct object *e = get(event, K_EVENT);
	unsigned i;

	if (!e)
		return;
	if (value > e->u.event.value)
		e->u.event.value = value;
	for (i = 0; i < wait_count;) {
		if (waits[i].event == event && waits[i].value <= e->u.event.value) {
			SetEvent(waits[i].handle);
			waits[i] = waits[--wait_count];
		} else {
			i++;
		}
	}
}

obj_handle_t MTLDevice_newSharedEvent(obj_handle_t dev)
{
	obj_handle_t h;

	CALLED();
	(void)dev;
	EnterCriticalSection(&lock);
	h = new_object(K_EVENT);
	LeaveCriticalSection(&lock);
	return h;
}

uint64_t MTLSharedEvent_signaledValue(obj_handle_t event)
{
	struct object *e;
	uint64_t v;

	CALLED();
	EnterCriticalSection(&lock);
	e = get(event, K_EVENT);
	v = e ? e->u.event.value : 0;
	LeaveCriticalSection(&lock);
	return v;
}

void MTLSharedEvent_signalValue(obj_handle_t event, uint64_t value)
{
	CALLED();
	EnterCriticalSection(&lock);
	set_event_value(event, value);
	LeaveCriticalSection(&lock);
}

bool MTLSharedEvent_waitUntilSignaledValue(obj_handle_t event, uint64_t value, uint64_t timeout)
{
	CALLED();
	(void)timeout;
	return MTLSharedEvent_signaledValue(event) >= value;
}

void MTLSharedEvent_setWin32EventAtValue(obj_handle_t event, obj_handle_t listener, void *nt_event_handle,
					 uint64_t at_value)
{
	struct object *e;

	CALLED();
	(void)listener;
	EnterCriticalSection(&lock);
	e = get(event, K_EVENT);
	if (!e) {
		pvg_log("FAILED: setWin32EventAtValue on no event");
	} else if (e->u.event.value >= at_value) {
		SetEvent(nt_event_handle);
	} else if (wait_count < MOST_WAITS) {
		waits[wait_count].event = event;
		waits[wait_count].handle = nt_event_handle;
		waits[wait_count].value = at_value;
		wait_count++;
	} else {
		pvg_log("FAILED: too many events waited for");
	}
	LeaveCriticalSection(&lock);
}

obj_handle_t SharedEventListener_create(void)
{
	obj_handle_t h;

	CALLED();
	EnterCriticalSection(&lock);
	h = new_object(K_LISTENER);
	LeaveCriticalSection(&lock);
	return h;
}

void SharedEventListener_start(obj_handle_t listener)
{
	CALLED();
	(void)listener;
}

void SharedEventListener_destroy(obj_handle_t listener)
{
	CALLED();
	(void)listener;
}

obj_handle_t MTLDevice_newEvent(obj_handle_t dev)
{
	CALLED();
	return MTLDevice_newSharedEvent(dev);
}

obj_handle_t MTLDevice_newFence(obj_handle_t dev)
{
	obj_handle_t h;

	CALLED();
	(void)dev;
	EnterCriticalSection(&lock);
	h = new_object(K_FENCE);
	LeaveCriticalSection(&lock);
	return h;
}
