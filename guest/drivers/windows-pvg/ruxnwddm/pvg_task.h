// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// What pvg_task.c, pvg_object.c and pvg_client.c share: the task's layout
// and limits, the serializer's operations, and the helpers each uses. Split
// out of ruxnpvg's task.c, which held all three.

#ifndef RUXNWDDM_PVG_TASK_H
#define RUXNWDDM_PVG_TASK_H

#include "pvg.h"

#define POOL_TAG_TASK	'txur'
#define POOL_TAG_TEMP	'mxur'

// How long a command that hands the GPU work is waited for: its stamp
// comes when the work is done.
#define TASK_TIMEOUT_MS		10000

// Task ids run from 0 to 255, and 0 is the kernel task.
#define TASK_FIRST_ID		1
#define TASK_LAST_ID		255

// Words of a task's root page.
#define ROOT_TABLE		0
#define ROOT_DEPTH		1
#define ROOT_OBJECTS		2	// an address of 64 bits
#define ROOT_OBJECTS_BYTES	4
#define ROOT_PLACEMENTS		7	// an address of 64 bits
#define ROOT_PLACEMENTS_BYTES	9

// The object list has 12 bytes for each object number: the kind in a byte,
// the description's length in three, and the description's address in 64
// bits. The placement list has 16 bytes for each, and the device takes the
// pair only if both have room for the same count.
#define OBJECT_WORDS		3
#define PLACEMENT_WORDS		2
#define OBJECTS_BYTES		(TASK_MAX_OBJECTS * OBJECT_WORDS * 4)
#define PLACEMENTS_BYTES	(TASK_MAX_OBJECTS * PLACEMENT_WORDS * 8)

// The driver's part of a task, in the task's pages: each list over whole
// pages of its own.
#define TASK_OBJECTS_PAGE	0
#define TASK_PLACEMENTS_PAGE	(TASK_OBJECTS_PAGE + TASK_OBJECT_LIST_PAGES)
#define TASK_DESCRIPTIONS_PAGE	(TASK_PLACEMENTS_PAGE + TASK_PLACEMENT_LIST_PAGES)

#define MAX_DESCRIPTION		0x4000
#define MAX_HEAD		16
#define MAX_RESOURCES		256
#define MAX_BUFFERS		16

// The serializer's operations that make an object, as a description's tail
// holds them, and the delete operation that belongs to each.
#define OP_SAMPLER		3
#define OP_DEPTH_STENCIL	4
#define OP_COMPUTE_PIPELINE	11
#define OP_RENDER_PIPELINE	14
#define OP_FUNCTION		15
// The serializer's deleteTextureRef: a kind 8 object (a view, a texture
// over a buffer), by command 40 (scene view-delete).
#define OP_DELETE_TEXTURE	1001
#define OP_DELETE_DEPTH_STENCIL	1002
#define OP_DELETE_SAMPLER	1003
#define OP_DELETE_FUNCTION	1005
#define OP_DELETE_COMPUTE_PIPELINE	1006
#define OP_DELETE_RENDER_PIPELINE	1007
#define OP_DELETE_BYTES		12
// The operation of a texture over a surface, its length, and where in it
// the texture's width and height are.
#define OP_SURFACE_TEXTURE	12
#define OP_SURFACE_TEXTURE_BYTES	48
#define OP_SURFACE_WIDTH	16
#define OP_SURFACE_HEIGHT	20
// The operation of a view of a whole texture in a format of its own, its
// length, and where in it the base texture's number is.
#define OP_TEXTURE_VIEW		7
#define OP_TEXTURE_VIEW_BYTES	20
#define OP_VIEW_BASE		12

