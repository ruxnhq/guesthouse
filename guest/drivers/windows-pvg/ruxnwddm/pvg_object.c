// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// What a program does with its task, through the task escapes: parameters,
// buffer objects and surfaces, objects made and destroyed, placements,
// submits and syncs, and the surface sharing controls. Ported from
// ruxnpvg's task.c (its controls and their dispatch), which was a port of
// pvg_task.c of the Linux driver at 377700a. The display controls are not
// offered to programs here: the graphics kernel's flips drive the display
// (flip.c).

#include "pvg_task.h"

static NTSTATUS get_param(struct ruxnpvg_get_param *args)
{
	if (args->pad)
		return E_INVAL;
	switch (args->param) {
	case RUXNPVG_PARAM_UAPI_VERSION:
		args->value = RUXNPVG_UAPI_VERSION;
		break;
	case RUXNPVG_PARAM_PROTOCOL_VERSION:
		args->value = RUXNPVG_VERSION;
		break;
	case RUXNPVG_PARAM_MAX_OBJECTS:
		args->value = TASK_MAX_OBJECTS;
		break;
	case RUXNPVG_PARAM_MAX_RESOURCES:
		args->value = MAX_RESOURCES;
		break;
	case RUXNPVG_PARAM_MAX_BUFFERS:
		args->value = MAX_BUFFERS;
		break;
	case RUXNPVG_PARAM_TASK_BYTES:
		args->value = pvg_task_bytes() - ((ULONG64)TASK_FIRST_BO_PAGE << RUXNWDDM_PAGE_SHIFT);
		break;
	case RUXNPVG_PARAM_MAX_DESCRIPTION:
		args->value = MAX_DESCRIPTION;
		break;
	default:
		return E_INVAL;
	}
	return STATUS_SUCCESS;
}

// The first free surface handle of the client: a hole a destroyed surface
// left, or the next one at the end; 0 if all are taken. A program that
// makes and drops surfaces all day, as the compositor does, would
// otherwise run out (runs rn53, rn55: dwm's imports failed with
// STATUS_INSUFFICIENT_RESOURCES and windows lost their pictures).
static ULONG free_surface_handle(RUXNWDDM_CLIENT *client)
{
	ULONG i;

	for (i = 0; i < client->surface_count; i++)
		if (!client->surfaces[i])
			return i + 1;
	return client->surface_count < RUXNPVG_MAX_SURFACES ? client->surface_count + 1 : 0;
}

static void put_surface(RUXNWDDM_CLIENT *client, ULONG handle, struct pvg_surface *surface)
{
	client->surfaces[handle - 1] = surface;
	if (handle > client->surface_count)
		client->surface_count = handle;
}

// Makes a surface and maps it into the caller, as Linux's surface buffer,
// unless the caller asks that only the GPU reach it. It is not put into the
// task. In the caller's context.
static NTSTATUS surface_create(RUXNWDDM_CLIENT *client, struct ruxnpvg_bo_create *args)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_surface *surface;
	NTSTATUS status;

	ULONG handle;

	if (client->dead || !pvg->mapper_regs)
		return E_NODEV;
	handle = free_surface_handle(client);
	if (!handle)
		return E_NOSPC;
	status = pvg_surface_create(client, args->width, args->height, &surface);
	if (!NT_SUCCESS(status))
		return status;
	if (!(args->flags & RUXNPVG_BO_NO_CPU)) {
		status = pvg_surface_map_user(surface);
		if (!NT_SUCCESS(status)) {
			pvg_surface_release(client, surface);
			return status;
		}
	}
	put_surface(client, handle, surface);

	args->handle = handle;
	args->size = surface->bytes;
	args->pitch = surface->pitch;
	args->address = 0;
	args->user_address = (ULONG64)(ULONG_PTR)surface->user;
	return STATUS_SUCCESS;
}

