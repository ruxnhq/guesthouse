/* SPDX-License-Identifier: MIT */
/*
 * What a program asks of the pvg driver, the guest driver for Apple's
 * paravirtualized graphics device, through its DRM file.
 *
 * Version 3. Version 1's calls are as they were;
 * version 2 adds a submit that does not wait, a call that waits for it, and
 * the sharing of buffer objects through PRIME. Version 3 adds fences going
 * into a queued submit, DRM's sync objects, read fences, and a call that
 * says what a handle is.
 *
 * In short: every open file of the device has a task of its own on the host.
 * A program makes buffer objects, which are memory it maps and which lies in
 * its task, and objects, which are what the host's Metal makes for the task:
 * buffers, textures, functions, pipelines and other states. It writes command
 * buffers into its memory and submits them with the resources they use.
 */
#ifndef _PVG_DRM_H_
#define _PVG_DRM_H_

#include "drm.h"

#if defined(__cplusplus)
extern "C" {
#endif

#define DRM_PVG_GET_PARAM		0x00
#define DRM_PVG_BO_CREATE		0x01
#define DRM_PVG_OBJECT_CREATE		0x02
#define DRM_PVG_OBJECT_DESTROY		0x03
#define DRM_PVG_OBJECT_PLACEMENT	0x04
#define DRM_PVG_SUBMIT			0x05
#define DRM_PVG_SYNC			0x06
/* Version 2. */
#define DRM_PVG_QUEUE			0x07
#define DRM_PVG_WAIT			0x08
/* Version 3. */
#define DRM_PVG_BO_INFO			0x09

/* The version of this interface that this file describes. */
#define PVG_UAPI_VERSION		3

/* The device counts memory in pages of this size, whatever the guest's is. */
#define PVG_DEVICE_PAGE_SIZE		0x4000

/* For struct drm_pvg_get_param. */
#define PVG_PARAM_UAPI_VERSION		0
/* The protocol version the driver has set on the device. */
#define PVG_PARAM_PROTOCOL_VERSION	1
/* Object numbers run from 1 to this value less one. */
#define PVG_PARAM_MAX_OBJECTS		2
/* The most resources and command buffers one submit takes. */
#define PVG_PARAM_MAX_RESOURCES		3
#define PVG_PARAM_MAX_BUFFERS		4
/* How many bytes of buffer objects one file can have in its task. */
#define PVG_PARAM_TASK_BYTES		5
/* The longest description of an object, in bytes. */
#define PVG_PARAM_MAX_DESCRIPTION	6
/*
 * Version 2. Whether the file's task has a ring of its own, which is when
 * QUEUE returns before the GPU is done: 1 or 0. Asking defines the task.
 */
#define PVG_PARAM_OWN_RING		7
/*
 * How many queued submits of a file can be in flight at once. Since version
 * 3 the held-back ones count among them.
 */
#define PVG_PARAM_MAX_QUEUED		8
/* Version 3. What the driver has, as the bits PVG_FEATURE_*. */
#define PVG_PARAM_FEATURES		9
/* The most sync objects one QUEUE waits for, and the most it signals. */
#define PVG_PARAM_MAX_SYNCS		10
/* The largest plain buffer object, in bytes: 256 MiB since version 3, 64 MiB before. */
#define PVG_PARAM_MAX_BO_BYTES		11
/*
 * With PVG_FEATURE_PITCH_ALIGN: a surface's rows are width * 4 rounded up to
 * this many bytes, 16. EINVAL before, where the rows were width * 4.
 */
#define PVG_PARAM_PITCH_ALIGN		12

/* For PVG_PARAM_FEATURES. */
/* QUEUE takes a sync_file in fence_fd with PVG_QUEUE_FENCE_IN. */
#define PVG_FEATURE_FENCE_IN		(1 << 0)
/* QUEUE waits for and signals binary sync objects, with PVG_QUEUE_SYNCOBJS. */
#define PVG_FEATURE_SYNCOBJ		(1 << 1)
/* The same with timeline points; DRM's timeline calls are there too. */
#define PVG_FEATURE_SYNCOBJ_TIMELINE	(1 << 2)
/* Resources a submit reads get its fence as a read in their reservation. */
#define PVG_FEATURE_READ_FENCES		(1 << 3)
/* A fence that something waits on signals within about a millisecond. */
#define PVG_FEATURE_SIGNAL_SOON		(1 << 4)
/* BO_INFO is there. */
#define PVG_FEATURE_BO_INFO		(1 << 5)
/* OBJECT_CREATE takes kind 8 with operation 9, a texture over a buffer. */
#define PVG_FEATURE_TEXTURE_BUFFER	(1 << 6)
/* A surface's pitch is width * 4 rounded up to PVG_PARAM_PITCH_ALIGN bytes. */
#define PVG_FEATURE_PITCH_ALIGN		(1 << 7)
/*
 * The driver took the screen from the firmware when it started: it removed a
 * firmware framebuffer over the device (simpledrm's) and turned the device's
 * early framebuffer off before its ring was set up.
 */
#define PVG_FEATURE_FIRMWARE_SCREEN	(1 << 8)

struct drm_pvg_get_param {
	__u32 param;
	__u32 pad;
	__u64 value;	/* out */
};

/*
 * A buffer object is memory in the device's pages, zeroed, pinned, and for
 * the host to read and write. It is a GEM object: the handle is closed with
 * DRM_IOCTL_GEM_CLOSE, and the memory is mapped with mmap() on the DRM file
 * at the offset that comes back.
 *
 * Without flags it is memory that lies in the file's task, for command
 * buffers, vertices, a function's library, a texture's pixels.
 *
 * With PVG_BO_SURFACE it is a picture of width by height, four bytes a pixel
 * as blue, green, red, alpha, top row first, rows pitch bytes apart (width * 4
 * rounded up to 16 with PVG_FEATURE_PITCH_ALIGN), that the display can show and
 * that a texture object of kind PVG_OBJECT_SURFACE_TEXTURE can be laid over.
 * A dumb buffer of 32 bits a pixel is the same thing. A surface does not lie
 * in the task.
 */
#define PVG_BO_SURFACE			(1 << 0)

struct drm_pvg_bo_create {
	__u64 size;	/* in: bytes wanted, ignored for a surface; out: bytes got */
	__u32 flags;
	__u32 width;	/* in, for a surface */
	__u32 height;	/* in, for a surface */
	__u32 handle;	/* out */
	__u32 pitch;	/* out, for a surface: bytes from one row to the next */
	__u32 pad;
	__u64 address;	/* out: where it lies in the task; 0 for a surface */
	__u64 offset;	/* out: what to hand mmap() as its offset */
};

/*
 * An object's kind. The numbers are the device's own.
 */
#define PVG_OBJECT_BUFFER		1
#define PVG_OBJECT_TEXTURE		2
#define PVG_OBJECT_FUNCTION		6
#define PVG_OBJECT_STATE		7
/*
 * Version 2: a view of a texture of kind 2 or 11, which it names. With
 * PVG_FEATURE_TEXTURE_BUFFER also a texture over a buffer of kind 1.
 */
#define PVG_OBJECT_TEXTURE_VIEW		8
#define PVG_OBJECT_SURFACE_TEXTURE	11

/* For PVG_OBJECT_BUFFER: the host keeps the bytes, the guest's are not written. */
#define PVG_BUFFER_PRIVATE		(1 << 1)

/*
 * Makes an object in the file's task.
 *
 * The program chooses the object's number, since the description it writes
 * and the commands it sends name objects by number. The number has to be
 * free. Numbers are the file's own: two files can both have an object 1.
 *
 * A description has a head, which the kernel writes from handle, offset, size
 * and flags, and a tail, which is the program's bytes at data:
 *
 *   kind             handle is  the kernel writes         data holds
 *   BUFFER           memory     all 16 bytes              nothing
 *   TEXTURE          memory     12: length, page          the dimension, then
 *                                                         the operation for a
 *                                                         new texture
 *   FUNCTION         memory     12: address, length       the operation for a
 *                                                         new function
 *   STATE            0          nothing                   one operation
 *   SURFACE_TEXTURE  a surface  8: the mapping's id       the operation for a
 *                                                         texture on a surface
 *   TEXTURE_VIEW     0          nothing                   the operation for a
 *                                                         view, 7, 8 or 27;
 *                                                         or 9, a texture over
 *                                                         a buffer
 *
 * offset and size name the part of the buffer object the object uses: a
 * buffer's bytes, the memory a texture is loaded from and written back to,
 * a function's Metal library. For BUFFER and TEXTURE the offset has to be a
 * multiple of PVG_DEVICE_PAGE_SIZE, and for TEXTURE the size too. For STATE,
 * SURFACE_TEXTURE and TEXTURE_VIEW both are 0. A texture with views can't be
 * destroyed before them, nor a buffer with textures over it: EBUSY.
 *
 * The kernel checks what it writes and that the tail's operation names this
 * object's number. The host reads the description later, when a command
 * buffer first needs the object: a description it does not take fails that
 * submit.
 */
struct drm_pvg_object_create {
	__u32 id;
	__u32 kind;
	__u32 handle;
	__u32 flags;
	__u64 offset;
	__u64 size;
	__u64 data;		/* a pointer to the tail */
	__u32 data_size;	/* the tail's length in bytes */
	__u32 pad;
};

/*
 * Takes an object out of the task. Its number is free again when this
 * returns. An object that names a buffer object keeps that memory, or that
 * surface's mapping, until it is destroyed, also after the handle is closed.
 * A pipeline that was made keeps working without its functions.
 */
struct drm_pvg_object_destroy {
	__u32 id;
	__u32 pad;
};

/*
 * What the host wrote about an object when it made it: two words of 64 bits.
 * For a buffer the first reads as its address on the GPU. Both are 0 until
 * a command buffer has needed the object.
 */
struct drm_pvg_object_placement {
	__u32 id;
	__u32 pad;
	__u64 words[2];	/* out */
};

/* A resource's flag in a submit: the command buffers write it. */
#define PVG_RESOURCE_WRITTEN		(1 << 1)

/* A buffer, a texture or a surface texture that the command buffers use. */
struct drm_pvg_resource {
	__u32 id;
	__u32 flags;
};

/* Where a command buffer lies: size bytes at offset of a buffer object. */
struct drm_pvg_command_buffer {
	__u32 handle;
	__u32 pad;
	__u64 offset;
	__u64 size;
};

/*
 * Hands the device command buffers, which it runs in their order, and names
 * every resource they use. A command buffer is one segment: render, compute
 * or blit.
 *
 * Returns when the device has stamped the command, which it does when the GPU
 * has done the work: what was drawn is there.
 *
 * EIO says the device refused: a command buffer's bytes, or an object that
 * could not be made from its description. The task and its objects are as
 * they were, and the next submit can work.
 */
struct drm_pvg_submit {
	__u64 resources;	/* a pointer to resource_count struct drm_pvg_resource */
	__u64 buffers;		/* a pointer to buffer_count struct drm_pvg_command_buffer */
	__u32 resource_count;
	__u32 buffer_count;
	__u32 flags;		/* 0 */
	__u32 pad;
};

/*
 * Has the host write back, into the memory a texture names, what command
 * buffers have drawn into it. That is done only for a texture that a submit
 * named with PVG_RESOURCE_WRITTEN. A surface texture needs none of this:
 * what is drawn into it is in the surface's memory.
 */
struct drm_pvg_sync {
	__u64 ids;	/* a pointer to count object numbers, 32 bits each */
	__u32 count;
	__u32 flags;	/* 0 */
};

/*
 * Version 2. Hands the device command buffers as SUBMIT does, with the same
 * resources and checks, and returns when the command is in the task's ring:
 * before the GPU has done it. stamp is the submit's number, the file's own:
 * 1 for its first QUEUE and one more for each after. A ring runs its commands
 * in order, so when a number is done all lower ones are.
 *
 * Until the submit is done, the memory of its command buffers and of what
 * they read is the host's to read: write it again only after. The buffer
 * objects the command buffers lie in stay in the task until then, also when
 * their handles are closed.
 *
 * Each queued submit is a fence. Resources named PVG_RESOURCE_WRITTEN get it
 * in their memory's reservation as a write, so a page flip of that memory
 * waits for it; since version 3 the others get it as a read. With
 * PVG_QUEUE_FENCE_OUT, fence_fd comes back as a sync_file of it.
 *
 * Version 3: fences going in. With PVG_QUEUE_FENCE_IN, fence_fd names a
 * sync_file, and with PVG_QUEUE_SYNCOBJS, waits names wait_count sync objects
 * to wait for. The submit gets its number and its fence at once, and the call
 * returns at once, but the command goes into the ring only when every fence
 * going in has signalled, and after every earlier submit of the file. With
 * PVG_QUEUE_SYNCOBJS, signals names signal_count sync objects that get the
 * submit's fence. Both flags and FENCE_OUT can be given together: fence_fd
 * goes in and comes back. A program built against version 2's structure,
 * which ends at stamp, works as it did.
 *
 * Where the task has no ring of its own the call waits for the fences going
 * in, and then as SUBMIT does, and the number is done when it returns.
 */
#define PVG_QUEUE_FENCE_OUT		(1 << 0)
/* Version 3. */
#define PVG_QUEUE_FENCE_IN		(1 << 1)
#define PVG_QUEUE_SYNCOBJS		(1 << 2)

/*
 * Version 3. A DRM sync object to wait for or to signal, by its handle on
 * this file. point 0 is a binary sync object, or a timeline's last fence for
 * a wait; any other is a point on a timeline.
 */
struct drm_pvg_sync_op {
	__u32 handle;
	__u32 flags;	/* 0 */
	__u64 point;
};

struct drm_pvg_queue {
	__u64 resources;	/* a pointer to resource_count struct drm_pvg_resource */
	__u64 buffers;		/* a pointer to buffer_count struct drm_pvg_command_buffer */
	__u32 resource_count;
	__u32 buffer_count;
	__u32 flags;		/* PVG_QUEUE_* or 0 */
	/*
	 * In, with PVG_QUEUE_FENCE_IN: a sync_file to wait for. Out, with
	 * PVG_QUEUE_FENCE_OUT: a sync_file of the submit; else -1.
	 */
	__s32 fence_fd;
	__u64 stamp;		/* out: the submit's number */
	/* Version 3, with PVG_QUEUE_SYNCOBJS; else 0. */
	__u64 waits;		/* a pointer to wait_count struct drm_pvg_sync_op */
	__u64 signals;		/* a pointer to signal_count struct drm_pvg_sync_op */
	__u32 wait_count;
	__u32 signal_count;
};

/*
 * Version 2. Sleeps until the file's submit with this number is done, or
 * until timeout_ns, a time on CLOCK_MONOTONIC, has come; 0 only looks.
 * 0: done. EIO: the device refused it. ETIMEDOUT: the time came first.
 * EINVAL: a number not handed out yet.
 */
struct drm_pvg_wait {
	__u64 stamp;
	__s64 timeout_ns;
	__u32 flags;	/* 0 */
	__u32 pad;
};

/*
 * Version 3. What a handle of this file is: for a program that took it in
 * through PRIME and has to know what it got. flags comes back as
 * PVG_BO_SURFACE for a surface, with width, height and pitch; as
 * PVG_BO_INFO_OTHER_DEVICE for a dma-buf of another device, which can only be
 * a framebuffer; as 0 for a plain buffer object, with the address it lies at
 * in this file's task. offset is what to hand mmap() on the DRM file, for a
 * buffer of this device.
 */
#define PVG_BO_INFO_OTHER_DEVICE	(1 << 31)

struct drm_pvg_bo_info {
	__u32 handle;	/* in */
	__u32 flags;	/* out */
	__u32 width;	/* out, for a surface */
	__u32 height;	/* out, for a surface */
	__u32 pitch;	/* out, for a surface */
	__u32 pad;
	__u64 size;	/* out: bytes */
	__u64 offset;	/* out: what to hand mmap() as its offset */
	__u64 address;	/* out: where it lies in the task; 0 for a surface */
};

#define DRM_IOCTL_PVG_GET_PARAM \
	DRM_IOWR(DRM_COMMAND_BASE + DRM_PVG_GET_PARAM, struct drm_pvg_get_param)
#define DRM_IOCTL_PVG_BO_CREATE \
	DRM_IOWR(DRM_COMMAND_BASE + DRM_PVG_BO_CREATE, struct drm_pvg_bo_create)
#define DRM_IOCTL_PVG_OBJECT_CREATE \
	DRM_IOW(DRM_COMMAND_BASE + DRM_PVG_OBJECT_CREATE, struct drm_pvg_object_create)
#define DRM_IOCTL_PVG_OBJECT_DESTROY \
	DRM_IOW(DRM_COMMAND_BASE + DRM_PVG_OBJECT_DESTROY, struct drm_pvg_object_destroy)
#define DRM_IOCTL_PVG_OBJECT_PLACEMENT \
	DRM_IOWR(DRM_COMMAND_BASE + DRM_PVG_OBJECT_PLACEMENT, struct drm_pvg_object_placement)
#define DRM_IOCTL_PVG_SUBMIT \
	DRM_IOW(DRM_COMMAND_BASE + DRM_PVG_SUBMIT, struct drm_pvg_submit)
#define DRM_IOCTL_PVG_SYNC \
	DRM_IOW(DRM_COMMAND_BASE + DRM_PVG_SYNC, struct drm_pvg_sync)
#define DRM_IOCTL_PVG_QUEUE \
	DRM_IOWR(DRM_COMMAND_BASE + DRM_PVG_QUEUE, struct drm_pvg_queue)
#define DRM_IOCTL_PVG_WAIT \
	DRM_IOW(DRM_COMMAND_BASE + DRM_PVG_WAIT, struct drm_pvg_wait)
#define DRM_IOCTL_PVG_BO_INFO \
	DRM_IOWR(DRM_COMMAND_BASE + DRM_PVG_BO_INFO, struct drm_pvg_bo_info)

#if defined(__cplusplus)
}
#endif

#endif
