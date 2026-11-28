// SPDX-License-Identifier: MIT
//
// The control interface of ruxnpvg, shared by the driver, pvgtest and
// pvgtri. The device is \\.\ruxnpvg, open to administrators and the system
// only. Include <ntddk.h> or <windows.h> and <winioctl.h> first.

#ifndef RUXNPVG_IOCTL_H
#define RUXNPVG_IOCTL_H

#define RUXNPVG_DEVICE_NAME	L"\\Device\\ruxnpvg"
#define RUXNPVG_LINK_NAME	L"\\DosDevices\\ruxnpvg"
#define RUXNPVG_USER_PATH	L"\\\\.\\ruxnpvg"

// Out: struct ruxnpvg_info.
#define RUXNPVG_IOCTL_INFO \
	CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_READ_ACCESS)
// In: struct ruxnpvg_nop_in. Out: struct ruxnpvg_nop_out.
#define RUXNPVG_IOCTL_NOP \
	CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS)
// Out: struct ruxnpvg_task_info.
#define RUXNPVG_IOCTL_TASK_INFO \
	CTL_CODE(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_READ_ACCESS)
// Out: struct ruxnpvg_display_info.
#define RUXNPVG_IOCTL_DISPLAY_INFO \
	CTL_CODE(FILE_DEVICE_UNKNOWN, 0x803, METHOD_BUFFERED, FILE_READ_ACCESS)

// The most command 30s one NOP control sends.
#define RUXNPVG_NOP_MAX		100000

struct ruxnpvg_info {
	ULONG version;		// as read back at the last start
	ULONG ring_on;
	ULONG written;		// bytes the driver has written, a running count
	ULONG waited;		// how far the device is known to have read
	ULONG last_stamp;
	ULONG stamp_cell;	// the ring's first word now
	ULONG64 sent;		// commands written into the ring
	ULONG64 late;		// stamps found with no interrupt for them
	ULONG64 rewinds;
	ULONG64 refused;
	ULONG64 irq_taken;	// message 0
	ULONG64 irq_done;
	ULONG64 irq_events;
	ULONG64 irq_faults;
	ULONG64 irq_empty;
	ULONG64 irq_other;	// message 1
};

struct ruxnpvg_nop_in {
	ULONG count;		// 1 to RUXNPVG_NOP_MAX
};

struct ruxnpvg_nop_out {
	ULONG through;		// commands whose stamp came back
	ULONG late;		// of those, found with no interrupt for them
	ULONG64 elapsed_us;
	LONG first_error;	// an NTSTATUS, 0 for none
	ULONG first_error_at;	// the command it came with, from 0
};

// What the tasks have done since the driver loaded. Buffer objects "kept"
// are ones the driver left allocated because they could not be unmapped
// in their own process or a task the device would not let go named them.
struct ruxnpvg_task_info {
	ULONG64 defined;	// tasks the device was told of
	ULONG64 deleted;	// and told to forget
	ULONG64 lost;		// that the device would not let go
	ULONG64 submits;
	ULONG64 refused;	// submits the device refused
	ULONG64 bos;		// buffer objects made
	ULONG64 bos_freed;
	ULONG64 bos_kept;
	ULONG64 unmapped;	// mappings undone in their own process
	ULONG64 bo_bytes;	// bytes of buffer objects allocated now
	ULONG64 open_files;	// handles open now
};

// The task controls. They mirror version 1 of the Linux driver's interface,
// tools/pvg-linux/uapi/pvg_drm.h at 377700a: GET_PARAM, BO_CREATE,
// OBJECT_CREATE, OBJECT_DESTROY, OBJECT_PLACEMENT, SUBMIT and SYNC, with the
// same fields and checks. Where Linux has a pointer to an array, the array
// here follows the fixed part in the same buffered input, so the driver
// reads no memory of the caller's but what the I/O manager copied in, and
// the input's length must be the fixed part's and the array's exactly.
//
// Every open handle has a task of its own on the device, made when the
// handle first needs it. A handle may be used only by the process that
// opened it. Closing it deletes the task and frees all it holds.
#define RUXNPVG_TASK_IOCTL(n) \
	CTL_CODE(FILE_DEVICE_UNKNOWN, 0x810 + (n), METHOD_BUFFERED, \
		 FILE_READ_ACCESS | FILE_WRITE_ACCESS)
