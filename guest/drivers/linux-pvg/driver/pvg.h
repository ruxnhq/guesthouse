/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Guest driver for Apple's paravirtualized graphics device.
 *
 * The device's protocol is not documented. What this driver knows of it was
 * observed from outside. The device's numbers are in ruxnpvg_proto.h, the
 * header this driver shares with the Windows driver. What is defined here is
 * this driver's own.
 */
#ifndef PVG_H
#define PVG_H

#include <linux/align.h>
#include <linux/bits.h>
#include <linux/hrtimer.h>
#include <linux/io.h>
#include <linux/ktime.h>
#include <linux/list.h>
#include <linux/mutex.h>
#include <linux/pci.h>
#include <linux/spinlock.h>
#include <linux/timer.h>
#include <linux/types.h>
#include <linux/wait.h>
#include <linux/workqueue.h>

#include <drm/drm_connector.h>
#include <drm/drm_crtc.h>
#include <drm/drm_device.h>
#include <drm/drm_encoder.h>
#include <drm/drm_gem.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_mm.h>
#include <drm/drm_plane.h>

#include "pvg_drm.h"
#include "ruxnpvg_proto.h"

/*
 * The device's page of 16 KiB, as an unsigned long for sizes: the header's
 * RUXNPVG_PAGE_BYTES is an unsigned int, and max() and size arithmetic here
 * want the wider type.
 */
#define PVG_PAGE_SIZE		(1UL << RUXNPVG_PAGE_SHIFT)
/* A device page as an order of the page allocator, and in the guest's pages. */
#define PVG_PAGE_ORDER		(RUXNPVG_PAGE_SHIFT - PAGE_SHIFT)
#define PVG_PAGE_PAGES		(1U << PVG_PAGE_ORDER)

/*
 * The first ring (RUXNPVG_RING_BYTES, its room RUXNPVG_RING_ROOM) is never
 * turned off and on while the driver runs: its room divides 2^32, so the
 * place goes on without a jump where the counters wrap. The longest payload
 * the debugfs instrument sends of its own:
 */
#define PVG_COMMAND_MAX_WORDS	256

/* The largest picture, in bytes: its device pages are named in one page. */
#define PVG_SURFACE_MAX_BYTES	((u64)RUXNPVG_SURFACE_MAX_PAGES << RUXNPVG_PAGE_SHIFT)

/*
 * A surface's rows are four bytes a pixel, rounded up to 16 bytes. Metal
 * takes a texture over an IOSurface only with rows a multiple of 16 bytes
 * apart: the helper refuses every command buffer that names one with other
 * rows ("Invalid row byte alignment"), and the display's present of such a
 * surface ends the host's process. IOSurface rounds rows to 128 on its own
 * on an M5 Mac and takes any multiple of 16 it is given. 16 and not more, so
 * that a width that is a multiple of 4 pixels keeps rows of width * 4, which
 * the Vulkan driver's bridge counts on when it takes in a dma-buf. Before
 * PVG_FEATURE_PITCH_ALIGN the rows were width * 4.
 */
#define PVG_SURFACE_PITCH_ALIGN	16u

static inline u32 pvg_surface_pitch(u32 width)
{
	return ALIGN(width * 4, PVG_SURFACE_PITCH_ALIGN);
}

struct dentry;
struct dma_buf;
struct drm_file;
struct drm_gem_object;
struct drm_mode_create_dumb;
struct drm_pending_vblank_event;
struct pvg_device;
struct pvg_display;
struct pvg_child;
struct pvg_mapper;
struct pvg_surface;
struct pvg_task;

struct pvg_ring {
	struct mutex lock;		/* one sender at a time */
	struct page *root_page;
	struct page *pages;
	__le32 *root;
	__le32 *mem;
	/*
	 * How far the guest has written, in bytes. It is a counter that runs
	 * on; the place in the ring is the counter modulo RUXNPVG_RING_ROOM.
	 */
	u32 written;
	u32 waited;			/* how far the device is known to have read */
	u32 last_stamp;
	u64 late;			/* stamps found with no interrupt for them */

