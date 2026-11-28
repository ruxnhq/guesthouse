// SPDX-License-Identifier: MIT
//
// winemetal over ruxnpvg: the handle table and Foundation's part (retain,
// release, autorelease pools, arrays, strings).

#include "wm_internal.h"

struct object objects[MOST_OBJECTS];
unsigned object_count = 1;   // handle 0 is no object

// Handles given back by objects whose last reference went (release_object),
// used again before new ones.
static obj_handle_t free_handles[MOST_OBJECTS];
static unsigned free_handle_count;
static ULONG64 objects_left_on_task;	// freed handles whose task object stays
ULONG64 objects_destroyed;		// handles whose last reference went
// The innermost autorelease pool of this thread, as Foundation keeps them.
static _Thread_local obj_handle_t current_pool;

unsigned wm_free_handle_count(void)
{
	return free_handle_count;
}

// What a program keeps: made by a create it can be told has failed.
static bool kept_kind(enum kind kind)
{
	switch (kind) {
	case K_BUFFER: case K_TEXTURE: case K_LIBRARY: case K_FUNCTION:
	case K_RENDER_PIPELINE: case K_COMPUTE_PIPELINE: case K_DEPTH_STENCIL:
	case K_SAMPLER: case K_SHADER:
		return true;
	default:
		return false;
	}
}

obj_handle_t new_object(enum kind kind)
{
	obj_handle_t h;

	if (kept_kind(kind) && object_count - free_handle_count >= MOST_OBJECTS - HANDLES_KEPT_FOR_WORK) {
		pvg_log("FAILED: out of object handles for kept objects (%u in use)",
			object_count - free_handle_count);
		return 0;
	}
	if (free_handle_count) {
		h = free_handles[--free_handle_count];
	} else if (object_count == MOST_OBJECTS) {
		pvg_log("FAILED: out of object handles");
		return 0;
	} else {
		h = object_count++;
	}
	memset(&objects[h], 0, sizeof(objects[h]));
	objects[h].kind = kind;
	objects[h].references = 1;
	return h;
}

struct object *get(obj_handle_t handle, enum kind kind)
{
	if (!handle || handle >= object_count || objects[handle].kind != kind)
		return NULL;
	return &objects[handle];
}


obj_handle_t new_string(const char *text)
{
	obj_handle_t h = new_object(K_STRING);
	size_t n = strlen(text);

	if (!h)
		return 0;
	autorelease(h);
	objects[h].u.string.text = malloc(n + 1);
	if (objects[h].u.string.text)
		memcpy(objects[h].u.string.text, text, n + 1);
	return h;
}

static obj_handle_t new_error(const char *text)
{
	obj_handle_t h = new_string(text);

	if (h)
		objects[h].kind = K_ERROR;
	pvg_log("FAILED: %s", text);
	return h;
}

void set_error(obj_handle_t *out, const char *text)
{
	obj_handle_t h = new_error(text);

	if (out)
		*out = h;
}

// ---------------------------------------------------------------------
// Foundation's part.

void NSObject_retain(obj_handle_t obj)
{
	CALLED();
	if (obj && obj < object_count)
		InterlockedIncrement(&objects[obj].references);
}

// The command buffers' memory, kept for the next ones: a command buffer is
// waited for at commit, so its memory is free again right after.
struct memory command_memory[MOST_POOLED_COMMAND_MEMORY];
unsigned command_memory_count;

void give_back_command_memory(struct command_buffer *cb)
{
	if (!cb->memory.chunk)
		return;
	if (command_memory_count < MOST_POOLED_COMMAND_MEMORY)
		command_memory[command_memory_count++] = cb->memory;
	cb->memory.chunk = NULL;
}

static void release_locked(obj_handle_t h);

// Takes an object out of the task (OBJECT_DESTROY: the device is told) and
// keeps its number for the next object.
bool wm_destroy_task_object(uint32_t number)
{
	struct ruxnpvg_object_destroy d = { number, 0 };

	// The process is ending: its task goes with it, and the graphics
	// kernel no longer takes escapes from it (runs rn53, rn55: hundreds of
	// destroys failed at exit).
	if (wm_process_ending())
		return false;
	if (!number || !wm_control("object_destroy", RUXNPVG_IOCTL_OBJECT_DESTROY, &d, sizeof(d), NULL, 0))
		return false;
	if (free_number_count < TASK_NUMBERS_KEPT)
		free_numbers[free_number_count++] = number;
	return true;
}