// In and out: struct ruxnpvg_get_param.
#define RUXNPVG_IOCTL_GET_PARAM		RUXNPVG_TASK_IOCTL(0)
// In and out: struct ruxnpvg_bo_create.
#define RUXNPVG_IOCTL_BO_CREATE		RUXNPVG_TASK_IOCTL(1)
// In: struct ruxnpvg_object_create and data_size bytes of the tail.
#define RUXNPVG_IOCTL_OBJECT_CREATE	RUXNPVG_TASK_IOCTL(2)
// In: struct ruxnpvg_object_destroy.
#define RUXNPVG_IOCTL_OBJECT_DESTROY	RUXNPVG_TASK_IOCTL(3)
// In and out: struct ruxnpvg_object_placement.
#define RUXNPVG_IOCTL_OBJECT_PLACEMENT	RUXNPVG_TASK_IOCTL(4)
// In: struct ruxnpvg_submit, resource_count struct ruxnpvg_resource, then
// buffer_count struct ruxnpvg_command_buffer.
#define RUXNPVG_IOCTL_SUBMIT		RUXNPVG_TASK_IOCTL(5)
// In: struct ruxnpvg_sync and count object numbers of 32 bits.
#define RUXNPVG_IOCTL_SYNC		RUXNPVG_TASK_IOCTL(6)
// The surface and display controls, which Linux has as DRM's own calls.
// In: struct ruxnpvg_surface_handle.
#define RUXNPVG_IOCTL_SURFACE_DESTROY	RUXNPVG_TASK_IOCTL(7)
// No input. One handle owns the screen at a time.
#define RUXNPVG_IOCTL_DISPLAY_TAKE	RUXNPVG_TASK_IOCTL(8)
// In and out: struct ruxnpvg_display_swap. The owner only.
#define RUXNPVG_IOCTL_DISPLAY_SWAP	RUXNPVG_TASK_IOCTL(9)
// No input. Gives the screen back to the early framebuffer.
#define RUXNPVG_IOCTL_DISPLAY_RELEASE	RUXNPVG_TASK_IOCTL(10)
// Out: struct ruxnpvg_efb. Reads the early framebuffer's bank, nothing else.
#define RUXNPVG_IOCTL_EFB_READ		RUXNPVG_TASK_IOCTL(11)
// Out: struct ruxnpvg_efb. Takes the screen with the early framebuffer
// turned off and no display; DISPLAY_RELEASE turns it back on.
#define RUXNPVG_IOCTL_EFB_OFF		RUXNPVG_TASK_IOCTL(12)
// In and out: struct ruxnpvg_surface_export. The surface's maker only.
#define RUXNPVG_IOCTL_SURFACE_EXPORT	RUXNPVG_TASK_IOCTL(13)
// In and out: struct ruxnpvg_surface_import.
#define RUXNPVG_IOCTL_SURFACE_IMPORT	RUXNPVG_TASK_IOCTL(14)
#define RUXNPVG_TASK_IOCTLS		15

// The version of the task controls this file describes: Linux's version 1.
#define RUXNPVG_UAPI_VERSION		1
// The device counts memory in pages of this size, whatever the guest's is.
#define RUXNPVG_DEVICE_PAGE_SIZE	0x4000
// The largest buffer object. Linux takes 64 MiB; this driver takes less.
#define RUXNPVG_BO_MAX_BYTES		(16ULL << 20)
// How many buffer objects one handle can have.
#define RUXNPVG_MAX_BOS			256

// For struct ruxnpvg_get_param, as Linux's PVG_PARAM_*.
#define RUXNPVG_PARAM_UAPI_VERSION	0
#define RUXNPVG_PARAM_PROTOCOL_VERSION	1
#define RUXNPVG_PARAM_MAX_OBJECTS	2
#define RUXNPVG_PARAM_MAX_RESOURCES	3
#define RUXNPVG_PARAM_MAX_BUFFERS	4
#define RUXNPVG_PARAM_TASK_BYTES	5
#define RUXNPVG_PARAM_MAX_DESCRIPTION	6

