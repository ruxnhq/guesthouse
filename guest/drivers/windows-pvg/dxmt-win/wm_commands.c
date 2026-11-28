// SPDX-License-Identifier: MIT
//
// winemetal over ruxnpvg: queues, command buffers, encoders, and the
// submits that hand their segments to the device.

#include "wm_internal.h"

// ---------------------------------------------------------------------
// Queues, command buffers and encoders.

obj_handle_t MTLDevice_newCommandQueue(obj_handle_t dev, uint64_t maxCommandBufferCount)
{
	obj_handle_t h;

	CALLED();
	(void)dev;
	(void)maxCommandBufferCount;
	EnterCriticalSection(&lock);
	h = new_object(K_QUEUE);
	LeaveCriticalSection(&lock);
	return h;
}

obj_handle_t MTLCommandQueue_commandBuffer(obj_handle_t queue)
{
	struct command_buffer *cb;
	obj_handle_t h = 0;

	CALLED();
	(void)queue;
	EnterCriticalSection(&lock);
	cb = calloc(1, sizeof(*cb));
	if (cb && command_memory_count)
		cb->memory = command_memory[--command_memory_count];
	if (cb && (cb->memory.chunk || allocate(COMMAND_BYTES, WM_USE_COMMANDS, &cb->memory))) {
		pvg_wire_init(&cb->wire, address(&cb->memory), COMMAND_BYTES);
		cb->inline_low = COMMAND_BYTES;
		cb->status = WMTCommandBufferStatusNotEnqueued;
		h = new_object(K_COMMAND_BUFFER);
		if (h) {
			objects[h].u.command_buffer = cb;
			autorelease(h);
		}
	}
	// With no handle for it, its memory goes back to the pool if there is
	// room, and the rest is freed.
	if (cb && !h) {
		if (cb->memory.chunk && command_memory_count < ARRAYSIZE(command_memory))
			command_memory[command_memory_count++] = cb->memory;
		free(cb);
	}
	LeaveCriticalSection(&lock);
	if (!h)
		pvg_log("FAILED: commandBuffer");
	return h;
}

static bool lookup(void *context, obj_handle_t handle, struct pvgx_info *info)
{
	struct object *o;

	memset(info, 0, sizeof(*info));
	if (!handle || handle >= object_count)
		return false;
	o = &objects[handle];
	// What the commands name is held until the command buffer is done
	// (run rn54: a dynamic vertex buffer given back while a draw still
	// named it served the next one, and the draw read the next one's
	// vertices).
	if (context && (o->kind == K_BUFFER || o->kind == K_TEXTURE))
		wm_hold(context, handle);
	switch (o->kind) {
	case K_BUFFER:
		info->kind = PVGX_BUFFER;
		info->number = o->u.buffer.number;
		info->offset = o->u.buffer.memory.offset;
		info->fresh = o->u.buffer.fresh;
		return true;
	case K_TEXTURE:
		info->kind = PVGX_TEXTURE;
		info->number = o->u.texture.number;
		info->view = o->u.texture.view_number;
		info->listed = o->u.texture.listed;
		info->base = o->u.texture.base;
		info->base_level = o->u.texture.base_level;
		info->base_slice = o->u.texture.base_slice;
		info->levels = o->u.texture.device_levels;
		info->width = o->u.texture.info.width;
		info->height = o->u.texture.info.height;
		info->stencil = o->u.texture.stencil_number;
		return true;
	case K_RENDER_PIPELINE:
		info->kind = PVGX_RENDER_PIPELINE;
		info->number = o->u.pipeline.number;
		info->blending = o->u.pipeline.blending;
		return true;
	case K_COMPUTE_PIPELINE:
		info->kind = PVGX_COMPUTE_PIPELINE;
		info->number = o->u.pipeline.number;
		return true;
	case K_DEPTH_STENCIL:
		info->kind = PVGX_DEPTH_STENCIL;
		info->number = o->u.state.number;
		return true;
	case K_SAMPLER:
		info->kind = PVGX_SAMPLER;
		info->number = o->u.state.number;
		return true;
	case K_PLACEHOLDER:
		info->kind = PVGX_PLACEHOLDER;
		return true;
	default:
		info->kind = PVGX_OTHER;
		return true;
	}
}