	wait_queue_head_t wait;
	spinlock_t irq_lock;		/* the fields below */
	u32 irq_stamp;			/* the stamp cell as the handler last saw it */
	bool faulted;
};

struct pvg_irq_counts {
	u64 taken;
	u64 done;	/* RUXNPVG_REG_DONE read non-zero */
	u64 events;	/* RUXNPVG_REG_DISPLAY_EVENTS read non-zero */
	u64 faults;	/* RUXNPVG_REG_FAULT read non-zero */
	u64 empty;	/* none of the three */
};

/* How page flips came to their end, for debugfs. */
struct pvg_flip_counts {
	u64 by_irq;	/* the device's word, noticed by the interrupt handler */
	u64 by_look;	/* the same, noticed by the sender after the stamp */
	u64 timed_out;	/* no word from the device in time */
	u64 by_timer;	/* left to the vblank timer */
};

/* What the device was told of the cursor, for debugfs. */
struct pvg_cursor_counts {
	u64 glyphs;
	u64 shows;
	u64 moves;
};

/* How frames reached the device, for debugfs. */
struct pvg_frame_counts {
	u64 copied;	/* pixels copied into the path's own memory */
	u64 direct;	/* a buffer's own memory shown as it is */
	u64 undrawn;	/* of those, with a write to it not yet done */
};

struct pvg_mode {
	u16 width;
	u16 height;
	u16 refresh;	/* in Hz; 0 where the device names none */
};

#define PVG_MAX_MODES	32

/*
 * A way of getting a picture to the device. The DRM side either copies
 * pixels into memory the path hands out, or, where the path can do that,
 * has it show a buffer's own memory. It calls nothing else, so the paths
 * can take each other's place without a change there or in the ring.
 */
struct pvg_scanout_ops {
	const char *name;
	const char *key;	/* what the module parameter calls the path */
	/* The path shows a picture of any size, not only the sizes it lists. */
	bool any_size;
	/*
	 * The device says when it has shown a frame, and the path passes
	 * that on with pvg_kms_frame_kicked() and pvg_kms_frame_shown().
	 */
	bool reports_frames;
	/* The path shows a struct pvg_surface as it is, with no copy. */
	bool surfaces;
	/* The path has a display that takes a cursor, see pvg_display.c. */
	bool cursor;
	/* Fills in the modes the path can show. */
	int (*init)(struct pvg_device *pvg);
	void (*fini)(struct pvg_device *pvg);
	/* Turns the display to a mode. */
	int (*enable)(struct pvg_device *pvg, const struct pvg_mode *mode);
	void (*disable)(struct pvg_device *pvg);
	/*
	 * Sees to it that the scanout's pixels are there for the mode, where
	 * enable does not do that already. Called before pixels are copied.
	 */
	int (*pixels)(struct pvg_device *pvg);
	/*
	 * Has the device show a frame: the surface, or without one what was
	 * copied into the scanout's pixels.
	 */
	int (*flush)(struct pvg_device *pvg, struct pvg_surface *surface);
};

struct pvg_scanout {
	const struct pvg_scanout_ops *ops;
	void *backend;
	struct mutex modes_lock;	/* the two fields below */
	struct pvg_mode modes[PVG_MAX_MODES];
	unsigned int mode_count;
	/*
	 * Where copied pixels go, or NULL: the picture's first pixel, top
	 * row first, four bytes a pixel as blue, green, red, alpha.
	 */
	void *pixels;
	unsigned int pitch;
	/* The pixels are new memory that holds nothing of the picture yet. */
	bool pixels_fresh;
};

struct pvg_device {
	struct drm_device drm;
	struct drm_plane plane;
	struct drm_plane cursor;
	bool cursor_shown;
	struct drm_crtc crtc;
	struct drm_encoder encoder;
	struct drm_connector connector;
	struct pvg_scanout scanout;
	bool kms;
	/*
	 * The connector shown as gone for a moment after the host's preferred
	 * mode has changed, see pvg_kms_settled(). Under
	 * scanout.modes_lock: the preferred mode last told, whether the
	 * connector is gone and whether a probe has seen it gone. The work
	 * brings it back.
	 */
	struct pvg_mode preferred;
	bool gone;
	bool gone_seen;
	struct delayed_work back_work;
	/*
	 * The host's mode list as last taken, once it had stayed the same for
	 * a while, see pvg_kms_modes_changed(). Under scanout.modes_lock. The
	 * work takes the list when it has settled.
	 */
	struct pvg_mode settled[PVG_MAX_MODES];
	unsigned int settled_count;
	struct delayed_work settle_work;