// Makes a buffer object, puts it into the task and maps it into the
// caller. In the caller's context.
static NTSTATUS bo_create(RUXNWDDM_CLIENT *client, struct ruxnpvg_bo_create *args)
{
	struct pvg_task *task;
	struct pvg_bo *bo;
	ULONG64 bytes;
	ULONG slot;
	NTSTATUS status;

	if (args->pad || (args->flags & ~(RUXNPVG_BO_SURFACE | RUXNPVG_BO_NO_CPU)))
		return E_INVAL;
	if (args->flags & RUXNPVG_BO_SURFACE)
		return surface_create(client, args);
	if (args->flags & RUXNPVG_BO_NO_CPU)
		return E_INVAL;
	if (args->width || args->height || !args->size || args->size > RUXNPVG_BO_MAX_BYTES)
		return E_INVAL;
	bytes = (args->size + RUXNWDDM_PAGE_SIZE - 1) & ~((ULONG64)RUXNWDDM_PAGE_SIZE - 1);

	// The memory goes into the task, which is made now if need be.
	status = client_task(client, &task);
	if (!NT_SUCCESS(status))
		return status;
	// A handle a destroyed buffer object left, else a new one.
	for (slot = 0; slot < client->bo_count && client->bos[slot]; slot++)
		;
	if (slot == RUXNPVG_MAX_BOS)
		return E_NOSPC;

	status = pvg_bo_alloc(client->pvg, bytes, &bo);
	if (!NT_SUCCESS(status))
		return status;
	// Mapped first, so that a failure leaves no entry in the task; a
	// failed place clears what it entered.
	status = pvg_bo_map(bo);
	if (NT_SUCCESS(status))
		status = task_place(task, bo);
	if (!NT_SUCCESS(status)) {
		pvg_bo_release(client, bo, FALSE);
		return status;
	}
	client->bos[slot] = bo;
	if (slot == client->bo_count)
		client->bo_count++;

	args->handle = slot + 1;
	args->size = bytes;
	args->pitch = 0;
	args->address = (ULONG64)bo->first_page << RUXNWDDM_PAGE_SHIFT;
	args->user_address = (ULONG64)(ULONG_PTR)bo->user;
	return STATUS_SUCCESS;
}