// An object's last reference has gone: what it holds goes, and its handle
// is used again. Textures, views, states and pipelines leave the task;
// buffers give their memory back; functions stay on the device, counted.
static void destroy_object(obj_handle_t h)
{
	struct object *o = &objects[h];
	unsigned i;

	switch (o->kind) {
	case K_COMMAND_BUFFER:
		wm_let_go(o->u.command_buffer);
		give_back_command_memory(o->u.command_buffer);
		free(o->u.command_buffer);
		break;
	case K_ENCODER:
		free(o->u.encoder);
		break;
	case K_STRING:
	case K_ERROR:
		free(o->u.string.text);
		break;
	case K_POOL:
		if (current_pool == h)
			current_pool = o->u.pool.outer;
		for (i = 0; i < o->u.pool.count; i++)
			release_locked(o->u.pool.held[i]);
		free(o->u.pool.held);
		break;
	case K_DEVICE:
	case K_ARRAY:
	case K_QUEUE:
		return;		// the one device and its array stay
	case K_TEXTURE:
		// A view of its own (a texture over a buffer, some levels of a
		// texture: kind 8) is deleted (operation 1001, scene
		// view-delete). A texture of its own (not over a surface, which
		// goes only with the task) leaves the task and its memory is
		// given back. Either way its number is used again.
		if (o->u.texture.own_view) {
			obj_handle_t over = o->u.texture.over;
			bool deleted = wm_destroy_task_object(o->u.texture.number);

			o->u.texture.over = 0;
			if (over)
				release_locked(over);
			if (deleted)
				break;
			goto left;
		}
		// A texture over a surface: its view (command 40) and itself
		// (command 37, scene surface-delete) leave the task, then its
		// surface handle is given back once no texture lies over it.
		// An alias (the whole texture again) lets go of its texture.
		if (o->u.texture.view && !o->u.texture.own_view) {
			obj_handle_t over = o->u.texture.over;

			o->u.texture.over = 0;
			if (over)
				release_locked(over);
			break;
		}
		if (o->u.texture.surface && !o->u.texture.view) {
			if (o->u.texture.view_number)
				wm_destroy_task_object(o->u.texture.view_number);
			if (wm_destroy_task_object(o->u.texture.number))
				wm_surface_release(o->u.texture.surface_handle);
			break;
		}
		if (!o->u.texture.view && !o->u.texture.surface && wm_destroy_task_object(o->u.texture.number)) {
			wm_give_back(&o->u.texture.memory);
			// A depth-and-stencil texture's stencil half goes with it.
			if (o->u.texture.stencil_number && wm_destroy_task_object(o->u.texture.stencil_number))
				wm_give_back(&o->u.texture.stencil_memory);
			break;
		}
		goto left;
	case K_SAMPLER:
	case K_DEPTH_STENCIL:
		if (wm_destroy_task_object(o->u.state.number))
			break;
		goto left;
	case K_RENDER_PIPELINE:
	case K_COMPUTE_PIPELINE:
		if (o->u.pipeline.number && wm_destroy_task_object(o->u.pipeline.number))
			break;
		goto left;
	case K_BUFFER:
		// A range of a chunk, whose object stays: the range is given
		// back for the next buffer or texture.
		wm_give_back(&o->u.buffer.memory);
		break;
	case K_FUNCTION:
	left:
		if (++objects_left_on_task % 256 == 0)
			pvg_log("%llu objects given up whose task objects stay on the device",
				objects_left_on_task);
		break;
	default:
		break;
	}
	o->kind = K_FREE;
	free_handles[free_handle_count++] = h;
	objects_destroyed++;
}

static void release_locked(obj_handle_t h)
{
	if (!h || h >= object_count || objects[h].kind == K_FREE)
		return;
	if (--objects[h].references == 0)
		destroy_object(h);
}