	/*
	 * A page flip whose frame the device has not yet said it has shown.
	 * Under drm.event_lock.
	 */
	struct drm_pending_vblank_event *flip_event;
	bool flip_kicked;
	struct timer_list flip_timer;
	/* What this commit's frame is, between the plane's update and the flush. */
	bool frame_dirty;
	struct pvg_surface *frame_surface;
	struct pvg_flip_counts flips;
	struct pvg_frame_counts frames;
	struct pvg_cursor_counts cursors;

	struct pci_dev *pdev;
	void __iomem *regs;
	unsigned int vectors;		/* MSI vectors granted */
	struct pvg_ring ring;
	/* The mapper's device and the display, where the mapper's path runs. */
	struct pvg_mapper *mapper;
	struct pvg_display *display;
	bool display_live;		/* the handler may look at the display */
	/* The ids of the tasks that open files have, 1 to 255. */
	struct ida task_ids;
	/*
	 * Child rings, see pvg_child.c: their ids, the rings there are, where
	 * their senders sleep, how many looks ended a job, and the work item
	 * that looks at them.
	 */
	struct ida child_ids;
	spinlock_t children_lock;
	struct list_head children;
	wait_queue_head_t child_wait;
	atomic_t child_ends;
	atomic64_t child_late;		/* jobs ended by a look with no interrupt */
	struct delayed_work child_work;
	/*
	 * Version 3: fences that something waits on and that have not
	 * signalled, and the timer that looks at the rings every millisecond
	 * while there are such and the rings have work. See pvg_child.c.
	 */
	atomic_t fences_wanted;
	struct hrtimer signal_timer;
	atomic64_t signal_looks;
	atomic_t tasks;			/* how many there are */
	atomic64_t submits;
	atomic64_t submits_refused;
	atomic64_t queued;		/* of the submits, those by QUEUE */
	atomic64_t held_back;		/* of those, held back for fences going in */
	atomic64_t held_cancelled;	/* of those, never sent: their file ended */
	/* For tests (reload.diff): how often a sender found a ring with no room. */
	atomic64_t ring_full;		/* the first ring, -ENOSPC */
	atomic64_t child_full_bytes;	/* a child ring, by its bytes */
	atomic64_t child_full_jobs;	/* a child ring, by its 512 jobs */

	spinlock_t counts_lock;
	struct pvg_irq_counts counts;

	/*
	 * The driver took the screen from the firmware at probe (the module's
	 * firmware knob): PVG_FEATURE_FIRMWARE_SCREEN.
	 */
	bool firmware;

	/* Knobs of the debugfs instrument, see pvg_debugfs.c. */
	struct dentry *debugfs;
	u32 irq_skip;
	bool irq_log;
};

/* For tests (reload.diff), in pvg_task.c. */
extern unsigned int pvg_test_leave;
extern bool pvg_test_no_retry;

static inline struct pvg_device *to_pvg(struct drm_device *drm)
{
	return container_of(drm, struct pvg_device, drm);
}

static inline u32 pvg_read(struct pvg_device *pvg, u32 reg)
{
	return readl(pvg->regs + reg);
}

static inline void pvg_write(struct pvg_device *pvg, u32 reg, u32 value)
{
	writel(value, pvg->regs + reg);
}

/* A block of device pages: zeroed, contiguous, aligned to 16 KiB. */
struct page *pvg_alloc_pages(struct pvg_device *pvg, size_t bytes);
void pvg_free_pages(struct page *pages, size_t bytes);
u32 pvg_page_number(struct page *pages);

int pvg_ring_start(struct pvg_device *pvg);
void pvg_ring_stop(struct pvg_device *pvg);
void pvg_ring_free(struct pvg_device *pvg);
int pvg_ring_send(struct pvg_device *pvg, u32 number, const u32 *payload,
		  unsigned int words);