struct ruxnpvg_get_param {
	ULONG param;
	ULONG pad;
	ULONG64 value;		// out
};

// Memory in device pages, zeroed, for the host to read and write, lying in
// the handle's task and mapped into the caller. It stays until the handle
// is closed.
//
// With RUXNPVG_BO_SURFACE in flags it is a surface instead, as Linux's
// PVG_BO_SURFACE: width by height pixels of four bytes, rows of pitch
// bytes, mapped into the caller and not into the task. The display shows
// it, and a texture of kind 11 draws into it. Its handle is one of the
// handle's surfaces, numbered apart from the buffer objects; size is ignored
// on the way in, and address is 0.
#define RUXNPVG_BO_SURFACE		1
// With RUXNPVG_BO_SURFACE: the surface is mapped into no process, so only
// the GPU writes it; user_address is 0.
#define RUXNPVG_BO_NO_CPU		2
// At most this many bytes and pixels a side; Linux takes 64 MiB and 8192.
#define RUXNPVG_SURFACE_MAX_BYTES	(16ULL << 20)
#define RUXNPVG_SURFACE_MAX_SIDE	8192
// How many surfaces one handle can have.
#define RUXNPVG_MAX_SURFACES		16

struct ruxnpvg_bo_create {
	ULONG64 size;		// in: bytes wanted; out: bytes got
	ULONG flags;
	ULONG width;
	ULONG height;
	ULONG handle;		// out: from 1
	ULONG pitch;		// out: 0
	ULONG pad;
	ULONG64 address;	// out: where it lies in the task
	ULONG64 user_address;	// out: where it is mapped in the caller
};

// An object's kind: the device's numbers, as in Linux.
#define RUXNPVG_OBJECT_BUFFER		1
#define RUXNPVG_OBJECT_TEXTURE		2
#define RUXNPVG_OBJECT_FUNCTION		6
#define RUXNPVG_OBJECT_STATE		7
// A texture over a surface. Its description's head is the surface's
// mapping id, which the driver writes. It cannot be destroyed by itself:
// the handle's end takes it away.
#define RUXNPVG_OBJECT_SURFACE_TEXTURE	11

// For RUXNPVG_OBJECT_BUFFER: the host keeps the bytes.
#define RUXNPVG_BUFFER_PRIVATE		(1 << 1)

// Makes an object in the handle's task, as Linux's OBJECT_CREATE: the
// driver writes the description's head from handle, offset, size and
// flags, and the tail follows this struct.
struct ruxnpvg_object_create {
	ULONG id;
	ULONG kind;
	ULONG handle;
	ULONG flags;
	ULONG64 offset;
	ULONG64 size;
	ULONG data_size;	// the tail's length in bytes
	ULONG pad;
};

struct ruxnpvg_object_destroy {
	ULONG id;
	ULONG pad;
};

struct ruxnpvg_object_placement {
	ULONG id;
	ULONG pad;
	ULONG64 words[2];	// out
};

// A resource's flag in a submit: the command buffers write it.
#define RUXNPVG_RESOURCE_WRITTEN	(1 << 1)

struct ruxnpvg_resource {
	ULONG id;
	ULONG flags;
};

struct ruxnpvg_command_buffer {
	ULONG handle;
	ULONG pad;
	ULONG64 offset;
	ULONG64 size;
};

// Returns when the device has stamped the command, which it does when the
// GPU has done the work. A refusal comes back as STATUS_DEVICE_DATA_ERROR
// (Win32 error 23), as EIO does in Linux.
struct ruxnpvg_submit {
	ULONG resource_count;
	ULONG buffer_count;
	ULONG flags;		// 0
	ULONG pad;
};

struct ruxnpvg_sync {
	ULONG count;
	ULONG flags;		// 0
};

struct ruxnpvg_surface_handle {
	ULONG handle;
	ULONG pad;
};