// A command buffer holds what its commands name (wm_internal.h). Under
// winemetal's lock.
void wm_hold(struct command_buffer *cb, obj_handle_t h)
{
	unsigned i;

	for (i = 0; i < cb->held_count; i++)
		if (cb->held[i] == h)
			return;
	if (cb->held_count == cb->held_room) {
		unsigned room = cb->held_room ? cb->held_room * 2 : 32;
		obj_handle_t *more = realloc(cb->held, room * sizeof(*more));

		if (!more)
			return;
		cb->held = more;
		cb->held_room = room;
	}
	objects[h].references++;
	cb->held[cb->held_count++] = h;
}

// Its work done (or never to be), what it held goes. Under the lock.
void wm_let_go(struct command_buffer *cb)
{
	unsigned i, count = cb->held_count;
	obj_handle_t *held = cb->held;

	cb->held = NULL;
	cb->held_count = cb->held_room = 0;
	for (i = 0; i < count; i++)
		release_locked(held[i]);
	free(held);
}

void NSObject_release(obj_handle_t obj)
{
	CALLED();
	EnterCriticalSection(&lock);
	release_locked(obj);
	LeaveCriticalSection(&lock);
}

// What Metal hands back autoreleased (command buffers, encoders, strings)
// belongs to the thread's innermost pool, and goes when the pool does.
// With no pool it is kept for good, as Foundation would leak it.
void autorelease(obj_handle_t h)
{
	struct object *p;
	obj_handle_t *held;

	if (!h || !current_pool)
		return;
	p = &objects[current_pool];
	if (p->kind != K_POOL)
		return;
	if (p->u.pool.count == p->u.pool.room) {
		unsigned room = p->u.pool.room ? 2 * p->u.pool.room : 64;

		held = realloc(p->u.pool.held, room * sizeof(*held));
		if (!held)
			return;
		p->u.pool.held = held;
		p->u.pool.room = room;
	}
	p->u.pool.held[p->u.pool.count++] = h;
}

obj_handle_t NSArray_object(obj_handle_t array, uint64_t index)
{
	struct object *a;

	CALLED();
	a = get(array, K_ARRAY);
	return a && index < a->u.array.count ? a->u.array.first + index : 0;
}

uint64_t NSArray_count(obj_handle_t array)
{
	struct object *a;

	CALLED();
	a = get(array, K_ARRAY);
	return a ? a->u.array.count : 0;
}

obj_handle_t NSAutoreleasePool_alloc_init(void)
{
	obj_handle_t h;

	CALLED();
	EnterCriticalSection(&lock);
	h = new_object(K_POOL);
	if (h) {
		objects[h].u.pool.outer = current_pool;
		current_pool = h;
	}
	LeaveCriticalSection(&lock);
	return h;
}

static const char *text_of(obj_handle_t str)
{
	if (!str || str >= object_count)
		return NULL;
	if (objects[str].kind != K_STRING && objects[str].kind != K_ERROR)
		return NULL;
	return objects[str].u.string.text;
}

uint32_t NSString_getCString(obj_handle_t str, char *buffer, uint64_t maxLength, enum WMTStringEncoding encoding)
{
	const char *text = text_of(str);
	size_t n;

	CALLED();
	(void)encoding;
	if (!text || !maxLength)
		return 0;
	n = strlen(text);
	if (n + 1 > maxLength)
		return 0;
	memcpy(buffer, text, n + 1);
	return 1;
}

uint64_t NSString_lengthOfBytesUsingEncoding(obj_handle_t str, enum WMTStringEncoding encoding)
{
	const char *text = text_of(str);

	CALLED();
	(void)encoding;
	return text ? strlen(text) : 0;
}

obj_handle_t NSString_string(const char *data, enum WMTStringEncoding encoding)
{
	obj_handle_t h;

	CALLED();
	(void)encoding;
	EnterCriticalSection(&lock);
	h = new_string(data ? data : "");
	LeaveCriticalSection(&lock);
	return h;
}

obj_handle_t NSString_alloc_init(const char *data, enum WMTStringEncoding encoding)
{
	CALLED();
	return NSString_string(data, encoding);
}

obj_handle_t NSObject_description(obj_handle_t obj)
{
	obj_handle_t h;
	const char *text = text_of(obj);

	CALLED();
	EnterCriticalSection(&lock);
	h = new_string(text ? text : "(an object of winemetal over ruxnpvg)");
	LeaveCriticalSection(&lock);
	return h;
}
