// SPDX-License-Identifier: MIT
//
// What the parts of winemetal over ruxnpvg share: the constants, the object
// table and its kinds, the task's memory, and the helpers each part calls
// in another. Not for anything outside winemetal.dll.

#ifndef WM_INTERNAL_H
#define WM_INTERNAL_H

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winioctl.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

#define WINEMETAL_API __declspec(dllexport)
#define WIN_EXPORT
#include "winemetal.h"
#include "airconv_public.h"
#include "ruxnpvg_ioctl.h"
#include "pvg_wire.h"
#include "pvg_wire_more.h"
#include "translate.h"
#include "winemetal_pvg.h"
#include "pvg_kmt.h"



#define PAGE RUXNPVG_DEVICE_PAGE_SIZE
#define CHUNK_BYTES (4u << 20)          // buffer objects are made this big, or bigger for one object
#define COMMAND_BYTES (64u * PAGE)      // a command buffer's room for its segments (1 MiB)
#define MOST_SEGMENTS 16
#define MOST_SIGNALS 16
#define MOST_OBJECTS 20480
// Handles only the work of a frame takes (command buffers, encoders, pools,
// strings): a resource or state is refused while fewer than this many are
// left, so a draw never finds the table full (run rn83: the table filled
// with textures and the next frame's command buffers had no handle, which
// drew nothing and read back a wrong colour).
#define HANDLES_KEPT_FOR_WORK 1024
// As many buffer objects as the kernel gives a process (RUXNPVG_MAX_BOS);
// a task's 1 GiB holds about 251 chunks of 4 MiB and their tails, and the
// kernel refuses the next cleanly (64 stopped memgrow at 256 MB, rn81).
// 1024 for a task of up to 4 GiB (Parameters\TaskMiB, pvg.h).
#define MOST_CHUNKS 1024
#define MOST_WAITS 64
#define TASK_NUMBERS_KEPT 16384	// as many as a task has (the driver's TASK_MAX_OBJECTS)


#define CALLED()                                         \
	do {                                             \
		static LONG once;                        \
		if (!InterlockedExchange(&once, 1))      \
			pvg_log("called: %s", __func__); \
	} while (0)

// ---------------------------------------------------------------------
// Objects. A handle is an index into this table; nothing is ever freed
// (the task and its memory go when the process closes its handle).

enum kind {
	K_FREE,
	K_DEVICE,
	K_ARRAY,
	K_STRING,
	K_POOL,
	K_ERROR,
	K_QUEUE,
	K_COMMAND_BUFFER,
	K_ENCODER,
	K_BUFFER,
	K_TEXTURE,
	K_DATA,
	K_LIBRARY,
	K_FUNCTION,
	K_RENDER_PIPELINE,
	K_COMPUTE_PIPELINE,
	K_DEPTH_STENCIL,
	K_SAMPLER,
	K_EVENT,
	K_FENCE,
	K_LISTENER,
	K_PLACEHOLDER,
	K_SHADER,
	K_OTHER,
};

// A buffer object of the driver's, with one object of kind 1 over all of
// it: every buffer DXMT asks for is a range of a chunk, named by the
// chunk's object and an offset, as KosmicKrisp's memory is one buffer over
// a heap. Its GPU address is the host's, from the placement list
// (wm_place.c).
struct chunk {
	ULONG handle;
	UCHAR *user;
	ULONG64 task_address;
	ULONG64 size, used;
	uint32_t number;	// the kind 1 object over the whole chunk
	ULONG64 gpu_address;	// the host's address of its first byte
	unsigned live;		// ranges handed out and not given back
	bool whole;		// made for one object bigger than CHUNK_BYTES (wm_memory.c)
};

// What a range is for, counted while it lives (wm_memory.c).
enum wm_use { WM_USE_BUFFER, WM_USE_TEXTURE, WM_USE_COMMANDS, WM_USE_LIBRARY, WM_USE_HELPER, WM_USES };

struct memory {
	struct chunk *chunk;
	ULONG64 offset;   // in the buffer object
	ULONG64 length;
	bool zeroed;	// handed out for the first time, so all zero (wm_memory.c)
	unsigned char use;	// an enum wm_use
};

struct command_buffer {
	struct memory memory;
	struct pvg_wire wire;
	struct pvgx_resources resources;
	ULONG64 segment_offset[MOST_SEGMENTS], segment_size[MOST_SEGMENTS];
	unsigned segments;
	obj_handle_t signal_event[MOST_SIGNALS];
	uint64_t signal_value[MOST_SIGNALS];
	unsigned signals;
	obj_handle_t open_encoder;
	bool failed, committed;
	uint64_t inline_low;	// bytes commands carry lie from here to the end (translate.c)
	enum WMTCommandBufferStatus status;
	// The buffers and textures its commands name, held until it is
	// done, as a Metal command buffer holds them: their memory must not
	// serve another object before the device has read it (wm_memory.c).
	obj_handle_t *held;
	unsigned held_count, held_room;
};