static NTSTATUS object_create(RUXNWDDM_CLIENT *client, const struct ruxnpvg_object_create *args,
			      const UCHAR *tail)
{
	struct pvg_object *object;
	struct pvg_task *task;
	struct pvg_bo *bo = NULL;
	struct pvg_surface *surface = NULL;
	UCHAR head[MAX_HEAD] = { 0 };
	ULONG head_bytes = 0, bytes, slot, slots, page;
	USHORT delete_op = 0;
	UCHAR *description;
	ULONG64 address;
	NTSTATUS status;

	if (args->pad || !args->id || args->id >= TASK_MAX_OBJECTS ||
	    args->data_size > MAX_DESCRIPTION - MAX_HEAD)
		return E_INVAL;

	status = client_task(client, &task);
	if (!NT_SUCCESS(status))
		return status;

	object = &task->objects[args->id];
	if (object->kind)
		return E_EXIST;

	switch (args->kind) {
	case RUXNPVG_OBJECT_BUFFER:
		// The length, the task's page the memory starts at, a byte of flags.
		if (args->data_size || (args->flags & ~RUXNPVG_BUFFER_PRIVATE) ||
		    (args->offset & (RUXNWDDM_PAGE_SIZE - 1)))
			return E_INVAL;
		status = task_memory(client, args->handle, args->offset, args->size, &bo);
		if (!NT_SUCCESS(status))
			return status;
		page = bo->first_page + (ULONG)(args->offset >> RUXNWDDM_PAGE_SHIFT);
		put_le64(head, args->size);
		put_le32(head + 8, page);
		head[12] = (UCHAR)args->flags;
		head_bytes = 16;
		break;
	case RUXNPVG_OBJECT_TEXTURE:
		// The length and the task's page of the memory it is loaded from.
		if (!args->data_size || args->flags ||
		    (args->offset & (RUXNWDDM_PAGE_SIZE - 1)) ||
		    (args->size & (RUXNWDDM_PAGE_SIZE - 1)))
			return E_INVAL;
		status = task_memory(client, args->handle, args->offset, args->size, &bo);
		if (!NT_SUCCESS(status))
			return status;
		page = bo->first_page + (ULONG)(args->offset >> RUXNWDDM_PAGE_SHIFT);
		put_le64(head, args->size);
		put_le32(head + 8, page);
		head_bytes = 12;
		break;
	case RUXNPVG_OBJECT_FUNCTION:
		// Where its library lies in the task, and the library's length.
		if (args->flags || args->size > MAXULONG ||
		    !tail_is_operation(tail, args->data_size, args->id) ||
		    get_le32(tail) != OP_FUNCTION)
			return E_INVAL;
		delete_op = OP_DELETE_FUNCTION;
		status = task_memory(client, args->handle, args->offset, args->size, &bo);
		if (!NT_SUCCESS(status))
			return status;
		address = ((ULONG64)bo->first_page << RUXNWDDM_PAGE_SHIFT) + args->offset;
		put_le64(head, address);
		put_le32(head + 8, (ULONG)args->size);
		head_bytes = 12;
		break;
	case RUXNPVG_OBJECT_STATE:
		if (args->handle || args->flags || args->offset || args->size ||
		    !tail_is_operation(tail, args->data_size, args->id))
			return E_INVAL;
		// What it is decides what deletes it.
		switch (get_le32(tail)) {
		case OP_SAMPLER:
			delete_op = OP_DELETE_SAMPLER;
			break;
		case OP_DEPTH_STENCIL:
			delete_op = OP_DELETE_DEPTH_STENCIL;
			break;
		case OP_COMPUTE_PIPELINE:
			delete_op = OP_DELETE_COMPUTE_PIPELINE;
			break;
		case OP_RENDER_PIPELINE:
			delete_op = OP_DELETE_RENDER_PIPELINE;
			break;
		default:
			return E_INVAL;
		}
		break;
	case RUXNPVG_OBJECT_SURFACE_TEXTURE:
		// The mapping's id, which the surface gets here if it has none.
		if (args->flags || args->offset || args->size ||
		    args->data_size != OP_SURFACE_TEXTURE_BYTES ||
		    !tail_is_operation(tail, args->data_size, args->id) ||
		    get_le32(tail) != OP_SURFACE_TEXTURE)
			return E_INVAL;
		surface = client_surface(client, args->handle);
		if (!surface)
			return E_NOENT;
		// Its rows are the surface's: as many bytes, over all of them.
		if (!pvg_surface_texture_fits(tail, surface->width) ||
		    get_le32(tail + OP_SURFACE_HEIGHT) != surface->height)
			return E_INVAL;
		status = pvg_surface_map(client->pvg, surface);
		if (!NT_SUCCESS(status))
			return status;
		put_le64(head, surface->id);
		head_bytes = 8;
		break;
	case RUXNPVG_OBJECT_TEXTURE_VIEW:
		// No head: the whole operation is checked (pvg_view.c).
		if (args->handle || args->flags || args->offset || args->size)
			return E_INVAL;
		status = pvg_view_check(task, object, args->id, tail, args->data_size);
		if (!NT_SUCCESS(status))
			return status;
		delete_op = OP_DELETE_TEXTURE;
		break;
	default:
		return E_INVAL;
	}

	bytes = head_bytes + args->data_size;
	description = ExAllocatePool2(POOL_FLAG_NON_PAGED, bytes ? bytes : 1, POOL_TAG_TEMP);
	if (!description)
		return E_NOMEM;
	RtlCopyMemory(description, head, head_bytes);
	if (args->data_size)
		RtlCopyMemory(description + head_bytes, tail, args->data_size);

	status = description_alloc(task, bytes, &slot, &slots);
	if (!NT_SUCCESS(status)) {
		ExFreePoolWithTag(description, POOL_TAG_TEMP);
		return status;
	}
	description_write(task, slot, description, bytes);
	ExFreePoolWithTag(description, POOL_TAG_TEMP);

	object->kind = (UCHAR)args->kind;
	object->delete_op = delete_op;
	object->bytes = args->kind == RUXNPVG_OBJECT_BUFFER ? args->size : 0;
	pvg_view_note_texture(object, tail, args->data_size);
	object->slot = slot;
	object->slots = (USHORT)slots;
	if (bo) {
		bo->objects++;
		object->bo = bo;
	}
	if (surface) {
		InterlockedIncrement(&surface->objects);
		object->surface = surface;
	}

	// The kind is written last: an entry without one is no object.
	address = ((ULONG64)TASK_DESCRIPTIONS_PAGE << RUXNWDDM_PAGE_SHIFT) +
		  (ULONG64)slot * TASK_DESCRIPTION_SLOT;
	*object_word(task, args->id, 1) = lower_32(address);
	*object_word(task, args->id, 2) = upper_32(address);
	KeMemoryBarrier();
	*object_word(task, args->id, 0) = args->kind | bytes << 8;
	return STATUS_SUCCESS;
}