static void note(void *context, const char *what)
{
	(void)context;
	note_once(what);
}


// Bytes a command carries, at the top of the command buffer's memory going
// down, 256 bytes apart as Metal aligns buffer offsets; the writer's room
// ends below them so no segment is written over them. Under the lock.
#define INLINE_ALIGN 256u

static bool inline_bytes(void *context, const void *bytes, uint64_t length, uint32_t *number, uint64_t *offset)
{
	struct command_buffer *cb = context;
	uint64_t low;

	if (!length || length > cb->inline_low)
		return false;
	low = (cb->inline_low - length) & ~(uint64_t)(INLINE_ALIGN - 1);
	if (low < cb->wire.used)
		return false;
	memcpy((UCHAR *)address(&cb->memory) + low, bytes, length);
	cb->inline_low = low;
	cb->wire.size = low;
	*number = cb->memory.chunk->number;
	*offset = cb->memory.offset + low;
	return true;
}

static obj_handle_t begin_encoder(obj_handle_t command_buffer, enum encoder_kind kind,
				  struct WMTRenderPassInfo *pass)
{
	struct object *c;
	struct command_buffer *cb;
	struct encoder *e;
	obj_handle_t h = 0;

	EnterCriticalSection(&lock);
	c = get(command_buffer, K_COMMAND_BUFFER);
	if (!c)
		goto out;
	cb = c->u.command_buffer;
	if (cb->open_encoder) {
		pvg_log("FAILED: an encoder begun while another is open");
		cb->failed = true;
		goto out;
	}
	e = calloc(1, sizeof(*e));
	if (!e)
		goto out;
	// A submit takes 16 command buffers at most, and so many resources:
	// what the command buffer holds so far goes to the device now, and the
	// rest follows at commit. Each submit is waited for, so the order holds.
	if (cb->segments == MOST_SEGMENTS || cb->resources.count > PVGX_MOST_RESOURCES * 3 / 4) {
		if (!cb->failed && !submit(cb))
			cb->failed = true;
		cb->segments = 0;
		memset(&cb->resources, 0, sizeof(cb->resources));
	}
	e->command_buffer = command_buffer;
	e->kind = kind;
	e->before = cb->wire;
	e->segment = cb->wire.used;
	pvgx_encoder_init(&e->x, &cb->wire, lookup, note, cb, &cb->resources);
	e->x.inline_bytes = inline_bytes;
	if (kind == E_RENDER)
		pvgx_begin_render(&e->x, pass);
	else if (kind == E_BLIT)
		pvg_wire_begin_blit(&cb->wire);
	else
		pvg_wire_begin_compute(&cb->wire);
	h = new_object(K_ENCODER);
	if (h) {
		autorelease(h);
		objects[h].u.encoder = e;
		cb->open_encoder = h;
	}
out:
	LeaveCriticalSection(&lock);
	return h;
}

obj_handle_t MTLCommandBuffer_renderCommandEncoder(obj_handle_t cmdbuf, struct WMTRenderPassInfo *info)
{
	CALLED();
	return begin_encoder(cmdbuf, E_RENDER, info);
}

obj_handle_t MTLCommandBuffer_blitCommandEncoder(obj_handle_t cmdbuf)
{
	CALLED();
	return begin_encoder(cmdbuf, E_BLIT, NULL);
}

obj_handle_t MTLCommandBuffer_computeCommandEncoder(obj_handle_t cmdbuf, bool concurrent)
{
	CALLED();
	(void)concurrent;
	return begin_encoder(cmdbuf, E_COMPUTE, NULL);
}

static struct encoder *encoder_of(obj_handle_t handle)
{
	struct object *o = get(handle, K_ENCODER);

	return o && !o->u.encoder->ended ? o->u.encoder : NULL;
}