enum encoder_kind { E_RENDER, E_BLIT, E_COMPUTE };

struct encoder {
	obj_handle_t command_buffer;
	enum encoder_kind kind;
	struct pvgx_encoder x;
	struct pvg_wire before;   // the writer as it was before the segment began
	ULONG64 segment;
	bool ended;
};

struct object {
	enum kind kind;
	LONG references;
	union {
		struct { char *text; } string;
		struct { obj_handle_t first; uint64_t count; } array;
		struct { obj_handle_t *held; unsigned count, room; obj_handle_t outer; } pool;
		struct { struct memory memory; uint32_t number; bool fresh; } buffer;
		struct {
			struct memory memory;
			uint32_t number;
			uint32_t view_number;	// over a surface: the view that has its resource id
			uint32_t listed;	// over a buffer or a level view: what command 67 lists
			uint32_t base, base_level;	// a view: the texture it views, and its first level
			uint32_t base_slice;		// and its first slice (a face of a cube)
			uint16_t device_levels;		// the levels the device has (wm_resources.c, compressed)
			uint64_t resource_id;	// the host's, from the placement list
			bool view;		// another texture's handle: its number is that one's
			bool own_view;		// a kind 8 object of its own (a view): deleted with it
			obj_handle_t over;	// the buffer it lies over, held, as Metal holds it
			// Depth32Float_Stencil8 is made as two textures the device
			// takes, depth and stencil: the stencil's number and memory.
			uint32_t stencil_number;
			struct memory stencil_memory;
			struct WMTTextureInfo info;
			bool surface;
			ULONG surface_handle;	// over a surface: the driver's handle of it
		} texture;
		struct { const void *data; uint64_t length; } data;
		struct { struct memory memory; bool placeholder; uint32_t function; } library;
		struct { uint32_t number; bool placeholder; } function;
		struct { uint32_t number; bool blending; } pipeline;
		struct { uint32_t number; } state;
		struct { uint64_t value; } event;
		struct { int which; } shader;   // 0 vertex, 1 pixel
		struct command_buffer *command_buffer;
		struct encoder *encoder;
	} u;
};


#define MOST_POOLED_COMMAND_MEMORY 32

struct wait {
	obj_handle_t event;
	HANDLE handle;
	uint64_t value;
};

extern ULONG64 max_objects, max_resources, max_buffers;
extern CRITICAL_SECTION lock;
bool wm_process_ending(void);	// the process is shutting down its DLLs
void note_once(const char *what);
bool allocate(ULONG64 length, enum wm_use use, struct memory *out);
void *address(const struct memory *m);
void wm_give_back(struct memory *m);
void wm_hold(struct command_buffer *cb, obj_handle_t h);
void wm_let_go(struct command_buffer *cb);
uint32_t wm_depth_format(uint32_t format);
uint32_t wm_stencil_format(uint32_t format);
uint64_t wm_allocated_bytes(void);
void wm_memory_say_live(void);
bool wm_destroy_task_object(uint32_t number);
unsigned wm_chunk_index(const struct chunk *c);
void wm_memory_report(void);
bool create_object(uint32_t number, ULONG kind, const struct memory *m, ULONG flags, const void *tail,
			  ULONG tail_size);
extern uint32_t free_numbers[TASK_NUMBERS_KEPT];
extern unsigned free_number_count;
extern struct object objects[MOST_OBJECTS];
extern unsigned object_count;
obj_handle_t new_object(enum kind kind);
struct object *get(obj_handle_t handle, enum kind kind);
obj_handle_t new_string(const char *text);
void set_error(obj_handle_t *out, const char *text);
extern struct memory command_memory[MOST_POOLED_COMMAND_MEMORY];
extern unsigned command_memory_count;
void give_back_command_memory(struct command_buffer *cb);
void autorelease(obj_handle_t h);
extern struct wait waits[MOST_WAITS];
void set_event_value(obj_handle_t event, uint64_t value);
bool submit(struct command_buffer *cb);
extern ULONG64 committed;
extern ULONG64 objects_destroyed;
unsigned wm_free_handle_count(void);
void wm_say_live_now_and_then(void);

// wm_helpers.c: DXMT's helper functions this build has, by name.
uint32_t wm_helper_function(const char *name);

#endif