// Takes an object out of the task. The host keeps what it made for a number
// and does not look at the list again, so the entry is cleared and the host
// is told: a buffer or a texture by the command that deletes a resource, a
// function or a state by the command that carries one of the serializer's
// delete operations. Only then is the number free.
// A buffer object given back by its maker (winemetal's empty chunk): out of
// the task (task_unplace), then its memory freed, in this process, which
// undoes the mapping. Refused while an object of the task names it. If the
// device did not say it dropped the place, the buffer object stays in its
// handle, untouched, until the client's end.
static NTSTATUS bo_destroy(RUXNWDDM_CLIENT *client, const struct ruxnpvg_bo_destroy *args)
{
	struct pvg_task *task = client->task;
	struct pvg_bo *bo;
	BOOLEAN unplaced;

	if (args->pad)
		return E_INVAL;
	if (!task || !args->handle || args->handle > client->bo_count || !(bo = client->bos[args->handle - 1]))
		return E_NOENT;
	if (bo->objects)
		return E_BUSY;
	unplaced = task_unplace(client->pvg, task, bo);
	if (!unplaced) {
		InterlockedIncrement64(&client->pvg->tasks.bos_not_unplaced);
		return E_NODEV;
	}
	client->bos[args->handle - 1] = NULL;
	pvg_bo_release(client, bo, FALSE);
	InterlockedIncrement64(&client->pvg->tasks.bos_unplaced);
	return STATUS_SUCCESS;
}

static NTSTATUS object_destroy(RUXNWDDM_CLIENT *client, const struct ruxnpvg_object_destroy *args)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_task *task = client->task;
	struct pvg_object *object;
	ULONG payload[4];
	NTSTATUS status;

	if (args->pad)
		return E_INVAL;
	if (!task)
		return E_NOENT;

	object = task_object(task, args->id);
	if (!object)
		return E_NOENT;
	// A texture over a surface goes by command 37 as any texture (scene
	// surface-delete); its surface is let go of by object_release. A view
	// goes by command 40 (delete_op).
	*object_word(task, args->id, 0) = 0;
	*object_word(task, args->id, 1) = 0;
	*object_word(task, args->id, 2) = 0;

	if (!task->defined) {
		status = E_NODEV;
	} else {
		payload[0] = task->id;
		if (kind_is_resource(object->kind)) {
			payload[1] = args->id;
			status = ruxnwddm_ring_send(pvg->adapter, RUXNPVG_CMD_TASK_DELETE_RESOURCE, payload,
						   2, TASK_TIMEOUT_MS);
		} else {
			payload[1] = object->delete_op;
			payload[2] = OP_DELETE_BYTES;
			payload[3] = args->id;
			status = ruxnwddm_ring_send(pvg->adapter, RUXNPVG_CMD_TASK_DELETE_OBJECT, payload,
						   4, TASK_TIMEOUT_MS);
		}
	}
	if (!NT_SUCCESS(status)) {
		// The host may still have it: what it names stays until the
		// task ends.
		object->retired = TRUE;
	} else {
		object_release(task, object);
	}
	return status;
}

static NTSTATUS object_placement(RUXNWDDM_CLIENT *client, struct ruxnpvg_object_placement *args)
{
	struct pvg_task *task = client->task;
	volatile ULONG64 *words;

	if (args->pad)
		return E_INVAL;
	if (!task)
		return E_NOENT;
	if (!task_object(task, args->id))
		return E_NOENT;
	words = placement_words(task, args->id);
	args->words[0] = words[0];
	args->words[1] = words[1];
	return STATUS_SUCCESS;
}