void MTLRenderCommandEncoder_encodeCommands(obj_handle_t encoder, const struct wmtcmd_base *cmd_head)
{
	struct encoder *e;

	CALLED();
	EnterCriticalSection(&lock);
	e = encoder_of(encoder);
	if (e && e->kind == E_RENDER)
		pvgx_render_commands(&e->x, cmd_head);
	LeaveCriticalSection(&lock);
}

void MTLBlitCommandEncoder_encodeCommands(obj_handle_t encoder, const struct wmtcmd_base *cmd_head)
{
	struct encoder *e;

	CALLED();
	EnterCriticalSection(&lock);
	e = encoder_of(encoder);
	if (e && e->kind == E_BLIT)
		pvgx_blit_commands(&e->x, cmd_head);
	LeaveCriticalSection(&lock);
}

void MTLComputeCommandEncoder_encodeCommands(obj_handle_t encoder, const struct wmtcmd_base *cmd_head)
{
	struct encoder *e;

	CALLED();
	EnterCriticalSection(&lock);
	e = encoder_of(encoder);
	if (e && e->kind == E_COMPUTE)
		pvgx_compute_commands(&e->x, cmd_head);
	LeaveCriticalSection(&lock);
}

void MTLCommandEncoder_endEncoding(obj_handle_t encoder)
{
	struct encoder *e;
	struct command_buffer *cb;
	size_t size;

	CALLED();
	EnterCriticalSection(&lock);
	e = encoder_of(encoder);
	if (!e)
		goto out;
	e->ended = true;
	cb = objects[e->command_buffer].u.command_buffer;
	cb->open_encoder = 0;
	if (e->x.failed) {
		pvg_log("FAILED: %s encoder: %s; the command buffer will not be submitted",
			e->kind == E_RENDER ? "render" : e->kind == E_BLIT ? "blit" : "compute", e->x.why);
		cb->failed = true;
		cb->wire = e->before;
		// The bytes commands carry stay below the writer's room.
		cb->wire.size = cb->inline_low;
		goto out;
	}
	// A blit or compute encoder with nothing in it is left out, as
	// neither pvgtri nor pvgcube sends an empty one; a render pass is
	// kept, as its clear is its work.
	if (e->kind != E_RENDER && !e->x.commands) {
		cb->wire = e->before;
		// The bytes commands carry stay below the writer's room.
		cb->wire.size = cb->inline_low;
		goto out;
	}
	size = pvg_wire_end(&cb->wire);
	if (!size || cb->wire.failed || cb->segments == MOST_SEGMENTS) {
		pvg_log("FAILED: the command buffer's segments did not fit");
		cb->failed = true;
		goto out;
	}
	cb->segment_offset[cb->segments] = cb->memory.offset + e->segment;
	cb->segment_size[cb->segments] = size;
	cb->segments++;
out:
	LeaveCriticalSection(&lock);
}

void MTLCommandEncoder_setLabel(obj_handle_t encoder, obj_handle_t label)
{
	CALLED();
	(void)encoder;
	(void)label;
}

void MTLCommandBuffer_encodeSignalEvent(obj_handle_t cmdbuf, obj_handle_t event, uint64_t value)
{
	struct object *c;
	struct command_buffer *cb;

	CALLED();
	EnterCriticalSection(&lock);
	c = get(cmdbuf, K_COMMAND_BUFFER);
	if (c) {
		cb = c->u.command_buffer;
		if (cb->signals < MOST_SIGNALS) {
			cb->signal_event[cb->signals] = event;
			cb->signal_value[cb->signals] = value;
			cb->signals++;
		} else {
			pvg_log("FAILED: too many events signalled by one command buffer");
		}
	}
	LeaveCriticalSection(&lock);
}

void MTLCommandBuffer_encodeWaitForEvent(obj_handle_t cmdbuf, obj_handle_t event, uint64_t value)
{
	CALLED();
	(void)cmdbuf;
	if (MTLSharedEvent_signaledValue(event) < value)
		pvg_log("a command buffer waits for an event value not reached (%llu); work is done in "
			"commit order", value);
}