int pvg_ring_send_slow(struct pvg_device *pvg, u32 number, const u32 *payload,
		       unsigned int words, unsigned long timeout);
int pvg_ring_queue(struct pvg_device *pvg, u32 number, const u32 *payload,
		   unsigned int words, u32 *stamp);
void pvg_ring_kick(struct pvg_device *pvg);
int pvg_ring_wait(struct pvg_device *pvg, u32 stamp);
int pvg_ring_poll(struct pvg_device *pvg, unsigned int usecs);
/* How long a sleeping sender waits before it looks for its stamp itself. */
ktime_t pvg_recheck(void);
void pvg_ring_irq(struct pvg_device *pvg, bool fault);
void pvg_ring_log(struct pvg_device *pvg, unsigned int ring, u32 number, u32 stamp,
		  const u32 *payload, unsigned int words);

/*
 * A command in a child ring, see pvg_child.c. The sender owns the memory;
 * the ring holds it from pvg_child_queue() until result is set, and calls
 * done, if there is one, just before. result is -EINPROGRESS until then.
 */
struct pvg_job {
	struct list_head link;
	u32 stamp;
	u32 start;		/* the ring's written count where it starts */
	u32 length;
	bool refused;
	int result;
	void (*done)(struct pvg_job *job, int result);
};

static inline bool pvg_job_ended(struct pvg_job *job)
{
	return smp_load_acquire(&job->result) != -EINPROGRESS;
}

void pvg_children_init(struct pvg_device *pvg);
void pvg_children_fini(struct pvg_device *pvg);
struct pvg_child *pvg_child_create(struct pvg_device *pvg);
void pvg_child_destroy(struct pvg_device *pvg, struct pvg_child *child,
		       bool device_there, bool keep);
int pvg_child_send(struct pvg_device *pvg, struct pvg_child *child, u32 number,
		   const u32 *payload, unsigned int words, unsigned long timeout);
int pvg_child_queue(struct pvg_device *pvg, struct pvg_child *child, u32 number,
		    const u32 *payload, unsigned int words, struct pvg_job *job);
int pvg_child_wait_room(struct pvg_device *pvg, struct pvg_child *child,
			unsigned long deadline, bool interruptible);
bool pvg_child_retire(struct pvg_device *pvg, struct pvg_child *child, bool late);
bool pvg_children_look(struct pvg_device *pvg, bool late);
void pvg_signal_soon(struct pvg_device *pvg);
bool pvg_child_job_due(struct pvg_device *pvg, struct pvg_child *child,
		       struct pvg_job *job);
void pvg_child_barrier(struct pvg_child *child);
void pvg_child_abandon_jobs(struct pvg_child *child);
void pvg_child_counts(struct pvg_device *pvg, unsigned int *rings, u64 *queued,
		      u64 *refused);
void pvg_child_irq(struct pvg_device *pvg);

extern const struct drm_driver pvg_drm_driver;
extern const struct pvg_scanout_ops pvg_efb_ops;
extern const struct pvg_scanout_ops pvg_mapper_ops;
int pvg_kms_init(struct pvg_device *pvg, const char *path);
void pvg_kms_exit(struct pvg_device *pvg);
void pvg_kms_frame_kicked(struct pvg_device *pvg);
void pvg_kms_frame_shown(struct pvg_device *pvg, bool from_irq);
void pvg_kms_modes_changed(struct pvg_device *pvg);

/*
 * A picture in guest memory that the mapper's device can give a mapping id:
 * device pages that need not lie together.
 */
struct pvg_surface {
	u32 width;
	u32 height;
	u32 pitch;
	size_t bytes;		/* a whole number of device pages */
	unsigned int page_count;	/* of the guest's pages */
	struct page **pages;
	void *vaddr;		/* all of it in one piece, for the kernel */
	struct page *list_page;	/* names the device pages for the device */
	u32 id;			/* the mapping id, 0 while not mapped */
	struct list_head link;	/* among the mapper's surfaces while mapped */
};