// Sharing a surface between handles, as a compositor needs: the maker
// exports it and gets a token, a number of 64 bits drawn at random, which
// it hands to another program however it likes; that program imports it
// with the token and gets a handle of its own for the same surface. The
// importer may lay a kind 11 object over it in its own task, draw with it
// and show it, but the surface is never mapped into its process, so no
// program but the maker can write its pixels from the CPU. Exporting again
// gives the same token. The token stops working once the maker lets the
// surface go; what importers hold stays until they let it go too.
struct ruxnpvg_surface_export {
	ULONG handle;		// in
	ULONG pad;
	ULONG64 token;		// out
};

struct ruxnpvg_surface_import {
	ULONG64 token;		// in
	ULONG handle;		// out: one of this handle's surfaces
	ULONG width;		// out
	ULONG height;		// out
	ULONG pitch;		// out
	ULONG64 size;		// out
};

// Shows a surface on the display. Returns once the device has stamped the
// command; swap_done says whether its event was seen by then.
struct ruxnpvg_display_swap {
	ULONG handle;		// in
	ULONG swap_done;	// out
};

// The early framebuffer's bank as read: the block's first page, its
// length in bytes, the bits a pixel and the mode, height above width.
struct ruxnpvg_efb {
	ULONG page;
	ULONG bytes;
	ULONG depth;
	ULONG mode;
};

// What the mapper, the surfaces and the display have done since the
// driver loaded. A balanced run ends with takes = releases, task0_defined =
// task0_deleted, hand_overs = let_gos, maps = unmaps, mapped_now =
// held_now = 0, surfaces = surfaces_freed, surface_bytes 0, mapper_irqs =
// mapper_commands, bank_restores = bank_offs, imports = import_releases
// and exported_now = holders_now = 0.
struct ruxnpvg_display_info {
	ULONG64 mapper_started;		// times the mapper was told of a ring
	ULONG64 mapper_commands;
	ULONG64 mapper_irqs;		// message 1
	ULONG64 mapper_early;		// done before the sender waited
	ULONG64 mapper_late;		// found done with no interrupt for it
	ULONG64 mapper_timeouts;
	ULONG64 maps;
	ULONG64 unmaps;
	ULONG64 outcomes_busy;		// the display still showed it
	ULONG64 outcomes_other;
	ULONG64 ids_passed;		// left mapped by a driver before
	ULONG64 mapped_now;
	ULONG64 held_now;
	ULONG64 mapper_written;		// the mapper's count as last written
	ULONG64 surfaces;
	ULONG64 surfaces_freed;
	ULONG64 surfaces_kept;		// left allocated, as buffer objects can be
	ULONG64 surface_bytes;		// allocated now
	ULONG64 takes;			// display and early framebuffer both
	ULONG64 releases;
	ULONG64 owner_process;		// 0 when nobody owns the screen
	ULONG64 owner_mode;		// 0 none, 1 early framebuffer off, 2 display
	ULONG64 spoiled;		// a display command failed: no more taking
	ULONG64 task0_defined;
	ULONG64 task0_deleted;
	ULONG64 hand_overs;
	ULONG64 let_gos;
	ULONG64 mode_lists;
	ULONG64 acknowledgements;
	ULONG64 swaps;
	ULONG64 swap_done;		// swap-done events taken
	ULONG64 events_taken;		// non-zero event words exchanged
	ULONG64 display_failed;		// display commands that failed
	ULONG64 bank_reads;
	ULONG64 bank_offs;
	ULONG64 bank_restores;
	ULONG64 shown_id;		// the mapping last swapped to
	ULONG64 first_mode;		// the device's first mode, height above width
	struct ruxnpvg_efb bank;	// as last read
	ULONG64 exports;		// tokens handed out
	ULONG64 imports;
	ULONG64 imports_refused;	// no exported surface had the token
	ULONG64 import_releases;	// importers that let theirs go
	ULONG64 exported_now;		// surfaces whose token works now
	ULONG64 holders_now;		// handles' holds on surfaces now
	ULONG64 outlived_maker;		// surfaces still held when their maker let go
};

#endif