// RUXN_KEEP_SUBMIT=N: the Nth submit is kept on disk (wm_refused.c).
static unsigned keep_submit_number(void)
{
	char value[16];
	DWORD n = GetEnvironmentVariableA("RUXN_KEEP_SUBMIT", value, sizeof(value));

	return n && n < sizeof(value) ? (unsigned)strtoul(value, NULL, 10) : 0;
}

// A line before each command buffer goes out, for a process whose log goes
// to the kernel driver (wddmtool klogfollow puts it on the disk at once):
// if the GPU faults on it, the guest may be ended before its refusal comes
// back, and this line still names it (run rn76). Its number, its resources
// (the first eight), its segments' sizes and an FNV-1a of their bytes.
static void say_before_sending(const struct command_buffer *cb)
{
	static unsigned sending;
	char ids[96];
	uint32_t hash = 2166136261u;
	unsigned i, n = 0;
	size_t j;

	for (i = 0; i < cb->resources.count && i < 8 && n < sizeof(ids) - 12; i++)
		n += (unsigned)snprintf(ids + n, sizeof(ids) - n, "%u%s ", cb->resources.ids[i],
					cb->resources.flags[i] ? "w" : "");
	ids[n] = 0;
	for (i = 0; i < cb->segments; i++) {
		const UCHAR *at = cb->memory.chunk->user + cb->segment_offset[i];

		for (j = 0; j < cb->segment_size[i]; j++)
			hash = (hash ^ at[j]) * 16777619u;
	}
	pvg_log("sending %u: %u resources (%s), %u segments, first %llu bytes, fnv %08x", ++sending,
		cb->resources.count, ids, cb->segments, (unsigned long long)cb->segment_size[0], hash);
}

bool submit(struct command_buffer *cb)
{
	static unsigned keep_submit;
	static bool keep_read;
	static UCHAR in[sizeof(struct ruxnpvg_submit) + PVGX_MOST_RESOURCES * sizeof(struct ruxnpvg_resource) +
			MOST_SEGMENTS * sizeof(struct ruxnpvg_command_buffer)];
	struct ruxnpvg_submit *s = (void *)in;
	struct ruxnpvg_resource *r = (void *)(s + 1);
	struct ruxnpvg_command_buffer *b;
	unsigned i;

	if (cb->resources.overflow || (max_resources && cb->resources.count > max_resources) ||
	    (max_buffers && cb->segments > max_buffers)) {
		pvg_log("FAILED: a submit of %u resources and %u command buffers is too big", cb->resources.count,
			cb->segments);
		return false;
	}
	// A guard of our own: a command buffer naming an object the device
	// refused to make is refused by it in turn, so it is not sent.
	{
		uint32_t bad = wm_broken_named(cb->resources.ids, cb->resources.count);

		static unsigned held_back;

		if (bad) {
			// The first sixteen, then every 256th, so a log stays readable.
			if (++held_back <= 16 || !(held_back % 256))
				pvg_log("FAILED: a command buffer names object %u, which the device never made: "
					"not sent (%u so far)", bad, held_back);
			return false;
		}
	}
	if (!keep_read) {
		keep_submit = keep_submit_number();
		keep_read = true;
	}
	b = (void *)(r + cb->resources.count);
	for (i = 0; i < cb->resources.count; i++) {
		r[i].id = cb->resources.ids[i];
		r[i].flags = cb->resources.flags[i];
	}
	for (i = 0; i < cb->segments; i++) {
		b[i].handle = cb->memory.chunk->handle;
		b[i].pad = 0;
		b[i].offset = cb->segment_offset[i];
		b[i].size = cb->segment_size[i];
	}
	s->resource_count = cb->resources.count;
	s->buffer_count = cb->segments;
	s->flags = 0;
	s->pad = 0;
	if (wm_log_in_kernel())
		say_before_sending(cb);
	wm_say_live_now_and_then();
	{
		bool sent = wm_control("submit", RUXNPVG_IOCTL_SUBMIT, in, (DWORD)((UCHAR *)(b + cb->segments) - in),
				       NULL, 0);
		static unsigned submits;
		const UCHAR *segments[MOST_SEGMENTS];
		uint32_t sizes[MOST_SEGMENTS];

		submits++;
		for (i = 0; i < cb->segments; i++) {
			segments[i] = cb->memory.chunk->user + cb->segment_offset[i];
			sizes[i] = (uint32_t)cb->segment_size[i];
		}
		if (sent)
			wm_recent_keep(cb->resources.ids, cb->resources.flags, cb->resources.count, segments, sizes,
				       cb->segments);
		if (sent && submits != keep_submit)
			return true;
		wm_refused_dump(sent ? "submit" : "refused", cb->resources.ids, cb->resources.flags,
				cb->resources.count, segments, sizes, cb->segments);
		return sent;
	}
}