// The fixed part and the arrays behind it, each length checked before it is
// used, and the input exactly as long as they are together.
NTSTATUS pvg_task_dispatch(RUXNWDDM_CLIENT *client, ULONG code, UCHAR *in, SIZE_T in_length,
			   PVOID out, SIZE_T out_length, ULONG_PTR *returned)
{
	switch (code) {
	case RUXNPVG_IOCTL_GET_PARAM: {
		struct ruxnpvg_get_param *args = (PVOID)in;
		NTSTATUS status;

		if (in_length != sizeof(*args) || out_length < sizeof(*args))
			return E_INVAL;
		status = get_param(args);
		if (NT_SUCCESS(status)) {
			RtlCopyMemory(out, args, sizeof(*args));
			*returned = sizeof(*args);
		}
		return status;
	}
	case RUXNPVG_IOCTL_BO_CREATE: {
		struct ruxnpvg_bo_create *args = (PVOID)in;
		NTSTATUS status;

		if (in_length != sizeof(*args) || out_length < sizeof(*args))
			return E_INVAL;
		status = bo_create(client, args);
		if (NT_SUCCESS(status)) {
			RtlCopyMemory(out, args, sizeof(*args));
			*returned = sizeof(*args);
		}
		return status;
	}
	case RUXNPVG_IOCTL_OBJECT_CREATE: {
		struct ruxnpvg_object_create *args = (PVOID)in;

		if (in_length < sizeof(*args) ||
		    args->data_size > MAX_DESCRIPTION - MAX_HEAD ||
		    in_length != sizeof(*args) + args->data_size)
			return E_INVAL;
		return object_create(client, args, in + sizeof(*args));
	}
	case RUXNPVG_IOCTL_BO_DESTROY: {
		struct ruxnpvg_bo_destroy *args = (PVOID)in;

		if (in_length != sizeof(*args))
			return E_INVAL;
		return bo_destroy(client, args);
	}
	case RUXNPVG_IOCTL_OBJECT_DESTROY: {
		struct ruxnpvg_object_destroy *args = (PVOID)in;

		if (in_length != sizeof(*args))
			return E_INVAL;
		return object_destroy(client, args);
	}
	case RUXNPVG_IOCTL_OBJECT_PLACEMENT: {
		struct ruxnpvg_object_placement *args = (PVOID)in;
		NTSTATUS status;

		if (in_length != sizeof(*args) || out_length < sizeof(*args))
			return E_INVAL;
		status = object_placement(client, args);
		if (NT_SUCCESS(status)) {
			RtlCopyMemory(out, args, sizeof(*args));
			*returned = sizeof(*args);
		}
		return status;
	}
	case RUXNPVG_IOCTL_SUBMIT: {
		struct ruxnpvg_submit *args = (PVOID)in;
		size_t resources_at = sizeof(*args), buffers_at;

		if (in_length < sizeof(*args))
			return E_INVAL;
		if (args->flags || args->pad || !args->buffer_count ||
		    args->buffer_count > MAX_BUFFERS || args->resource_count > MAX_RESOURCES)
			return E_INVAL;
		buffers_at = resources_at + args->resource_count * sizeof(struct ruxnpvg_resource);
		if (in_length != buffers_at +
				 args->buffer_count * sizeof(struct ruxnpvg_command_buffer))
			return E_INVAL;
		return pvg_task_submit(client, args, (PVOID)(in + resources_at), (PVOID)(in + buffers_at));
	}
	case RUXNPVG_IOCTL_SYNC: {
		struct ruxnpvg_sync *args = (PVOID)in;

		if (in_length < sizeof(*args))
			return E_INVAL;
		if (args->flags || !args->count || args->count > MAX_RESOURCES)
			return E_INVAL;
		if (in_length != sizeof(*args) + args->count * sizeof(ULONG))
			return E_INVAL;
		return pvg_task_sync(client, args, (PVOID)(in + sizeof(*args)));
	}
	case RUXNPVG_IOCTL_SURFACE_DESTROY: {
		struct ruxnpvg_surface_handle *args = (PVOID)in;
		struct pvg_surface *surface;

		if (in_length != sizeof(*args) || args->pad)
			return E_INVAL;
		surface = client_surface(client, args->handle);
		if (!surface)
			return E_NOENT;
		// A kind 11 object of this task holds it until the task ends.
		if (task_names_surface(client, surface))
			return STATUS_DEVICE_BUSY;
		client->surfaces[args->handle - 1] = NULL;
		pvg_surface_release(client, surface);
		return STATUS_SUCCESS;
	}
	case RUXNPVG_IOCTL_SURFACE_EXPORT: {
		struct ruxnpvg_surface_export *args = (PVOID)in;
		struct pvg_surface *surface;
		NTSTATUS status;

		if (in_length != sizeof(*args) || out_length < sizeof(*args) || args->pad)
			return E_INVAL;
		surface = client_surface(client, args->handle);
		if (!surface)
			return E_NOENT;
		status = pvg_surface_export(client, surface, &args->token);
		if (NT_SUCCESS(status)) {
			RtlCopyMemory(out, args, sizeof(*args));
			*returned = sizeof(*args);
		}
		return status;
	}
	case RUXNPVG_IOCTL_SURFACE_IMPORT: {
		struct ruxnpvg_surface_import *args = (PVOID)in;
		struct pvg_surface *surface;
		NTSTATUS status;
		ULONG handle;

		if (in_length != sizeof(*args) || out_length < sizeof(*args))
			return E_INVAL;
		if (client->dead || !client->pvg->mapper_regs)
			return E_NODEV;
		handle = free_surface_handle(client);
		if (!handle)
			return E_NOSPC;
		status = pvg_surface_import(client, args->token, &surface);
		if (!NT_SUCCESS(status))
			return status;
		put_surface(client, handle, surface);
		args->handle = handle;
		args->width = surface->width;
		args->height = surface->height;
		args->pitch = surface->pitch;
		args->size = surface->bytes;
		RtlCopyMemory(out, args, sizeof(*args));
		*returned = sizeof(*args);
		return STATUS_SUCCESS;
	}
	default:
		return STATUS_INVALID_DEVICE_REQUEST;
	}
}