// Linux's error numbers, as this driver returns them.
#define E_INVAL		STATUS_INVALID_PARAMETER
#define E_NOENT		STATUS_NOT_FOUND
#define E_EXIST		STATUS_OBJECT_NAME_COLLISION
#define E_NOSPC		STATUS_INSUFFICIENT_RESOURCES
#define E_NOMEM		STATUS_INSUFFICIENT_RESOURCES
#define E_NODEV		STATUS_DEVICE_NOT_READY
#define E_BUSY		STATUS_DEVICE_BUSY
#define E_IO		STATUS_DEVICE_DATA_ERROR	// the device refused

C_ASSERT(TASK_DESCRIPTIONS_PAGE + TASK_DESCRIPTION_PAGES <= TASK_FIRST_BO_PAGE);
C_ASSERT(TASK_TABLES <= TASK_TABLE_ENTRIES);
C_ASSERT(TASK_DESCRIPTION_SLOTS % 32 == 0);

static __forceinline ULONG lower_32(ULONG64 value)
{
	return (ULONG)value;
}

static __forceinline ULONG upper_32(ULONG64 value)
{
	return (ULONG)(value >> 32);
}

static __forceinline void put_le32(UCHAR *at, ULONG value)
{
	at[0] = (UCHAR)value;
	at[1] = (UCHAR)(value >> 8);
	at[2] = (UCHAR)(value >> 16);
	at[3] = (UCHAR)(value >> 24);
}

static __forceinline void put_le64(UCHAR *at, ULONG64 value)
{
	put_le32(at, lower_32(value));
	put_le32(at + 4, upper_32(value));
}

static __forceinline ULONG get_le32(const UCHAR *at)
{
	return (ULONG)at[0] | (ULONG)at[1] << 8 | (ULONG)at[2] << 16 | (ULONG)at[3] << 24;
}


// Shared by pvg_task.c, pvg_object.c and pvg_client.c.
NTSTATUS client_task(RUXNWDDM_CLIENT *client, struct pvg_task **task);
NTSTATUS task_place(struct pvg_task *task, struct pvg_bo *bo);
BOOLEAN task_unplace(RUXNWDDM_PVG *pvg, struct pvg_task *task, struct pvg_bo *bo);
NTSTATUS task_memory(RUXNWDDM_CLIENT *client, ULONG handle, ULONG64 offset, ULONG64 size,
		     struct pvg_bo **bo);
NTSTATUS description_alloc(struct pvg_task *task, ULONG bytes, ULONG *slot_out,
			   ULONG *slots_out);
void description_write(struct pvg_task *task, ULONG slot, const UCHAR *data, ULONG bytes);
volatile ULONG *object_word(struct pvg_task *task, ULONG id, ULONG word);
volatile ULONG64 *placement_words(struct pvg_task *task, ULONG id);
void object_release(struct pvg_task *task, struct pvg_object *object);
BOOLEAN tail_is_operation(const UCHAR *tail, ULONG bytes, ULONG id);
struct pvg_object *task_object(struct pvg_task *task, ULONG id);
NTSTATUS pvg_task_submit(RUXNWDDM_CLIENT *client, const struct ruxnpvg_submit *args,
			 const struct ruxnpvg_resource *resources, const struct ruxnpvg_command_buffer *buffers);
NTSTATUS pvg_task_sync(RUXNWDDM_CLIENT *client, const struct ruxnpvg_sync *args, const ULONG *ids);
BOOLEAN pvg_surface_texture_fits(const UCHAR *tail, ULONG surface_width);
void pvg_view_note_texture(struct pvg_object *object, const UCHAR *tail, ULONG size);
NTSTATUS pvg_view_check(struct pvg_task *task, const struct pvg_object *self, ULONG id, const UCHAR *tail,
			ULONG size);
BOOLEAN kind_is_resource(UCHAR kind);
struct pvg_surface *client_surface(RUXNWDDM_CLIENT *client, ULONG handle);
BOOLEAN task_names_surface(RUXNWDDM_CLIENT *client, const struct pvg_surface *surface);
void task_end(RUXNWDDM_PVG *pvg, struct pvg_task *task);
void task_free(RUXNWDDM_PVG *pvg, struct pvg_task *task);

#endif