ULONG64 committed;
static bool quiet_submits;

void MTLCommandBuffer_commit(obj_handle_t cmdbuf)
{
	struct object *c;
	struct command_buffer *cb;
	unsigned i;

	CALLED();
	EnterCriticalSection(&lock);
	c = get(cmdbuf, K_COMMAND_BUFFER);
	if (!c)
		goto out;
	cb = c->u.command_buffer;
	cb->committed = true;
	if (cb->open_encoder) {
		pvg_log("FAILED: commit with an encoder open");
		cb->failed = true;
	}
	if (cb->failed) {
		pvg_log("NOT SUBMITTED: a command buffer of %u segments (see the failure above)", cb->segments);
		cb->status = WMTCommandBufferStatusError;
	} else if (!cb->segments) {
		cb->status = WMTCommandBufferStatusCompleted;
	} else if (submit(cb)) {
		if (!quiet_submits)
			pvg_log("submitted: %u segments, %u resources", cb->segments, cb->resources.count);
		cb->status = WMTCommandBufferStatusCompleted;
	} else {
		cb->status = WMTCommandBufferStatusError;
	}
	// Buffers the command buffer reached are no longer fresh.
	for (i = 1; i < object_count; i++) {
		unsigned j;

		if (objects[i].kind != K_BUFFER || !objects[i].u.buffer.fresh)
			continue;
		for (j = 0; j < cb->resources.count; j++)
			if (cb->resources.ids[j] == objects[i].u.buffer.number)
				objects[i].u.buffer.fresh = false;
	}
	// The work is done: the memory serves the next command buffer.
	give_back_command_memory(cb);
	if (++committed == 64 && !quiet_submits) {
		pvg_log("64 command buffers: submits are not logged one by one from here on");
		quiet_submits = true;
	}
	// Events are set even after a failure, so that the program does not
	// wait for ever; the failure is in the log and the picture.
	for (i = 0; i < cb->signals; i++)
		set_event_value(cb->signal_event[i], cb->signal_value[i]);
	// Waited for at commit: what it named may go now.
	wm_let_go(cb);
out:
	LeaveCriticalSection(&lock);
}

void MTLCommandBuffer_waitUntilCompleted(obj_handle_t cmdbuf)
{
	CALLED();
	(void)cmdbuf;
}

enum WMTCommandBufferStatus MTLCommandBuffer_status(obj_handle_t cmdbuf)
{
	struct object *c;
	enum WMTCommandBufferStatus status = WMTCommandBufferStatusError;

	CALLED();
	EnterCriticalSection(&lock);
	c = get(cmdbuf, K_COMMAND_BUFFER);
	if (c)
		status = c->u.command_buffer->status;
	LeaveCriticalSection(&lock);
	return status;
}

obj_handle_t MTLCommandBuffer_error(obj_handle_t cmdbuf)
{
	obj_handle_t h = 0;

	CALLED();
	if (MTLCommandBuffer_status(cmdbuf) == WMTCommandBufferStatusError) {
		EnterCriticalSection(&lock);
		h = new_string("the command buffer was not submitted, or the device refused it");
		if (h)
			objects[h].kind = K_ERROR;
		LeaveCriticalSection(&lock);
	}
	return h;
}

obj_handle_t MTLCommandBuffer_logs(obj_handle_t cmdbuf)
{
	CALLED();
	(void)cmdbuf;
	return 0;
}