int pvg_mapper_init(struct pvg_device *pvg);
void pvg_mapper_fini(struct pvg_device *pvg);
struct pvg_surface *pvg_surface_create(struct pvg_device *pvg, u32 width,
				       u32 height);
void pvg_surface_destroy(struct pvg_device *pvg, struct pvg_surface *surface);
int pvg_surface_map(struct pvg_device *pvg, struct pvg_surface *surface);
int pvg_surface_unmap(struct pvg_device *pvg, struct pvg_surface *surface);
void pvg_surface_reap(struct pvg_device *pvg);

/* Buffers whose memory is a surface. */
int pvg_gem_dumb_create(struct drm_file *file, struct drm_device *drm,
			struct drm_mode_create_dumb *args);
int pvg_gem_surface_create(struct drm_file *file, struct drm_device *drm,
			   u32 width, u32 height, u32 *handle, u32 *pitch,
			   u64 *size, u64 *offset);
struct pvg_surface *pvg_gem_surface(struct drm_gem_object *obj);

/*
 * A buffer object: memory in device pages that a program maps and that lies
 * in the task of each file that has a handle to it. See pvg_bo.c.
 */
struct pvg_bo {
	struct drm_gem_object base;
	unsigned int page_count;	/* of the guest's pages */
	struct page **pages;
	struct mutex lock;		/* the list below */
	struct list_head placements;
};

/*
 * Where a buffer object lies in one task. The file's handle holds it, and so
 * does every object of the task that names the memory. Under the task's lock.
 */
struct pvg_placement {
	struct pvg_bo *bo;
	struct pvg_task *task;
	struct drm_mm_node node;	/* in device pages */
	struct list_head link;		/* among the buffer object's placements */
	struct list_head stale;		/* among the task's that could not be unmapped */
	unsigned int refs;
	bool handle;			/* the file's handle is one of them */
};

struct pvg_bo *to_pvg_bo(struct drm_gem_object *obj);
int pvg_bo_create_ioctl(struct drm_device *drm, void *data, struct drm_file *file);
int pvg_bo_info_ioctl(struct drm_device *drm, void *data, struct drm_file *file);
struct drm_gem_object *pvg_prime_import(struct drm_device *drm,
					struct dma_buf *dma_buf);

/* What an open file has: its task, once it has needed one. See pvg_task.c. */
struct pvg_file {
	struct mutex lock;		/* the task's making */
	struct pvg_task *task;
};

#define PVG_IOCTL_COUNT	(DRM_PVG_BO_INFO + 1)
extern const struct drm_ioctl_desc pvg_ioctls[PVG_IOCTL_COUNT];
int pvg_file_open(struct drm_device *drm, struct drm_file *file);
void pvg_file_release(struct drm_file *file);
void pvg_file_postclose(struct drm_device *drm, struct drm_file *file);
struct pvg_task *pvg_file_task(struct drm_file *file);
int pvg_task_place(struct pvg_task *task, struct pvg_bo *bo);
int pvg_task_address(struct pvg_task *task, struct pvg_bo *bo, u64 *address);
void pvg_task_unplace(struct pvg_task *task, struct pvg_bo *bo);

void __iomem *pvg_mapper_regs(struct pvg_device *pvg);
void pvg_mapper_counts(struct pvg_device *pvg, unsigned int *mapped,
		       unsigned int *held);
__le32 *pvg_display_state(struct pvg_device *pvg);
int pvg_display_init(struct pvg_device *pvg);
void pvg_display_fini(struct pvg_device *pvg);
int pvg_display_swap(struct pvg_device *pvg, u32 mapping);
int pvg_display_cursor_glyph(struct pvg_device *pvg, const void *pixels,
			     unsigned int pitch, u32 width, u32 height,
			     u32 hot_x, u32 hot_y);
int pvg_display_cursor_show(struct pvg_device *pvg, bool show);
void pvg_display_cursor_move(struct pvg_device *pvg, u32 x, u32 y);
void pvg_display_irq(struct pvg_device *pvg);

void pvg_debugfs_init(struct pvg_device *pvg);
void pvg_debugfs_exit(struct pvg_device *pvg);
void pvg_debugfs_register(void);
void pvg_debugfs_unregister(void);

#endif
