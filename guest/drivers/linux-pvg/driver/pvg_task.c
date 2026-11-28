// SPDX-License-Identifier: GPL-2.0
/*
 * A task for each open file, and what a program does with it.
 *
 * A task is an address space on the host: a root page, a page table of the
 * guest's memory, and a list of objects that the host's Metal makes for it.
 * Commands that carry work name a task, and through it the memory their
 * command buffers lie in and the objects those use.
 *
 * The driver owns a task's layout. From address 0 lie the object list, the
 * placement list, which the host writes, and the objects' descriptions.
 * Buffer objects lie behind them, wherever there is room. A program never
 * writes the lists: it hands a description's tail to the driver, which puts
 * before it the part that names memory or a surface, so that no program can
 * name what is not its own.
 *
 * All of this is observed. The interface is ../uapi/pvg_drm.h
 */

#include <linux/bitmap.h>
#include <linux/dma-fence.h>
#include <linux/dma-fence-chain.h>
#include <linux/dma-fence-unwrap.h>
#include <linux/dma-resv.h>
#include <linux/file.h>
#include <linux/module.h>
#include <linux/overflow.h>
#include <linux/slab.h>
#include <linux/sync_file.h>
#include <linux/uaccess.h>
#include <linux/unaligned.h>

#include <drm/drm_drv.h>
#include <drm/drm_file.h>
#include <drm/drm_gem.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_syncobj.h>
#include <drm/drm_utils.h>

#include "pvg.h"

/*
 * For asking the device: 55 has a submit go out as the older command for
 * command buffers, whose resources are 24 bytes each, in place of 67.
 */
static unsigned int exec = RUXNPVG_CMD_EXEC;
module_param(exec, uint, 0644);
MODULE_PARM_DESC(exec, "The command a submit is sent as: 67 (default) or 55");

static bool child_rings = true;
module_param(child_rings, bool, 0644);
MODULE_PARM_DESC(child_rings,
		 "Give each task a command ring of its own, where one is to be had, in place of the first ring (default on)");

/*
 * For tests of a driver loaded again (reload.diff). With test_leave a task
 * whose file ends is not deleted on the device: its pages and its id stay,
 * as for a task the device would not let go, and the display's kernel task
 * stays defined when the driver is unloaded. With test_leave=2 the task's
 * id is free again at once, so that the next task of a file is defined
 * under an id the device still knows. With test_no_retry a refused
 * command 56 is returned as it is, with no delete and no second try.
 */
unsigned int pvg_test_leave;
module_param_named(test_leave, pvg_test_leave, uint, 0644);
MODULE_PARM_DESC(test_leave, "For tests: leave tasks defined on the device when they end; 2 also gives their ids to the next tasks");
bool pvg_test_no_retry;
module_param_named(test_no_retry, pvg_test_no_retry, bool, 0644);
MODULE_PARM_DESC(test_no_retry, "For tests: do not delete and define again a task the device refuses to define");

/*
 * How long a command that hands the GPU work is waited for: its stamp comes
 * when the work is done.
 */
#define PVG_TASK_TIMEOUT	msecs_to_jiffies(RUXNPVG_TASK_TIMEOUT_MS)

/*
 * A task is PVG_TASK_BYTES, 4 GiB, with a table of two levels, its layout
 * otherwise that of ruxnpvg_proto.h. That header has 1 GiB,
 * RUXNPVG_TASK_BYTES; the length is the drivers' choice, and Guesthouse's
 * guard takes a task of up to 4 GiB. A page of the table holds 4096 entries, so the top page
 * names tables of 64 MiB each, 64 of them.
 */
#define PVG_TASK_BYTES		(4ull << 30)
#define PVG_TASK_PAGES		(PVG_TASK_BYTES >> RUXNPVG_PAGE_SHIFT)
#define PVG_TASK_TABLES		(PVG_TASK_PAGES >> RUXNPVG_TABLE_SHIFT)

/*
 * For tests: test_task_room=MIB gives a file's buffer objects only that
 * many MiB of the task, from page RUXNPVG_TASK_FIRST_BO_PAGE on, and
 * PVG_PARAM_TASK_BYTES says so. A guest with less memory than a task can
 * then ask for more than its task has room for. The task's length on the
 * device stays PVG_TASK_BYTES. 0, the default, is all of it.
 */
static unsigned int pvg_test_task_room;
module_param_named(test_task_room, pvg_test_task_room, uint, 0444);
MODULE_PARM_DESC(test_task_room, "For tests: the MiB of a task that buffer objects may take (0: all)");

/* The task's pages that buffer objects may take. */
static u64 pvg_task_room_pages(void)
{
	u64 room = PVG_TASK_PAGES - RUXNPVG_TASK_FIRST_BO_PAGE;
	u64 asked = (u64)pvg_test_task_room << (20 - RUXNPVG_PAGE_SHIFT);

	return asked && asked < room ? asked : room;
}

/*
 * The object list's and the placement list's room, for RUXNPVG_MAX_OBJECTS:
 * the device takes the pair only if both have room for the same count. An
 * object's entry is three 32-bit words, a placement's two of 64 bits.
 */
#define PVG_OBJECT_WORDS	(RUXNPVG_OBJECT_ENTRY_BYTES / 4)
#define PVG_PLACEMENT_WORDS	(RUXNPVG_PLACEMENT_ENTRY_BYTES / 8)
#define PVG_OBJECTS_BYTES	(RUXNPVG_MAX_OBJECTS * RUXNPVG_OBJECT_ENTRY_BYTES)
#define PVG_PLACEMENTS_BYTES	(RUXNPVG_MAX_OBJECTS * RUXNPVG_PLACEMENT_ENTRY_BYTES)

/* Descriptions are given room in pieces of RUXNPVG_DESCRIPTION_SLOT bytes. */
#define PVG_DESCRIPTION_SLOTS	\
	(RUXNPVG_TASK_DESCRIPTION_PAGES * PVG_PAGE_SIZE / RUXNPVG_DESCRIPTION_SLOT)

/*
 * The most queued submits of a file in flight, and for how many of the last
 * numbers a refusal is remembered.
 */
#define PVG_MAX_QUEUED		256
#define PVG_TASK_WINDOW		4096

/*
 * Version 3: the most sync objects one QUEUE waits for and signals, each,
 * and the most fences a submit waits for once sync_files and timelines are
 * taken apart.
 */
#define PVG_MAX_SYNCS		256
#define PVG_MAX_INS		1024

/*
 * Operation 9 of the serializer, a texture over a buffer: 64 bytes, the
 * buffer's object number at byte 12, the offset in it at 16 and a row's
 * length at 24, 64 bits each, the texture's descriptor from 32. Not in
 * ruxnpvg_proto.h, so it is the driver's own name.
 */
#define PVG_OP_TEXTURE_BUFFER		9
#define PVG_OP_TEXTURE_BUFFER_BYTES	64u
#define PVG_OP_TEXTURE_BUFFER_OFFSET	16
#define PVG_OP_TEXTURE_BUFFER_ROW	24
/* The descriptor's words, from its start at byte 32. */
#define PVG_TEXTURE_DESC		32
#define PVG_TEXTURE_DESC_FORMAT		(PVG_TEXTURE_DESC + 2)
#define PVG_TEXTURE_DESC_WIDTH		(PVG_TEXTURE_DESC + 4)
#define PVG_TEXTURE_DESC_HEIGHT		(PVG_TEXTURE_DESC + 8)
#define PVG_TEXTURE_DESC_DEPTH		(PVG_TEXTURE_DESC + 12)
#define PVG_TEXTURE_DESC_LEVELS		(PVG_TEXTURE_DESC + 16)
#define PVG_TEXTURE_DESC_SAMPLES	(PVG_TEXTURE_DESC + 18)
#define PVG_TEXTURE_DESC_ARRAY		(PVG_TEXTURE_DESC + 20)
/* Metal's MTLTextureType, and its alignment for textures over buffers here. */
#define PVG_TEXTURE_TYPE_2D		2
#define PVG_TEXTURE_TYPE_BUFFER		9
#define PVG_TEXTURE_LINEAR_ALIGN	16
#define PVG_TEXTURE_MAX_SIDE		16384


struct pvg_object {
	u8 kind;			/* 0 while the number is free */
	bool retired;			/* destroyed by its program, kept by the host */
	u16 delete_op;			/* what deletes it, if it is no resource */
	u16 slots;			/* the description's room */
	u32 slot;
	struct pvg_placement *placement;	/* the memory it names, if it names some */
	struct drm_gem_object *surface;	/* the surface it names, if it names one */
	u32 base;			/* a view's texture, or the buffer under it */
	u32 views;			/* a texture's views, a buffer's textures */
	u64 size;			/* a buffer's bytes */
};

struct pvg_submit;

/* Version 3: a fence a held-back submit waits for, with its callback. */
struct pvg_in {
	struct dma_fence_cb cb;
	struct dma_fence *fence;
	struct pvg_submit *submit;
};

/*
 * A queued submit. Its fence is its life: the fence's last reference frees
 * it, which is why the fence comes first. The task's lists hold one
 * reference until the submit is reaped, and with it the holds on the places
 * of the command buffers' memory.
 *
 * Since version 3 a submit can be held back: it has its number and its
 * fence, and keeps its command's words until every fence going in has
 * signalled and the submits before it have gone into the ring.
 */
struct pvg_submit {
	struct dma_fence fence;
	spinlock_t lock;		/* the fence's */
	struct pvg_job job;
	struct list_head link;		/* among the task's queued or held back, oldest first */
	struct pvg_device *pvg;
	struct pvg_task *task;
	u64 stamp;
	bool sent;			/* in the ring: the job is the ring's */
	unsigned int held_count;
	struct pvg_placement *held[RUXNPVG_MAX_BUFFERS];

	/* Held back: the command, and the fences going in. */
	u32 number;
	u32 *payload;
	unsigned int words;
	struct pvg_in *ins;
	unsigned int in_count;
	unsigned int in_room;
	/* Fences going in not yet signalled, and one while QUEUE sets them up. */
	atomic_t pending;
};

struct pvg_task {
	struct pvg_device *pvg;
	struct mutex lock;
	u32 id;
	bool defined;			/* the device knows the task */
	bool lost;			/* the device would not let it go: its pages stay */
	bool given_up;			/* work in flight did not end: nothing is sent */
	struct pvg_child *child;	/* its own ring, or NULL for the first ring */

	/* Version 2's submits: in flight or not yet reaped, oldest first. */
	struct list_head queued;
	unsigned int queued_count;
	/*
	 * Version 3's held-back submits, oldest first; all their numbers are
	 * above the queued ones'. The work sends what is due.
	 */
	struct list_head held_back;
	unsigned int held_back_count;
	struct work_struct send_work;
	u64 last_stamp;			/* the last number handed out */
	u64 reaped;			/* every number up to this one is reaped */
	u64 fence_context;
	DECLARE_BITMAP(refused, PVG_TASK_WINDOW);	/* by number, of the reaped */

	struct page *root_page;
	struct page *top_page;
	struct page *tables[PVG_TASK_TABLES];
	struct page *objects_page;
	struct page *placements_page;
	struct page *description_pages[RUXNPVG_TASK_DESCRIPTION_PAGES];
	DECLARE_BITMAP(description_slots, PVG_DESCRIPTION_SLOTS);

	struct drm_mm mm;		/* buffer objects' places, in device pages */
	struct list_head stale;		/* places the device may still have mapped */
	struct pvg_object objects[RUXNPVG_MAX_OBJECTS];
};

/*
 * Sends a command that names the task: through the task's own ring if it has
 * one, else through the first.
 */
static int pvg_task_send(struct pvg_task *task, u32 number, const u32 *payload,
			 unsigned int words)
{
	if (task->child)
		return pvg_child_send(task->pvg, task->child, number, payload, words,
				      PVG_TASK_TIMEOUT);
	return pvg_ring_send_slow(task->pvg, number, payload, words, PVG_TASK_TIMEOUT);
}

/* Where the table's entry for a page of the task is. */
static __le32 *pvg_task_entry(struct pvg_task *task, u32 page)
{
	unsigned int index = page >> RUXNPVG_TABLE_SHIFT;

	if (!task->tables[index]) {
		struct page *table = pvg_alloc_pages(task->pvg, PVG_PAGE_SIZE);
		__le32 *top = page_address(task->top_page);

		if (!table)
			return NULL;
		task->tables[index] = table;
		WRITE_ONCE(top[index], cpu_to_le32(pvg_page_number(table)));
	}
	return (__le32 *)page_address(task->tables[index]) +
	       (page & (RUXNPVG_TABLE_ENTRIES - 1));
}

static int pvg_task_enter(struct pvg_task *task, u32 page, struct page *memory,
			  bool writable)
{
	__le32 *entry = pvg_task_entry(task, page);

	if (!entry)
		return -ENOMEM;
	WRITE_ONCE(*entry, cpu_to_le32(pvg_page_number(memory) |
				       (writable ? RUXNPVG_ENTRY_WRITABLE : 0)));
	return 0;
}

static void pvg_task_write_root(struct pvg_task *task)
{
	__le32 *root = page_address(task->root_page);
	u64 objects = (u64)RUXNPVG_TASK_OBJECTS_PAGE << RUXNPVG_PAGE_SHIFT;
	u64 placements = (u64)RUXNPVG_TASK_PLACEMENTS_PAGE << RUXNPVG_PAGE_SHIFT;

	memset(root, 0, PVG_PAGE_SIZE);
	root[RUXNPVG_TASK_ROOT_TABLE] = cpu_to_le32(pvg_page_number(task->top_page));
	root[RUXNPVG_TASK_ROOT_DEPTH] = cpu_to_le32(RUXNPVG_TASK_DEPTH);
	root[RUXNPVG_TASK_ROOT_OBJECTS] = cpu_to_le32(lower_32_bits(objects));
	root[RUXNPVG_TASK_ROOT_OBJECTS + 1] = cpu_to_le32(upper_32_bits(objects));
	root[RUXNPVG_TASK_ROOT_OBJECTS_BYTES] = cpu_to_le32(PVG_OBJECTS_BYTES);
	root[RUXNPVG_TASK_ROOT_PLACEMENTS] = cpu_to_le32(lower_32_bits(placements));
	root[RUXNPVG_TASK_ROOT_PLACEMENTS + 1] = cpu_to_le32(upper_32_bits(placements));
	root[RUXNPVG_TASK_ROOT_PLACEMENTS_BYTES] = cpu_to_le32(PVG_PLACEMENTS_BYTES);
}

/*
 * Tells the device of the task. A task of this id that a driver before this
 * one left behind is deleted first.
 */
static int pvg_task_define(struct pvg_task *task)
{
	struct pvg_device *pvg = task->pvg;
	__le32 *root = page_address(task->root_page);
	u32 payload[4] = {
		task->id << 1,
		lower_32_bits(PVG_TASK_BYTES), upper_32_bits(PVG_TASK_BYTES),
		pvg_page_number(task->root_page),
	};
	int ret;

	pvg_task_write_root(task);
	ret = pvg_ring_send(pvg, RUXNPVG_CMD_TASK_DEFINE, payload, ARRAY_SIZE(payload));
	if (ret == -EIO && READ_ONCE(pvg_test_no_retry))
		dev_info(&pvg->pdev->dev, "task %u: command 56 refused, not tried again (test_no_retry)\n",
			 task->id);
	else if (ret == -EIO) {
		dev_info(&pvg->pdev->dev, "task %u is there already, defining it anew\n",
			 task->id);
		ret = pvg_ring_send(pvg, RUXNPVG_CMD_TASK_DELETE, &task->id, 1);
		if (ret)
			return ret;
		pvg_task_write_root(task);
		ret = pvg_ring_send(pvg, RUXNPVG_CMD_TASK_DEFINE, payload,
				    ARRAY_SIZE(payload));
	}
	if (ret)
		return ret;
	task->defined = true;
	atomic_inc(&pvg->tasks);
	if (READ_ONCE(child_rings))
		task->child = pvg_child_create(pvg);

	/* The device zeroes the lists' words if it does not take the lists. */
	if (!root[RUXNPVG_TASK_ROOT_OBJECTS_BYTES] || !root[RUXNPVG_TASK_ROOT_PLACEMENTS_BYTES]) {
		dev_err(&pvg->pdev->dev, "task %u: the device did not take the lists\n",
			task->id);
		return -ENODEV;
	}
	return 0;
}

/* Has the device forget the task. Its memory is the guest's again after. */
static void pvg_task_delete(struct pvg_task *task)
{
	struct pvg_device *pvg = task->pvg;
	int idx, ret;

	lockdep_assert_held(&task->lock);
	if (!task->defined)
		return;
	task->defined = false;
	atomic_dec(&pvg->tasks);
	/* Work that did not end may still read and write all of it. */
	if (task->given_up) {
		if (task->child)
			pvg_child_destroy(pvg, task->child, false, true);
		task->child = NULL;
		task->lost = true;
		return;
	}
	/* A device that is gone reads nothing. */
	if (!drm_dev_enter(&pvg->drm, &idx)) {
		if (task->child)
			pvg_child_destroy(pvg, task->child, false, false);
		task->child = NULL;
		return;
	}
	if (READ_ONCE(pvg_test_leave)) {
		/*
		 * The ring goes, so that no descriptor of it is left in a root
		 * page the next driver does not have; the task stays.
		 */
		if (task->child)
			pvg_child_destroy(pvg, task->child, true, false);
		task->child = NULL;
		drm_dev_exit(idx);
		dev_info(&pvg->pdev->dev, "task %u left defined on the device (test_leave=%u)\n",
			 task->id, READ_ONCE(pvg_test_leave));
		task->lost = true;
		if (READ_ONCE(pvg_test_leave) == 2)
			ida_free(&pvg->task_ids, task->id);
		return;
	}
	ret = pvg_ring_send(pvg, RUXNPVG_CMD_TASK_DELETE, &task->id, 1);
	/* The task's ring goes after the task, whose commands it carried. */
	if (task->child)
		pvg_child_destroy(pvg, task->child, true, false);
	task->child = NULL;
	drm_dev_exit(idx);
	if (ret) {
		dev_err(&pvg->pdev->dev,
			"task %u not deleted: %d. Its memory is left to the device\n",
			task->id, ret);
		task->lost = true;
	}
}

static void pvg_task_free(struct pvg_task *task)
{
	struct pvg_placement *placement, *next;
	unsigned int i;

	/*
	 * A task the device would not let go may still be read and written
	 * by it: its pages, its id and what its objects name all stay.
	 */
	if (task->lost)
		return;

	list_for_each_entry_safe(placement, next, &task->stale, stale) {
		drm_mm_remove_node(&placement->node);
		drm_gem_object_put(&placement->bo->base);
		kfree(placement);
	}
	drm_mm_takedown(&task->mm);
	for (i = 0; i < RUXNPVG_TASK_DESCRIPTION_PAGES; i++)
		pvg_free_pages(task->description_pages[i], PVG_PAGE_SIZE);
	for (i = 0; i < PVG_TASK_TABLES; i++)
		pvg_free_pages(task->tables[i], PVG_PAGE_SIZE);
	pvg_free_pages(task->placements_page, PVG_PAGE_SIZE);
	pvg_free_pages(task->objects_page, PVG_PAGE_SIZE);
	pvg_free_pages(task->top_page, PVG_PAGE_SIZE);
	pvg_free_pages(task->root_page, PVG_PAGE_SIZE);
	if (task->id)
		ida_free(&task->pvg->task_ids, task->id);
	mutex_destroy(&task->lock);
	kvfree(task);
}

static void pvg_task_send_work(struct work_struct *work);

static struct pvg_task *pvg_task_create(struct pvg_device *pvg)
{
	struct pvg_task *task;
	int ret;

	BUILD_BUG_ON(PVG_OBJECTS_BYTES > PVG_PAGE_SIZE);
	BUILD_BUG_ON(PVG_PLACEMENTS_BYTES > PVG_PAGE_SIZE);
	BUILD_BUG_ON(PVG_TASK_TABLES > RUXNPVG_TABLE_ENTRIES);

	task = kvzalloc(sizeof(*task), GFP_KERNEL);
	if (!task)
		return ERR_PTR(-ENOMEM);
	task->pvg = pvg;
	mutex_init(&task->lock);
	INIT_LIST_HEAD(&task->stale);
	INIT_LIST_HEAD(&task->queued);
	INIT_LIST_HEAD(&task->held_back);
	INIT_WORK(&task->send_work, pvg_task_send_work);
	task->fence_context = dma_fence_context_alloc(1);
	drm_mm_init(&task->mm, RUXNPVG_TASK_FIRST_BO_PAGE, pvg_task_room_pages());

	task->root_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	task->top_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	task->objects_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	task->placements_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	if (!task->root_page || !task->top_page || !task->objects_page ||
	    !task->placements_page) {
		ret = -ENOMEM;
		goto err;
	}
	/* The host reads the object list and writes the placement list. */
	ret = pvg_task_enter(task, RUXNPVG_TASK_OBJECTS_PAGE, task->objects_page, false);
	if (!ret)
		ret = pvg_task_enter(task, RUXNPVG_TASK_PLACEMENTS_PAGE,
				     task->placements_page, true);
	if (ret)
		goto err;

	ret = ida_alloc_range(&pvg->task_ids, RUXNPVG_TASK_FIRST_ID, RUXNPVG_TASK_LAST_ID,
			      GFP_KERNEL);
	if (ret < 0)
		goto err;
	task->id = ret;

	mutex_lock(&task->lock);
	ret = pvg_task_define(task);
	if (ret)
		pvg_task_delete(task);
	mutex_unlock(&task->lock);
	if (ret)
		goto err;
	return task;

err:
	pvg_task_free(task);
	return ERR_PTR(ret == -EIO ? -ENODEV : ret);
}

int pvg_file_open(struct drm_device *drm, struct drm_file *file)
{
	struct pvg_file *priv;

	priv = kzalloc(sizeof(*priv), GFP_KERNEL);
	if (!priv)
		return -ENOMEM;
	mutex_init(&priv->lock);
	file->driver_priv = priv;
	return 0;
}

/* The file's task. It is made when the file first needs it. */
struct pvg_task *pvg_file_task(struct drm_file *file)
{
	struct pvg_device *pvg = to_pvg(file->minor->dev);
	struct pvg_file *priv = file->driver_priv;
	struct pvg_task *task;
	int idx;

	mutex_lock(&priv->lock);
	task = priv->task;
	if (!task) {
		if (!drm_dev_enter(&pvg->drm, &idx)) {
			mutex_unlock(&priv->lock);
			return ERR_PTR(-ENODEV);
		}
		task = pvg_task_create(pvg);
		drm_dev_exit(idx);
		if (!IS_ERR(task)) {
			priv->task = task;
			dev_dbg(&pvg->pdev->dev, "task %u for %s\n", task->id,
				current->comm);
		}
	}
	mutex_unlock(&priv->lock);
	return task;
}

/* A place's entries in the table, with the memory or without. */
static int pvg_placement_enter(struct pvg_placement *placement, bool there)
{
	struct pvg_task *task = placement->task;
	u32 i;

	for (i = 0; i < placement->node.size; i++) {
		__le32 *entry = pvg_task_entry(task, placement->node.start + i);

		if (!entry)
			return -ENOMEM;
		WRITE_ONCE(*entry, there ?
			   cpu_to_le32(pvg_page_number(placement->bo->pages[i * PVG_PAGE_PAGES]) |
				       RUXNPVG_ENTRY_WRITABLE) : 0);
	}
	return 0;
}

static struct pvg_placement *pvg_placement_find(struct pvg_task *task,
						struct pvg_bo *bo)
{
	struct pvg_placement *placement, *found = NULL;

	lockdep_assert_held(&task->lock);
	mutex_lock(&bo->lock);
	list_for_each_entry(placement, &bo->placements, link) {
		if (placement->task == task) {
			found = placement;
			break;
		}
	}
	mutex_unlock(&bo->lock);
	return found;
}

/*
 * Lets go of a place. With the last hold gone the memory leaves the task:
 * its entries are cleared, and the device is told to drop what it has mapped
 * of it. If the device does not say it did, the place and its pages are kept.
 */
static void pvg_placement_put(struct pvg_placement *placement)
{
	struct pvg_task *task = placement->task;
	struct pvg_device *pvg = task->pvg;
	struct pvg_bo *bo = placement->bo;
	u64 address = (u64)placement->node.start << RUXNPVG_PAGE_SHIFT;
	u64 bytes = (u64)placement->node.size << RUXNPVG_PAGE_SHIFT;
	u32 payload[5] = {
		task->id,
		lower_32_bits(address), upper_32_bits(address),
		lower_32_bits(bytes), upper_32_bits(bytes),
	};
	int idx, ret = 0;

	lockdep_assert_held(&task->lock);
	if (--placement->refs)
		return;

	pvg_placement_enter(placement, false);
	if (task->defined && drm_dev_enter(&pvg->drm, &idx)) {
		ret = pvg_task_send(task, RUXNPVG_CMD_TASK_UNMAP, payload,
				    ARRAY_SIZE(payload));
		drm_dev_exit(idx);
	}
	mutex_lock(&bo->lock);
	list_del(&placement->link);
	mutex_unlock(&bo->lock);
	if (ret) {
		dev_warn_ratelimited(&pvg->pdev->dev, "task %u: %#llx bytes at %#llx not unmapped: %d\n",
				     task->id, bytes, address, ret);
		list_add(&placement->stale, &task->stale);
		return;
	}
	drm_mm_remove_node(&placement->node);
	kfree(placement);
	drm_gem_object_put(&bo->base);
}

static struct pvg_object *pvg_task_object(struct pvg_task *task, u32 id);

static const char *pvg_fence_driver_name(struct dma_fence *fence)
{
	return "pvg";
}

static const char *pvg_fence_timeline_name(struct dma_fence *fence)
{
	return "pvg-task";
}

/*
 * Something waits on the fence: a page flip, a poll() on a sync_file, a
 * fence going into another submit. Version 3 has the rings looked at every
 * millisecond until it signals. Called under the fence's lock, with
 * interrupts off; the fence has not signalled, so its task is there.
 */
static bool pvg_fence_enable_signaling(struct dma_fence *fence)
{
	struct pvg_submit *submit = container_of(fence, struct pvg_submit, fence);

	atomic_inc(&submit->pvg->fences_wanted);
	pvg_signal_soon(submit->pvg);
	return true;
}

/* The fence of a queued submit. Its release is the default, which frees it. */
static const struct dma_fence_ops pvg_fence_ops = {
	.get_driver_name = pvg_fence_driver_name,
	.get_timeline_name = pvg_fence_timeline_name,
	.enable_signaling = pvg_fence_enable_signaling,
};

/*
 * Signals the submit's fence, with an error if result is one. Every fence
 * of a submit is signalled here, so that a fence something waited on is
 * counted out of the millisecond look once.
 */
static void pvg_submit_signal(struct pvg_submit *submit, int result)
{
	struct dma_fence *fence = &submit->fence;
	unsigned long flags;
	bool wanted;

	spin_lock_irqsave(&submit->lock, flags);
	if (test_bit(DMA_FENCE_FLAG_SIGNALED_BIT, &fence->flags)) {
		spin_unlock_irqrestore(&submit->lock, flags);
		return;
	}
	if (result)
		dma_fence_set_error(fence, result);
	wanted = test_bit(DMA_FENCE_FLAG_ENABLE_SIGNAL_BIT, &fence->flags);
	dma_fence_signal_locked(fence);
	spin_unlock_irqrestore(&submit->lock, flags);
	if (wanted)
		atomic_dec(&submit->pvg->fences_wanted);
}

/* Called by the ring, under its lock, when the submit's stamp has come. */
static void pvg_submit_done(struct pvg_job *job, int result)
{
	struct pvg_submit *submit = container_of(job, struct pvg_submit, job);

	pvg_submit_signal(submit, result);
}

/*
 * Ends a submit that never went into a ring: its fence with the error, and
 * its job as ended, so that it is reaped in its order.
 */
static void pvg_submit_end_unsent(struct pvg_submit *submit, int result)
{
	pvg_submit_signal(submit, result);
	smp_store_release(&submit->job.result, result);
}

/*
 * A fence going in has signalled. This may run where the device's look
 * signals a fence, under a ring's lock with interrupts off, so it only
 * schedules the task's work. The submit stays until its fences' callbacks
 * are taken off, see pvg_submit_drop_ins(), so in is there while this runs.
 */
static void pvg_in_signalled(struct dma_fence *fence, struct dma_fence_cb *cb)
{
	struct pvg_in *in = container_of(cb, struct pvg_in, cb);
	struct pvg_submit *submit = in->submit;

	if (atomic_dec_and_test(&submit->pending))
		queue_work(system_unbound_wq, &submit->task->send_work);
}

/*
 * Lets go of the fences going in. Taking a callback off takes the fence's
 * lock, under which its callbacks run, so after this none of them runs.
 */
static void pvg_submit_drop_ins(struct pvg_submit *submit)
{
	unsigned int i;

	for (i = 0; i < submit->in_count; i++) {
		dma_fence_remove_callback(submit->ins[i].fence, &submit->ins[i].cb);
		dma_fence_put(submit->ins[i].fence);
	}
	kfree(submit->ins);
	submit->ins = NULL;
	submit->in_count = 0;
	submit->in_room = 0;
}

/*
 * Lets go of the queued submits that have ended, oldest first: their holds
 * on memory, and the list's reference on their fences. A refusal is kept in
 * the window by number. A task given up keeps what its submits held.
 */
static void pvg_task_reap(struct pvg_task *task)
{
	struct pvg_submit *submit, *next;
	unsigned int i;

	lockdep_assert_held(&task->lock);
	list_for_each_entry_safe(submit, next, &task->queued, link) {
		if (!pvg_job_ended(&submit->job))
			break;
		if (submit->job.result) {
			__set_bit(submit->stamp % PVG_TASK_WINDOW, task->refused);
			/* One that never went out was refused by no device. */
			if (submit->sent)
				atomic64_inc(&task->pvg->submits_refused);
		}
		else
			__clear_bit(submit->stamp % PVG_TASK_WINDOW, task->refused);
		task->reaped = submit->stamp;
		if (!task->given_up) {
			for (i = 0; i < submit->held_count; i++)
				pvg_placement_put(submit->held[i]);
		}
		list_del(&submit->link);
		task->queued_count--;
		dma_fence_put(&submit->fence);
	}
}

/*
 * Version 3: writes into the ring every held-back submit that is due, oldest
 * first, and stops at the first that still waits for a fence. One that can't
 * be written ends with the error in its fence, and the next goes on. Under
 * the task's lock: from the work, and from QUEUE.
 */
static void pvg_task_send_held(struct pvg_task *task)
{
	struct pvg_device *pvg = task->pvg;
	struct pvg_submit *submit;
	bool moved = false;

	lockdep_assert_held(&task->lock);
	while ((submit = list_first_entry_or_null(&task->held_back,
						  struct pvg_submit, link))) {
		unsigned long deadline = jiffies + PVG_TASK_TIMEOUT;
		int idx, ret = -ENODEV;

		if (atomic_read(&submit->pending))
			break;
		pvg_submit_drop_ins(submit);
		if (!task->given_up && task->defined && task->child &&
		    drm_dev_enter(&pvg->drm, &idx)) {
			for (;;) {
				ret = pvg_child_queue(pvg, task->child, submit->number,
						      submit->payload, submit->words,
						      &submit->job);
				if (ret != -EAGAIN)
					break;
				ret = pvg_child_wait_room(pvg, task->child, deadline, false);
				if (ret)
					break;
			}
			drm_dev_exit(idx);
		}
		kfree(submit->payload);
		submit->payload = NULL;
		list_move_tail(&submit->link, &task->queued);
		task->held_back_count--;
		task->queued_count++;
		if (ret) {
			dev_warn_ratelimited(&pvg->pdev->dev,
					     "task %u: held-back submit %llu not sent: %d\n",
					     task->id, submit->stamp, ret);
			pvg_submit_end_unsent(submit, ret);
		} else {
			smp_store_release(&submit->sent, true);
		}
		moved = true;
	}
	/* A waiter for a held-back number looks again. */
	if (moved)
		wake_up_all(&pvg->child_wait);
}

static void pvg_task_send_work(struct work_struct *work)
{
	struct pvg_task *task = container_of(work, struct pvg_task, send_work);

	mutex_lock(&task->lock);
	pvg_task_send_held(task);
	mutex_unlock(&task->lock);
}

/*
 * The file ends: held-back submits are never sent. Their fences end with
 * -ECANCELED. The caller then lets the task's lock go and waits for the
 * work, which finds nothing more to send.
 */
static void pvg_task_cancel_held(struct pvg_task *task)
{
	struct pvg_submit *submit, *next;

	lockdep_assert_held(&task->lock);
	list_for_each_entry_safe(submit, next, &task->held_back, link) {
		pvg_submit_drop_ins(submit);
		kfree(submit->payload);
		submit->payload = NULL;
		list_move_tail(&submit->link, &task->queued);
		task->held_back_count--;
		task->queued_count++;
		atomic64_inc(&task->pvg->held_cancelled);
		pvg_submit_end_unsent(submit, -ECANCELED);
	}
}

/*
 * Waits for every queued submit of the task to end, held-back ones too, up
 * to timeout in jiffies, and reaps them. Returns 0, -ETIMEDOUT, or
 * -ERESTARTSYS if interruptible.
 *
 * Held-back submits are sent by the task's work, which needs the task's
 * lock: while one is waited for the lock is let go, so the caller looks at
 * what it holds again after this.
 */
static int pvg_task_drain(struct pvg_task *task, unsigned long timeout,
			  bool interruptible)
{
	unsigned long deadline = jiffies + timeout;

	lockdep_assert_held(&task->lock);
	for (;;) {
		struct pvg_submit *last, *submit;
		struct dma_fence *fence;
		bool held = !list_empty(&task->held_back);
		long left, ret;

		pvg_task_reap(task);
		if (!held && list_empty(&task->queued))
			return 0;
		left = (long)deadline - (long)jiffies;
		if (left <= 0)
			return -ETIMEDOUT;
		/*
		 * The last one in a ring: when it is done, so are those before
		 * it. One that ended without going out may lie behind it.
		 */
		if (held) {
			last = list_last_entry(&task->held_back, struct pvg_submit, link);
		} else {
			last = NULL;
			list_for_each_entry_reverse(submit, &task->queued, link) {
				if (submit->sent) {
					last = submit;
					break;
				}
			}
			if (WARN_ON(!last))
				return -ETIMEDOUT;
		}
		fence = dma_fence_get(&last->fence);
		if (held)
			mutex_unlock(&task->lock);
		ret = dma_fence_wait_timeout(fence, interruptible, left);
		if (held)
			mutex_lock(&task->lock);
		dma_fence_put(fence);
		if (ret < 0)
			return ret;
		if (!ret)
			return -ETIMEDOUT;
		/*
		 * The ring signals a fence and then hands the job back, both
		 * under its lock. Taking the lock once is waiting for that.
		 */
		if (task->child)
			pvg_child_barrier(task->child);
		if (!held) {
			pvg_task_reap(task);
			return list_empty(&task->queued) ? 0 : -ETIMEDOUT;
		}
	}
}

/*
 * The task's work did not end in time: its submits are let go with an error
 * and nothing more is sent for the task. What they held stays the device's.
 */
static void pvg_task_give_up(struct pvg_task *task)
{
	lockdep_assert_held(&task->lock);
	dev_err(&task->pvg->pdev->dev,
		"task %u: %u submits did not end. The task is left to the device\n",
		task->id, task->queued_count);
	task->given_up = true;
	if (task->child)
		pvg_child_abandon_jobs(task->child);
	pvg_task_reap(task);
}

/* Puts a buffer object into the task, for a handle of the task's file. */
int pvg_task_place(struct pvg_task *task, struct pvg_bo *bo)
{
	struct pvg_placement *placement;
	int ret = 0;

	mutex_lock(&task->lock);
	placement = pvg_placement_find(task, bo);
	if (placement) {
		/* Objects have kept it since an earlier handle was closed. */
		if (!placement->handle) {
			placement->handle = true;
			placement->refs++;
		}
		goto out;
	}

	placement = kzalloc(sizeof(*placement), GFP_KERNEL);
	if (!placement) {
		ret = -ENOMEM;
		goto out;
	}
	placement->task = task;
	placement->bo = bo;
	ret = drm_mm_insert_node(&task->mm, &placement->node,
				 bo->page_count / PVG_PAGE_PAGES);
	if (ret) {
		kfree(placement);
		goto out;
	}
	ret = pvg_placement_enter(placement, true);
	if (ret) {
		pvg_placement_enter(placement, false);
		drm_mm_remove_node(&placement->node);
		kfree(placement);
		goto out;
	}
	placement->handle = true;
	placement->refs = 1;
	drm_gem_object_get(&bo->base);
	mutex_lock(&bo->lock);
	list_add(&placement->link, &bo->placements);
	mutex_unlock(&bo->lock);
out:
	mutex_unlock(&task->lock);
	return ret;
}

/* The handle of the task's file is closed. */
void pvg_task_unplace(struct pvg_task *task, struct pvg_bo *bo)
{
	struct pvg_placement *placement;

	mutex_lock(&task->lock);
	/* A queued submit that has ended may be the last to hold it. */
	pvg_task_reap(task);
	placement = pvg_placement_find(task, bo);
	if (placement && placement->handle) {
		placement->handle = false;
		pvg_placement_put(placement);
	}
	mutex_unlock(&task->lock);
}

int pvg_task_address(struct pvg_task *task, struct pvg_bo *bo, u64 *address)
{
	struct pvg_placement *placement;

	mutex_lock(&task->lock);
	placement = pvg_placement_find(task, bo);
	if (placement)
		*address = (u64)placement->node.start << RUXNPVG_PAGE_SHIFT;
	mutex_unlock(&task->lock);
	return placement ? 0 : -ENOENT;
}

/*
 * The place in the task of the buffer object a handle names, with the part
 * offset and size checked against it. The caller holds the task's lock, so
 * the place stays while it looks; *held is the buffer object to put after.
 */
static struct pvg_placement *pvg_task_memory(struct pvg_task *task,
					     struct drm_file *file, u32 handle,
					     u64 offset, u64 size,
					     struct drm_gem_object **held)
{
	struct drm_gem_object *obj = drm_gem_object_lookup(file, handle);
	struct pvg_placement *placement;
	struct pvg_bo *bo = to_pvg_bo(obj);

	if (!obj)
		return ERR_PTR(-ENOENT);
	if (!bo || !size || offset > obj->size || size > obj->size - offset) {
		drm_gem_object_put(obj);
		return ERR_PTR(-EINVAL);
	}
	placement = pvg_placement_find(task, bo);
	if (!placement) {
		drm_gem_object_put(obj);
		return ERR_PTR(-ENOENT);
	}
	*held = obj;
	return placement;
}

/* Room for a description, as its first piece; the pages come as needed. */
static int pvg_description_alloc(struct pvg_task *task, u32 bytes, u32 *slots)
{
	unsigned long slot;
	u32 page, last;

	*slots = DIV_ROUND_UP(bytes, RUXNPVG_DESCRIPTION_SLOT);
	slot = bitmap_find_next_zero_area(task->description_slots,
					  PVG_DESCRIPTION_SLOTS, 0, *slots, 0);
	if (slot >= PVG_DESCRIPTION_SLOTS)
		return -ENOSPC;

	last = ((slot + *slots) * RUXNPVG_DESCRIPTION_SLOT - 1) >> RUXNPVG_PAGE_SHIFT;
	for (page = slot * RUXNPVG_DESCRIPTION_SLOT >> RUXNPVG_PAGE_SHIFT; page <= last; page++) {
		struct page *memory = task->description_pages[page];

		if (memory)
			continue;
		memory = pvg_alloc_pages(task->pvg, PVG_PAGE_SIZE);
		if (!memory)
			return -ENOMEM;
		if (pvg_task_enter(task, RUXNPVG_TASK_DESCRIPTIONS_PAGE + page, memory,
				   false)) {
			pvg_free_pages(memory, PVG_PAGE_SIZE);
			return -ENOMEM;
		}
		task->description_pages[page] = memory;
	}
	bitmap_set(task->description_slots, slot, *slots);
	return slot;
}

static void pvg_description_write(struct pvg_task *task, u32 slot,
				  const void *data, u32 bytes)
{
	u32 at = slot * RUXNPVG_DESCRIPTION_SLOT;

	while (bytes) {
		u32 in_page = at & (PVG_PAGE_SIZE - 1);
		u32 chunk = min_t(u32, bytes, PVG_PAGE_SIZE - in_page);

		memcpy(page_address(task->description_pages[at >> RUXNPVG_PAGE_SHIFT]) + in_page,
		       data, chunk);
		data += chunk;
		at += chunk;
		bytes -= chunk;
	}
}

static __le32 *pvg_object_entry(struct pvg_task *task, u32 id)
{
	return (__le32 *)page_address(task->objects_page) + id * PVG_OBJECT_WORDS;
}

/* Gives up what the object holds. The number is free after. */
static void pvg_object_release(struct pvg_task *task, struct pvg_object *object)
{
	lockdep_assert_held(&task->lock);
	if (object->placement)
		pvg_placement_put(object->placement);
	/* The last hold on a surface unmaps it, which the task had to let first. */
	if (object->surface)
		drm_gem_object_put(object->surface);
	if (object->slots)
		bitmap_clear(task->description_slots, object->slot, object->slots);
	/* A view lets go of its texture; at the file's end that may be gone already. */
	if (object->base && task->objects[object->base].kind)
		task->objects[object->base].views--;
	memset(object, 0, sizeof(*object));
}

/* An operation of the serializer's starts with its number, its length and the object's number. */
static bool pvg_tail_is_operation(const u8 *tail, u32 bytes, u32 id)
{
	return bytes >= 12 && !(bytes % 4) &&
	       get_unaligned_le32(tail + 4) == bytes &&
	       get_unaligned_le32(tail + 8) == id;
}

/*
 * The bytes of a pixel of Metal's MTLPixelFormat, for the ordinary formats a
 * texture over a buffer may have; 0 for any other (compressed, packed 24- and
 * 48-bit, depth and stencil, YUV, extended range). The numbers are those of
 * Metal's public header MTLPixelFormat.h.
 */
static u32 pvg_pixel_bytes(u32 format)
{
	switch (format) {
	case 1:					/* A8Unorm */
	case 10 ... 14:				/* R8 */
		return 1;
	case 20: case 22 ... 25:		/* R16 */
	case 30 ... 34:				/* RG8 */
	case 40 ... 43:				/* B5G6R5, A1BGR5, ABGR4, BGR5A1 */
		return 2;
	case 53 ... 55:				/* R32 */
	case 60: case 62 ... 65:		/* RG16 */
	case 70 ... 74:				/* RGBA8 */
	case 80: case 81:			/* BGRA8 */
	case 90 ... 94:				/* RGB10A2, RG11B10F, RGB9E5, BGR10A2 */
		return 4;
	case 103 ... 105:			/* RG32 */
	case 110: case 112 ... 115:		/* RGBA16 */
		return 8;
	case 123 ... 125:			/* RGBA32 */
		return 16;
	default:
		return 0;
	}
}

/*
 * Operation 9: whether the texture it describes lies inside the buffer it
 * names, the object at byte 12, which has to be of kind 1. A 2D texture or
 * a texture buffer of one level, one sample, one slice, of a format whose
 * pixel the kernel knows, its rows at least as long as its width and the
 * last of them ending inside the buffer.
 */
static bool pvg_texture_buffer_fits(struct pvg_task *task, const u8 *tail, u32 id,
				    u32 *buffer_id)
{
	struct pvg_object *buffer;
	u32 type = tail[PVG_TEXTURE_DESC] & 15;
	u32 pixel = pvg_pixel_bytes(get_unaligned_le16(tail + PVG_TEXTURE_DESC_FORMAT));
	u32 width = get_unaligned_le32(tail + PVG_TEXTURE_DESC_WIDTH);
	u32 height = get_unaligned_le32(tail + PVG_TEXTURE_DESC_HEIGHT);
	u64 offset = get_unaligned_le64(tail + PVG_OP_TEXTURE_BUFFER_OFFSET);
	u64 row = get_unaligned_le64(tail + PVG_OP_TEXTURE_BUFFER_ROW);
	u64 end;

	*buffer_id = get_unaligned_le32(tail + RUXNPVG_OP_VIEW_BASE);
	buffer = *buffer_id != id ? pvg_task_object(task, *buffer_id) : NULL;
	if (!buffer || buffer->kind != PVG_OBJECT_BUFFER)
		return false;
	if (!pixel || !width || !height ||
	    get_unaligned_le32(tail + PVG_TEXTURE_DESC_DEPTH) != 1 ||
	    get_unaligned_le16(tail + PVG_TEXTURE_DESC_LEVELS) != 1 ||
	    get_unaligned_le16(tail + PVG_TEXTURE_DESC_SAMPLES) != 1 ||
	    get_unaligned_le16(tail + PVG_TEXTURE_DESC_ARRAY) != 1 ||
	    offset % PVG_TEXTURE_LINEAR_ALIGN)
		return false;
	switch (type) {
	case PVG_TEXTURE_TYPE_2D:
		if (width > PVG_TEXTURE_MAX_SIDE || height > PVG_TEXTURE_MAX_SIDE ||
		    row % PVG_TEXTURE_LINEAR_ALIGN)
			return false;
		break;
	case PVG_TEXTURE_TYPE_BUFFER:
		if (height != 1)
			return false;
		break;
	default:
		return false;
	}
	/* The last row starts height - 1 rows on and is width pixels long. */
	if (row < (u64)width * pixel ||
	    check_mul_overflow(row, (u64)(height - 1), &end) ||
	    check_add_overflow(end, (u64)width * pixel, &end) ||
	    check_add_overflow(end, offset, &end))
		return false;
	return end <= buffer->size;
}

static int pvg_object_create_ioctl(struct drm_device *drm, void *data,
				   struct drm_file *file)
{
	struct pvg_device *pvg = to_pvg(drm);
	struct drm_pvg_object_create *args = data;
	struct drm_gem_object *held = NULL, *surface_obj = NULL;
	struct pvg_placement *placement = NULL;
	struct pvg_surface *surface;
	struct pvg_object *object;
	struct pvg_task *task;
	u8 head[RUXNPVG_MAX_HEAD] = {};
	u32 head_bytes = 0, bytes, slots, page;
	u16 delete_op = 0;
	u32 view_base = 0;
	u64 buffer_bytes = 0;
	u8 *tail = NULL, *description = NULL;
	u64 address;
	__le32 *entry;
	int slot, idx, ret;

	if (args->pad || !args->id || args->id >= RUXNPVG_MAX_OBJECTS ||
	    args->data_size > RUXNPVG_MAX_DESCRIPTION - RUXNPVG_MAX_HEAD)
		return -EINVAL;
	if (args->data_size) {
		tail = memdup_user(u64_to_user_ptr(args->data), args->data_size);
		if (IS_ERR(tail))
			return PTR_ERR(tail);
	}

	task = pvg_file_task(file);
	if (IS_ERR(task)) {
		kfree(tail);
		return PTR_ERR(task);
	}

	mutex_lock(&task->lock);
	object = &task->objects[args->id];
	if (object->kind) {
		ret = -EEXIST;
		goto out;
	}

	ret = -EINVAL;
	switch (args->kind) {
	case PVG_OBJECT_BUFFER:
		/* The length, the task's page the memory starts at, a byte of flags. */
		if (args->data_size || (args->flags & ~PVG_BUFFER_PRIVATE) ||
		    (args->offset & (PVG_PAGE_SIZE - 1)))
			goto out;
		placement = pvg_task_memory(task, file, args->handle, args->offset,
					    args->size, &held);
		if (IS_ERR(placement))
			break;
		page = placement->node.start + (args->offset >> RUXNPVG_PAGE_SHIFT);
		put_unaligned_le64(args->size, head);
		put_unaligned_le32(page, head + 8);
		head[12] = args->flags;
		head_bytes = 16;
		buffer_bytes = args->size;
		break;
	case PVG_OBJECT_TEXTURE:
		/* The length and the task's page of the memory it is loaded from. */
		if (!args->data_size || args->flags ||
		    (args->offset & (PVG_PAGE_SIZE - 1)) ||
		    (args->size & (PVG_PAGE_SIZE - 1)))
			goto out;
		placement = pvg_task_memory(task, file, args->handle, args->offset,
					    args->size, &held);
		if (IS_ERR(placement))
			break;
		page = placement->node.start + (args->offset >> RUXNPVG_PAGE_SHIFT);
		put_unaligned_le64(args->size, head);
		put_unaligned_le32(page, head + 8);
		head_bytes = 12;
		break;
	case PVG_OBJECT_FUNCTION:
		/* Where its library lies in the task, and the library's length. */
		if (args->flags || args->size > U32_MAX ||
		    !pvg_tail_is_operation(tail, args->data_size, args->id) ||
		    get_unaligned_le32(tail) != RUXNPVG_OP_FUNCTION)
			goto out;
		delete_op = RUXNPVG_OP_DELETE_FUNCTION;
		placement = pvg_task_memory(task, file, args->handle, args->offset,
					    args->size, &held);
		if (IS_ERR(placement))
			break;
		address = ((u64)placement->node.start << RUXNPVG_PAGE_SHIFT) + args->offset;
		put_unaligned_le64(address, head);
		put_unaligned_le32(args->size, head + 8);
		head_bytes = 12;
		break;
	case PVG_OBJECT_STATE:
		if (args->handle || args->flags || args->offset || args->size ||
		    !pvg_tail_is_operation(tail, args->data_size, args->id))
			goto out;
		/* What it is decides what deletes it. */
		switch (get_unaligned_le32(tail)) {
		case RUXNPVG_OP_SAMPLER:
			delete_op = RUXNPVG_OP_DELETE_SAMPLER;
			break;
		case RUXNPVG_OP_DEPTH_STENCIL:
			delete_op = RUXNPVG_OP_DELETE_DEPTH_STENCIL;
			break;
		case RUXNPVG_OP_COMPUTE_PIPELINE:
			delete_op = RUXNPVG_OP_DELETE_COMPUTE_PIPELINE;
			break;
		case RUXNPVG_OP_RENDER_PIPELINE:
			delete_op = RUXNPVG_OP_DELETE_RENDER_PIPELINE;
			break;
		default:
			goto out;
		}
		break;
	case PVG_OBJECT_TEXTURE_VIEW: {
		struct pvg_object *base;
		u32 op, op_bytes;

		/* No head: one operation that names the view and its texture. */
		if (args->handle || args->flags || args->offset || args->size ||
		    !pvg_tail_is_operation(tail, args->data_size, args->id))
			goto out;
		op = get_unaligned_le32(tail);
		op_bytes = op == RUXNPVG_OP_TEXTURE_VIEW ? 20 :
			   op == RUXNPVG_OP_TEXTURE_VIEW_RANGE ? 52 :
			   op == RUXNPVG_OP_TEXTURE_VIEW_SWIZZLE ? 56 :
			   op == PVG_OP_TEXTURE_BUFFER ? PVG_OP_TEXTURE_BUFFER_BYTES : 0;
		if (!op_bytes || args->data_size != op_bytes)
			goto out;
		/* A texture over a buffer: it holds the buffer as a view its texture. */
		if (op == PVG_OP_TEXTURE_BUFFER) {
			if (!pvg_texture_buffer_fits(task, tail, args->id, &view_base))
				goto out;
			break;
		}
		view_base = get_unaligned_le32(tail + RUXNPVG_OP_VIEW_BASE);
		base = view_base != args->id ? pvg_task_object(task, view_base) : NULL;
		if (!base || (base->kind != PVG_OBJECT_TEXTURE &&
			      base->kind != PVG_OBJECT_SURFACE_TEXTURE))
			goto out;
		break;
	}
	case PVG_OBJECT_SURFACE_TEXTURE:
		/* The mapping's id, which the surface gets here if it has none. */
		if (args->flags || args->offset || args->size ||
		    args->data_size != RUXNPVG_OP_SURFACE_TEXTURE_BYTES ||
		    !pvg_tail_is_operation(tail, args->data_size, args->id) ||
		    get_unaligned_le32(tail) != RUXNPVG_OP_SURFACE_TEXTURE)
			goto out;
		surface_obj = drm_gem_object_lookup(file, args->handle);
		if (!surface_obj) {
			ret = -ENOENT;
			goto out;
		}
		surface = pvg_gem_surface(surface_obj);
		if (!surface ||
		    get_unaligned_le32(tail + RUXNPVG_OP_SURFACE_TEXTURE_WIDTH) != surface->width ||
		    get_unaligned_le32(tail + RUXNPVG_OP_SURFACE_TEXTURE_HEIGHT) != surface->height)
			goto out;
		if (!drm_dev_enter(drm, &idx)) {
			ret = -ENODEV;
			goto out;
		}
		ret = pvg_surface_map(pvg, surface);
		drm_dev_exit(idx);
		if (ret)
			goto out;
		put_unaligned_le64(surface->id, head);
		head_bytes = 8;
		break;
	default:
		goto out;
	}
	if (IS_ERR(placement)) {
		ret = PTR_ERR(placement);
		placement = NULL;
		goto out;
	}

	bytes = head_bytes + args->data_size;
	description = kmalloc(bytes, GFP_KERNEL);
	if (!description) {
		ret = -ENOMEM;
		goto out;
	}
	memcpy(description, head, head_bytes);
	if (args->data_size)
		memcpy(description + head_bytes, tail, args->data_size);

	slot = pvg_description_alloc(task, bytes, &slots);
	if (slot < 0) {
		ret = slot;
		goto out;
	}
	pvg_description_write(task, slot, description, bytes);

	object->kind = args->kind;
	object->delete_op = delete_op;
	object->slot = slot;
	object->slots = slots;
	if (placement) {
		placement->refs++;
		object->placement = placement;
	}
	object->surface = surface_obj;
	surface_obj = NULL;
	object->base = view_base;
	object->views = 0;
	object->size = buffer_bytes;
	if (view_base)
		task->objects[view_base].views++;

	/* The kind is written last: an entry without one is no object. */
	address = ((u64)RUXNPVG_TASK_DESCRIPTIONS_PAGE << RUXNPVG_PAGE_SHIFT) +
		  (u64)slot * RUXNPVG_DESCRIPTION_SLOT;
	entry = pvg_object_entry(task, args->id);
	WRITE_ONCE(entry[1], cpu_to_le32(lower_32_bits(address)));
	WRITE_ONCE(entry[2], cpu_to_le32(upper_32_bits(address)));
	WRITE_ONCE(entry[0], cpu_to_le32(args->kind | bytes << 8));
	ret = 0;
out:
	mutex_unlock(&task->lock);
	if (held)
		drm_gem_object_put(held);
	if (surface_obj)
		drm_gem_object_put(surface_obj);
	kfree(description);
	kfree(tail);
	return ret;
}

static struct pvg_object *pvg_task_object(struct pvg_task *task, u32 id)
{
	struct pvg_object *object;

	if (!id || id >= RUXNPVG_MAX_OBJECTS)
		return NULL;
	object = &task->objects[id];
	return object->kind && !object->retired ? object : NULL;
}

static struct pvg_task *pvg_file_task_if_any(struct drm_file *file)
{
	struct pvg_file *priv = file->driver_priv;
	struct pvg_task *task;

	mutex_lock(&priv->lock);
	task = priv->task;
	mutex_unlock(&priv->lock);
	return task;
}

/* Whether a command's resource list may name an object of this kind. */
static bool pvg_kind_is_resource(u8 kind)
{
	return kind == PVG_OBJECT_BUFFER || kind == PVG_OBJECT_TEXTURE ||
	       kind == PVG_OBJECT_SURFACE_TEXTURE;
}

/*
 * Takes an object out of the task. The host keeps what it made for a number
 * and does not look at the list again, so the entry is cleared and the host
 * is told: a buffer or a texture by the command that deletes a resource, a
 * function or a state by the command that carries one of the serializer's
 * delete operations. Only then is the memory or the surface the object named
 * let go, and the number free.
 */
static int pvg_object_destroy_ioctl(struct drm_device *drm, void *data,
				    struct drm_file *file)
{
	struct pvg_device *pvg = to_pvg(drm);
	struct drm_pvg_object_destroy *args = data;
	struct pvg_task *task = pvg_file_task_if_any(file);
	struct pvg_object *object;
	u32 payload[4];
	__le32 *entry;
	int idx, ret;

	if (args->pad)
		return -EINVAL;
	if (!task)
		return -ENOENT;

	mutex_lock(&task->lock);
	object = pvg_task_object(task, args->id);
	if (!object) {
		ret = -ENOENT;
		goto out;
	}
	/* The host's view needs its texture. */
	if (object->views) {
		ret = -EBUSY;
		goto out;
	}
	/* Queued submits may use it, held-back ones too. */
	ret = pvg_task_drain(task, PVG_TASK_TIMEOUT, true);
	if (ret)
		goto out;
	/* The lock may have been let go while held-back ones were waited for. */
	object = pvg_task_object(task, args->id);
	if (!object) {
		ret = -ENOENT;
		goto out;
	}
	if (object->views) {
		ret = -EBUSY;
		goto out;
	}
	entry = pvg_object_entry(task, args->id);
	WRITE_ONCE(entry[0], 0);
	WRITE_ONCE(entry[1], 0);
	WRITE_ONCE(entry[2], 0);

	if (!task->defined || !drm_dev_enter(drm, &idx)) {
		ret = -ENODEV;
	} else {
		payload[0] = task->id;
		/*
		 * A view is deleted as a texture is. 40 with 1001, the
		 * serializer's delete for a texture, was seen to do it too.
		 */
		if (pvg_kind_is_resource(object->kind) ||
		    object->kind == PVG_OBJECT_TEXTURE_VIEW) {
			payload[1] = args->id;
			ret = pvg_task_send(task, RUXNPVG_CMD_DELETE_RESOURCE, payload, 2);
		} else {
			payload[1] = object->delete_op;
			payload[2] = RUXNPVG_OP_DELETE_BYTES;
			payload[3] = args->id;
			ret = pvg_task_send(task, RUXNPVG_CMD_DELETE_OBJECT, payload, 4);
		}
		drm_dev_exit(idx);
	}
	if (ret) {
		/* The host may still have it: what it names stays until the task ends. */
		dev_warn_ratelimited(&pvg->pdev->dev,
				     "task %u: object %u not deleted: %d\n",
				     task->id, args->id, ret);
		object->retired = true;
	} else {
		pvg_object_release(task, object);
	}
out:
	mutex_unlock(&task->lock);
	return ret;
}

static int pvg_object_placement_ioctl(struct drm_device *drm, void *data,
				      struct drm_file *file)
{
	struct drm_pvg_object_placement *args = data;
	struct pvg_task *task = pvg_file_task_if_any(file);
	const __le64 *words;
	int ret = -ENOENT;

	if (args->pad)
		return -EINVAL;
	if (!task)
		return -ENOENT;

	mutex_lock(&task->lock);
	if (pvg_task_object(task, args->id)) {
		words = (const __le64 *)page_address(task->placements_page) +
			args->id * PVG_PLACEMENT_WORDS;
		args->words[0] = le64_to_cpu(READ_ONCE(words[0]));
		args->words[1] = le64_to_cpu(READ_ONCE(words[1]));
		ret = 0;
	}
	mutex_unlock(&task->lock);
	return ret;
}

/*
 * What a submit hands over, copied in: its resources, its command buffers,
 * and room for command 67's payload, or 55's.
 */
struct pvg_submit_args {
	struct drm_pvg_resource *resources;
	struct drm_pvg_command_buffer *buffers;
	u32 resource_count;
	u32 buffer_count;
	bool older;
	u32 *payload;
	unsigned int words;
};

static void pvg_submit_args_free(struct pvg_submit_args *in)
{
	kfree(in->payload);
	kfree(in->resources);
	kfree(in->buffers);
}

static int pvg_submit_args_copy(struct pvg_submit_args *in, u64 resources,
				u32 resource_count, u64 buffers, u32 buffer_count)
{
	memset(in, 0, sizeof(*in));
	if (!buffer_count || buffer_count > RUXNPVG_MAX_BUFFERS ||
	    resource_count > RUXNPVG_MAX_RESOURCES)
		return -EINVAL;
	in->resource_count = resource_count;
	in->buffer_count = buffer_count;
	in->buffers = memdup_array_user(u64_to_user_ptr(buffers), buffer_count,
					sizeof(*in->buffers));
	if (IS_ERR(in->buffers)) {
		int ret = PTR_ERR(in->buffers);

		in->buffers = NULL;
		return ret;
	}
	if (resource_count) {
		in->resources = memdup_array_user(u64_to_user_ptr(resources),
						  resource_count, sizeof(*in->resources));
		if (IS_ERR(in->resources)) {
			int ret = PTR_ERR(in->resources);

			in->resources = NULL;
			pvg_submit_args_free(in);
			return ret;
		}
	}
	in->older = READ_ONCE(exec) == RUXNPVG_CMD_EXEC_OLDER;
	in->words = 3 + (in->older ? 6 : 2) * resource_count + 4 * buffer_count;
	in->payload = kmalloc_array(in->words, sizeof(*in->payload), GFP_KERNEL);
	if (!in->payload) {
		pvg_submit_args_free(in);
		return -ENOMEM;
	}
	return 0;
}

/*
 * Writes the payload: the task, the two counts, for each resource its
 * number and its flags, and for each command buffer its address in the task
 * and its length, 64 bits each. Under the task's lock. With held, each
 * command buffer's place gets a hold, put into held.
 */
static int pvg_submit_payload(struct pvg_task *task, struct drm_file *file,
			      struct pvg_submit_args *in,
			      struct pvg_placement **held, unsigned int *held_count)
{
	u32 *word = in->payload;
	unsigned int i;
	int ret = -EINVAL;

	lockdep_assert_held(&task->lock);
	*word++ = task->id;
	*word++ = in->resource_count;
	*word++ = in->buffer_count;
	for (i = 0; i < in->resource_count; i++) {
		struct pvg_object *object = pvg_task_object(task, in->resources[i].id);

		if (!object || !pvg_kind_is_resource(object->kind) ||
		    (in->resources[i].flags & ~PVG_RESOURCE_WRITTEN))
			goto err;
		*word++ = in->resources[i].id;
		if (in->older) {
			/* Its byte 5 says the resource is written; the rest is zero. */
			*word++ = in->resources[i].flags & PVG_RESOURCE_WRITTEN ? 0x100 : 0;
			memset(word, 0, 4 * sizeof(*word));
			word += 4;
		} else {
			*word++ = in->resources[i].flags;
		}
	}
	for (i = 0; i < in->buffer_count; i++) {
		const struct drm_pvg_command_buffer *buffer = &in->buffers[i];
		struct drm_gem_object *obj;
		struct pvg_placement *placement;
		u64 address;

		if (buffer->pad)
			goto err;
		placement = pvg_task_memory(task, file, buffer->handle, buffer->offset,
					    buffer->size, &obj);
		if (IS_ERR(placement)) {
			ret = PTR_ERR(placement);
			goto err;
		}
		address = ((u64)placement->node.start << RUXNPVG_PAGE_SHIFT) + buffer->offset;
		drm_gem_object_put(obj);
		if (held) {
			placement->refs++;
			held[(*held_count)++] = placement;
		}
		*word++ = lower_32_bits(address);
		*word++ = upper_32_bits(address);
		*word++ = lower_32_bits(buffer->size);
		*word++ = upper_32_bits(buffer->size);
	}
	return 0;

err:
	if (held) {
		while (*held_count)
			pvg_placement_put(held[--(*held_count)]);
	}
	return ret;
}

/*
 * Hands the device command buffers and waits until it has stamped them. A
 * command the device refuses faults the ring, which the ring's sender
 * resumes, and comes back as -EIO: the refusal is found by its place, so it
 * is this command's.
 */
static int pvg_submit_ioctl(struct drm_device *drm, void *data,
			    struct drm_file *file)
{
	struct pvg_device *pvg = to_pvg(drm);
	struct drm_pvg_submit *args = data;
	struct pvg_submit_args in;
	struct pvg_task *task;
	int idx, ret;

	if (args->flags || args->pad)
		return -EINVAL;
	ret = pvg_submit_args_copy(&in, args->resources, args->resource_count,
				   args->buffers, args->buffer_count);
	if (ret)
		return ret;

	task = pvg_file_task(file);
	if (IS_ERR(task)) {
		ret = PTR_ERR(task);
		goto out;
	}

	mutex_lock(&task->lock);
	pvg_task_reap(task);
	ret = pvg_submit_payload(task, file, &in, NULL, NULL);
	if (ret)
		goto unlock;
	if (!task->defined || !drm_dev_enter(drm, &idx)) {
		ret = -ENODEV;
		goto unlock;
	}
	ret = pvg_task_send(task, in.older ? RUXNPVG_CMD_EXEC_OLDER : RUXNPVG_CMD_EXEC,
			    in.payload, in.words);
	drm_dev_exit(idx);
	atomic64_inc(&pvg->submits);
	if (ret == -EIO)
		atomic64_inc(&pvg->submits_refused);
unlock:
	mutex_unlock(&task->lock);
out:
	pvg_submit_args_free(&in);
	return ret;
}

/*
 * The submit's fence goes into the reservation of every resource it names:
 * as a write for those it writes, so that whoever honours implicit fences
 * waits for it, the display above all, and since version 3 as a read for the
 * others. Before the command goes into the ring, under the task's lock. A
 * reservation with no room is -ENOMEM: the caller sends nothing then, and
 * the fence ends with the error where it was put already.
 */
static int pvg_submit_fence_resources(struct pvg_task *task,
				      struct pvg_submit *submit,
				      const struct pvg_submit_args *in)
{
	unsigned int i;

	for (i = 0; i < in->resource_count; i++) {
		bool written = in->resources[i].flags & PVG_RESOURCE_WRITTEN;
		struct pvg_object *object;
		struct dma_resv *resv;
		int ret;

		object = pvg_task_object(task, in->resources[i].id);
		if (!object)
			continue;
		if (object->surface)
			resv = object->surface->resv;
		else if (object->placement)
			resv = object->placement->bo->base.resv;
		else
			continue;
		dma_resv_lock(resv, NULL);
		ret = dma_resv_reserve_fences(resv, 1);
		if (!ret)
			dma_resv_add_fence(resv, &submit->fence,
					   written ? DMA_RESV_USAGE_WRITE : DMA_RESV_USAGE_READ);
		dma_resv_unlock(resv);
		if (ret)
			return ret;
	}
	return 0;
}

/*
 * Version 3: what QUEUE takes in and hands out besides the command. The
 * fences going in as QUEUE found them, before they are taken apart, and the
 * sync objects to signal with what each needs.
 */
struct pvg_queue_syncs {
	struct dma_fence **fences;
	unsigned int fence_count;
	struct drm_syncobj **signals;
	struct dma_fence_chain **chains;
	u64 *points;
	unsigned int signal_count;
};

static void pvg_queue_syncs_free(struct pvg_queue_syncs *syncs)
{
	unsigned int i;

	for (i = 0; i < syncs->fence_count; i++)
		dma_fence_put(syncs->fences[i]);
	for (i = 0; i < syncs->signal_count; i++) {
		if (syncs->signals[i])
			drm_syncobj_put(syncs->signals[i]);
		dma_fence_chain_free(syncs->chains[i]);
	}
	kfree(syncs->fences);
	kfree(syncs->signals);
	kfree(syncs->chains);
	kfree(syncs->points);
	memset(syncs, 0, sizeof(*syncs));
}

/*
 * Takes in the sync_file and the sync objects of a QUEUE: the fences to
 * wait for, found now, and for each sync object to signal its reference
 * and, for a timeline, the link of the chain it will need.
 */
static int pvg_queue_syncs_get(struct pvg_queue_syncs *syncs, struct drm_file *file,
			       const struct drm_pvg_queue *args)
{
	struct drm_pvg_sync_op *ops = NULL;
	unsigned int i;
	int ret = 0;

	memset(syncs, 0, sizeof(*syncs));
	syncs->fences = kcalloc(1 + args->wait_count, sizeof(*syncs->fences), GFP_KERNEL);
	if (!syncs->fences)
		return -ENOMEM;
	if (args->flags & PVG_QUEUE_FENCE_IN) {
		struct dma_fence *fence = sync_file_get_fence(args->fence_fd);

		if (!fence)
			return -EINVAL;
		syncs->fences[syncs->fence_count++] = fence;
	}
	if (args->wait_count) {
		ops = memdup_array_user(u64_to_user_ptr(args->waits), args->wait_count,
					sizeof(*ops));
		if (IS_ERR(ops))
			return PTR_ERR(ops);
		for (i = 0; i < args->wait_count; i++) {
			struct dma_fence *fence;

			if (ops[i].flags) {
				ret = -EINVAL;
				break;
			}
			ret = drm_syncobj_find_fence(file, ops[i].handle, ops[i].point, 0,
						     &fence);
			if (ret)
				break;
			syncs->fences[syncs->fence_count++] = fence;
		}
		kfree(ops);
		if (ret)
			return ret;
	}
	if (!args->signal_count)
		return 0;

	ops = memdup_array_user(u64_to_user_ptr(args->signals), args->signal_count,
				sizeof(*ops));
	if (IS_ERR(ops))
		return PTR_ERR(ops);
	syncs->signals = kcalloc(args->signal_count, sizeof(*syncs->signals), GFP_KERNEL);
	syncs->chains = kcalloc(args->signal_count, sizeof(*syncs->chains), GFP_KERNEL);
	syncs->points = kcalloc(args->signal_count, sizeof(*syncs->points), GFP_KERNEL);
	if (!syncs->signals || !syncs->chains || !syncs->points) {
		kfree(ops);
		return -ENOMEM;
	}
	for (i = 0; i < args->signal_count; i++) {
		struct drm_syncobj *syncobj;
		struct dma_fence *last;

		if (ops[i].flags) {
			ret = -EINVAL;
			break;
		}
		syncobj = drm_syncobj_find(file, ops[i].handle);
		if (!syncobj) {
			ret = -ENOENT;
			break;
		}
		syncs->signals[i] = syncobj;
		syncs->points[i] = ops[i].point;
		syncs->signal_count = i + 1;
		if (!ops[i].point)
			continue;
		/* A timeline's points only go up. */
		last = drm_syncobj_fence_get(syncobj);
		if (last && to_dma_fence_chain(last) && last->seqno >= ops[i].point)
			ret = -EINVAL;
		dma_fence_put(last);
		if (ret)
			break;
		syncs->chains[i] = dma_fence_chain_alloc();
		if (!syncs->chains[i]) {
			ret = -ENOMEM;
			break;
		}
	}
	kfree(ops);
	return ret;
}

/* The submit's fence into the sync objects to signal, once it is queued. */
static void pvg_queue_syncs_signal(struct pvg_queue_syncs *syncs,
				   struct dma_fence *fence)
{
	unsigned int i;

	for (i = 0; i < syncs->signal_count; i++) {
		if (syncs->points[i]) {
			drm_syncobj_add_point(syncs->signals[i], syncs->chains[i], fence,
					      syncs->points[i]);
			syncs->chains[i] = NULL;
		} else {
			drm_syncobj_replace_fence(syncs->signals[i], fence);
		}
	}
}

/*
 * The fences a submit waits for, taken apart: those of sync_files and of
 * timelines that hold several. One that has signalled, or one of the file's
 * own submits, is left out: the ring runs the file's commands in order.
 */
static int pvg_submit_take_ins(struct pvg_submit *submit, struct pvg_task *task,
			       const struct pvg_queue_syncs *syncs)
{
	unsigned int i;

	for (i = 0; i < syncs->fence_count; i++) {
		struct dma_fence_unwrap iter;
		struct dma_fence *fence;

		dma_fence_unwrap_for_each(fence, &iter, syncs->fences[i]) {
			struct pvg_in *in;

			if (fence->context == task->fence_context ||
			    dma_fence_is_signaled(fence))
				continue;
			if (submit->in_count == submit->in_room) {
				unsigned int room = max(4U, 2 * submit->in_room);
				struct pvg_in *ins;

				if (room > PVG_MAX_INS)
					return -EINVAL;
				ins = krealloc_array(submit->ins, room, sizeof(*ins), GFP_KERNEL);
				if (!ins)
					return -ENOMEM;
				submit->ins = ins;
				submit->in_room = room;
			}
			in = &submit->ins[submit->in_count++];
			INIT_LIST_HEAD(&in->cb.node);
			in->fence = dma_fence_get(fence);
			in->submit = submit;
		}
	}
	return 0;
}

/*
 * The first ring holds nothing back: there QUEUE waits for the fences going
 * in before it sends, as it waits for the GPU after. Not under the task's
 * lock. Returns 0, -ETIMEDOUT or -ERESTARTSYS.
 */
static int pvg_submit_wait_ins(struct pvg_submit *submit, unsigned long deadline)
{
	unsigned int i;

	for (i = 0; i < submit->in_count; i++) {
		long left = (long)deadline - (long)jiffies;
		long ret;

		ret = left > 0 ? dma_fence_wait_timeout(submit->ins[i].fence, true, left) : 0;
		if (ret < 0)
			return ret;
		if (!ret)
			return -ETIMEDOUT;
	}
	return 0;
}

/*
 * Version 2: hands the device command buffers as SUBMIT does and returns
 * once they are in the task's ring, with the submit's number. Where the task
 * has no ring of its own it waits as SUBMIT does.
 *
 * Version 3: with fences going in that have not signalled, or behind a
 * submit that is held back, the submit is held back: it has its number and
 * its fence, and goes into the ring later, by pvg_task_send_held().
 */
static int pvg_queue_ioctl(struct drm_device *drm, void *data,
			   struct drm_file *file)
{
	struct pvg_device *pvg = to_pvg(drm);
	struct drm_pvg_queue *args = data;
	unsigned long deadline = jiffies + PVG_TASK_TIMEOUT;
	struct pvg_queue_syncs syncs = {};
	struct sync_file *sync = NULL;
	struct pvg_submit_args in;
	struct pvg_submit *submit;
	struct pvg_task *task;
	bool hold = false;
	int idx, ret, fd = -1;
	u32 number;

	if (args->flags & ~(PVG_QUEUE_FENCE_OUT | PVG_QUEUE_FENCE_IN | PVG_QUEUE_SYNCOBJS))
		return -EINVAL;
	if (!(args->flags & PVG_QUEUE_SYNCOBJS) &&
	    (args->waits || args->signals || args->wait_count || args->signal_count))
		return -EINVAL;
	if (args->wait_count > PVG_MAX_SYNCS || args->signal_count > PVG_MAX_SYNCS)
		return -EINVAL;
	ret = pvg_submit_args_copy(&in, args->resources, args->resource_count,
				   args->buffers, args->buffer_count);
	if (ret)
		return ret;
	number = in.older ? RUXNPVG_CMD_EXEC_OLDER : RUXNPVG_CMD_EXEC;
	submit = kzalloc(sizeof(*submit), GFP_KERNEL);
	if (!submit) {
		ret = -ENOMEM;
		goto out;
	}
	spin_lock_init(&submit->lock);
	INIT_LIST_HEAD(&submit->job.link);
	submit->pvg = pvg;
	submit->job.done = pvg_submit_done;
	submit->job.result = -EINPROGRESS;

	if (args->flags & (PVG_QUEUE_FENCE_IN | PVG_QUEUE_SYNCOBJS)) {
		ret = pvg_queue_syncs_get(&syncs, file, args);
		if (ret)
			goto free_submit;
	}
	if (args->flags & PVG_QUEUE_FENCE_OUT) {
		fd = get_unused_fd_flags(O_CLOEXEC);
		if (fd < 0) {
			ret = fd;
			goto free_submit;
		}
	}

	task = pvg_file_task(file);
	if (IS_ERR(task)) {
		ret = PTR_ERR(task);
		goto free_submit;
	}
	submit->task = task;
	ret = pvg_submit_take_ins(submit, task, &syncs);
	if (ret)
		goto free_submit;
	/* A ring is the task's from its making to the file's end. */
	if (!task->child && submit->in_count) {
		ret = pvg_submit_wait_ins(submit, deadline);
		if (ret)
			goto free_submit;
		pvg_submit_drop_ins(submit);
	}

	mutex_lock(&task->lock);
	pvg_task_reap(task);
	/*
	 * Room among the file's submits in flight: the oldest has to end. A
	 * held-back one is sent by the work, which needs the lock.
	 */
	while (task->queued_count + task->held_back_count >= PVG_MAX_QUEUED) {
		bool held = list_empty(&task->queued);
		struct pvg_submit *oldest = held ?
			list_first_entry(&task->held_back, struct pvg_submit, link) :
			list_first_entry(&task->queued, struct pvg_submit, link);
		struct dma_fence *fence = dma_fence_get(&oldest->fence);
		long left = (long)deadline - (long)jiffies;

		if (held)
			mutex_unlock(&task->lock);
		ret = left > 0 ? dma_fence_wait_timeout(fence, true, left) : 0;
		if (held)
			mutex_lock(&task->lock);
		dma_fence_put(fence);
		if (ret <= 0) {
			ret = ret ? ret : -ETIMEDOUT;
			goto unlock;
		}
		if (task->child) {
			pvg_child_barrier(task->child);
		}
		pvg_task_reap(task);
	}
	ret = pvg_submit_payload(task, file, &in, submit->held, &submit->held_count);
	if (ret)
		goto unlock;
	dma_fence_init64(&submit->fence, &pvg_fence_ops, &submit->lock,
			 task->fence_context, task->last_stamp + 1);
	submit->stamp = task->last_stamp + 1;
	if (fd >= 0) {
		sync = sync_file_create(&submit->fence);
		if (!sync) {
			ret = -ENOMEM;
			goto unqueued;
		}
	}
	ret = pvg_submit_fence_resources(task, submit, &in);
	if (ret)
		goto unqueued;

	if (!task->defined || !drm_dev_enter(drm, &idx)) {
		ret = -ENODEV;
		goto unqueued;
	}
	hold = task->child && (submit->in_count || !list_empty(&task->held_back));
	if (hold) {
		/* Its words wait in the submit; the ring gets them later. */
		submit->number = number;
		submit->payload = in.payload;
		submit->words = in.words;
		in.payload = NULL;
		ret = 0;
	} else if (task->child) {
		for (;;) {
			ret = pvg_child_queue(pvg, task->child, number, in.payload,
					      in.words, &submit->job);
			if (ret != -EAGAIN)
				break;
			ret = pvg_child_wait_room(pvg, task->child, deadline, true);
			if (ret)
				break;
		}
		if (!ret)
			submit->sent = true;
	} else {
		/* The first ring: sent and waited for, as SUBMIT does. */
		ret = pvg_ring_send_slow(pvg, number, in.payload, in.words,
					 PVG_TASK_TIMEOUT);
		if (!ret || ret == -EIO) {
			pvg_submit_done(&submit->job, ret);
			submit->job.result = ret;
			submit->sent = true;
			ret = 0;
		}
	}
	drm_dev_exit(idx);
	if (ret)
		goto unqueued;

	task->last_stamp = submit->stamp;
	if (hold) {
		unsigned int i;

		/*
		 * One more than the fences, until their callbacks are on: a
		 * callback that ends the count schedules the work, and so does
		 * the end of this.
		 */
		atomic_set(&submit->pending, submit->in_count + 1);
		for (i = 0; i < submit->in_count; i++) {
			if (dma_fence_add_callback(submit->ins[i].fence, &submit->ins[i].cb,
						   pvg_in_signalled))
				atomic_dec(&submit->pending);
		}
		list_add_tail(&submit->link, &task->held_back);
		task->held_back_count++;
		atomic64_inc(&pvg->held_back);
		atomic_dec(&submit->pending);
		pvg_task_send_held(task);
	} else {
		list_add_tail(&submit->link, &task->queued);
		task->queued_count++;
	}
	atomic64_inc(&pvg->submits);
	atomic64_inc(&pvg->queued);
	/* The sync objects take references of their own. */
	pvg_queue_syncs_signal(&syncs, &submit->fence);
	args->stamp = submit->stamp;
	if (sync) {
		fd_install(fd, sync->file);
		args->fence_fd = fd;
	} else {
		args->fence_fd = -1;
	}
	mutex_unlock(&task->lock);
	pvg_queue_syncs_free(&syncs);
	pvg_submit_args_free(&in);
	return 0;

unqueued:
	/* Nothing went to the device: the fence ends with the error. */
	while (submit->held_count)
		pvg_placement_put(submit->held[--submit->held_count]);
	pvg_submit_drop_ins(submit);
	kfree(submit->payload);
	submit->payload = NULL;
	pvg_submit_signal(submit, ret);
	if (sync)
		fput(sync->file);
	dma_fence_put(&submit->fence);
	submit = NULL;
unlock:
	mutex_unlock(&task->lock);
free_submit:
	if (submit) {
		pvg_submit_drop_ins(submit);
		kfree(submit);
	}
out:
	pvg_queue_syncs_free(&syncs);
	if (fd >= 0)
		put_unused_fd(fd);
	pvg_submit_args_free(&in);
	return ret == -ERESTARTSYS ? -EINTR : ret;
}

/*
 * Whether a waiter for the submit has to look: its fence has signalled, or
 * it is in the ring and the ring has news of it. A held-back submit's job is
 * not the ring's yet.
 */
static bool pvg_submit_due(struct pvg_device *pvg, struct pvg_child *child,
			   struct pvg_submit *submit)
{
	return dma_fence_is_signaled(&submit->fence) ||
	       (smp_load_acquire(&submit->sent) &&
		pvg_child_job_due(pvg, child, &submit->job));
}

/* Version 2: waits for a queued submit of the file, by its number. */
static int pvg_wait_ioctl(struct drm_device *drm, void *data,
			  struct drm_file *file)
{
	struct drm_pvg_wait *args = data;
	struct pvg_task *task = pvg_file_task_if_any(file);
	struct pvg_submit *submit, *found = NULL;
	struct pvg_child *child;
	struct dma_fence *fence;
	signed long timeout;
	long ret;

	if (args->flags || args->pad || !args->stamp || !task)
		return -EINVAL;
	timeout = drm_timeout_abs_to_jiffies(args->timeout_ns);

	mutex_lock(&task->lock);
	if (args->stamp > task->last_stamp) {
		mutex_unlock(&task->lock);
		return -EINVAL;
	}
	pvg_task_reap(task);
	if (args->stamp <= task->reaped) {
		ret = task->last_stamp - args->stamp < PVG_TASK_WINDOW &&
		      test_bit(args->stamp % PVG_TASK_WINDOW, task->refused) ? -EIO : 0;
		mutex_unlock(&task->lock);
		return ret;
	}
	list_for_each_entry(submit, &task->queued, link) {
		if (submit->stamp == args->stamp) {
			found = submit;
			break;
		}
	}
	if (!found) {
		list_for_each_entry(submit, &task->held_back, link) {
			if (submit->stamp == args->stamp) {
				found = submit;
				break;
			}
		}
	}
	if (WARN_ON(!found)) {
		mutex_unlock(&task->lock);
		return -EINVAL;
	}
	fence = dma_fence_get(&found->fence);
	child = task->child;
	mutex_unlock(&task->lock);

	/*
	 * On a ring of the task's own the waiter looks itself when the stamp
	 * or a refusal is there, rather than wait for the work item to signal.
	 * The fence keeps the submit, and the file the ring.
	 */
	if (child) {
		struct pvg_device *pvg = to_pvg(drm);
		unsigned long deadline = jiffies + timeout;
		bool forever = timeout == MAX_SCHEDULE_TIMEOUT;

		bool timed_out = false;

		for (;;) {
			long woken;

			pvg_child_retire(pvg, child, timed_out);
			if (dma_fence_is_signaled(fence)) {
				ret = fence->error;
				break;
			}
			if (!forever && time_after_eq(jiffies, deadline)) {
				ret = -ETIMEDOUT;
				break;
			}
			/* 0 when woken for the job, -ETIME at the look's time. */
			woken = wait_event_interruptible_hrtimeout(pvg->child_wait,
					pvg_submit_due(pvg, child, found), pvg_recheck());
			if (woken == -ERESTARTSYS) {
				ret = -EINTR;
				break;
			}
			timed_out = woken == -ETIME;
		}
	} else {
		ret = dma_fence_wait_timeout(fence, true, timeout);
		if (ret > 0)
			ret = fence->error;
		else if (!ret)
			ret = -ETIMEDOUT;
		else if (ret == -ERESTARTSYS)
			ret = -EINTR;
	}
	dma_fence_put(fence);
	return ret;
}

/* Has the host write textures back into the memory they name. */
static int pvg_sync_ioctl(struct drm_device *drm, void *data,
			  struct drm_file *file)
{
	struct drm_pvg_sync *args = data;
	struct pvg_task *task = pvg_file_task_if_any(file);
	u32 *payload;
	unsigned int i;
	int idx, ret;

	if (args->flags || !args->count || args->count > RUXNPVG_MAX_RESOURCES)
		return -EINVAL;
	if (!task)
		return -ENOENT;

	payload = kmalloc_array(args->count + 2, sizeof(*payload), GFP_KERNEL);
	if (!payload)
		return -ENOMEM;
	if (copy_from_user(payload + 2, u64_to_user_ptr(args->ids),
			   args->count * sizeof(*payload))) {
		kfree(payload);
		return -EFAULT;
	}

	mutex_lock(&task->lock);
	/* What is read back is what queued submits drew. */
	ret = pvg_task_drain(task, PVG_TASK_TIMEOUT, true);
	if (ret)
		goto unlock;
	payload[0] = task->id;
	payload[1] = args->count;
	ret = -EINVAL;
	for (i = 0; i < args->count; i++) {
		struct pvg_object *object = pvg_task_object(task, payload[2 + i]);

		if (!object || !pvg_kind_is_resource(object->kind))
			goto unlock;
	}
	if (!task->defined || !drm_dev_enter(drm, &idx)) {
		ret = -ENODEV;
		goto unlock;
	}
	ret = pvg_task_send(task, RUXNPVG_CMD_SYNC, payload, args->count + 2);
	drm_dev_exit(idx);
unlock:
	mutex_unlock(&task->lock);
	kfree(payload);
	return ret;
}

static int pvg_get_param_ioctl(struct drm_device *drm, void *data,
			       struct drm_file *file)
{
	struct pvg_device *pvg = to_pvg(drm);
	struct drm_pvg_get_param *args = data;

	if (args->pad)
		return -EINVAL;
	switch (args->param) {
	case PVG_PARAM_UAPI_VERSION:
		args->value = PVG_UAPI_VERSION;
		break;
	case PVG_PARAM_PROTOCOL_VERSION:
		args->value = RUXNPVG_VERSION;
		break;
	case PVG_PARAM_MAX_OBJECTS:
		args->value = RUXNPVG_MAX_OBJECTS;
		break;
	case PVG_PARAM_MAX_RESOURCES:
		args->value = RUXNPVG_MAX_RESOURCES;
		break;
	case PVG_PARAM_MAX_BUFFERS:
		args->value = RUXNPVG_MAX_BUFFERS;
		break;
	case PVG_PARAM_TASK_BYTES:
		args->value = pvg_task_room_pages() << RUXNPVG_PAGE_SHIFT;
		break;
	case PVG_PARAM_MAX_DESCRIPTION:
		args->value = RUXNPVG_MAX_DESCRIPTION;
		break;
	case PVG_PARAM_OWN_RING: {
		struct pvg_task *task = pvg_file_task(file);

		if (IS_ERR(task))
			return PTR_ERR(task);
		args->value = task->child != NULL;
		break;
	}
	case PVG_PARAM_MAX_QUEUED:
		args->value = PVG_MAX_QUEUED;
		break;
	case PVG_PARAM_FEATURES:
		args->value = PVG_FEATURE_FENCE_IN | PVG_FEATURE_SYNCOBJ |
			      PVG_FEATURE_SYNCOBJ_TIMELINE | PVG_FEATURE_READ_FENCES |
			      PVG_FEATURE_SIGNAL_SOON | PVG_FEATURE_BO_INFO |
			      PVG_FEATURE_TEXTURE_BUFFER | PVG_FEATURE_PITCH_ALIGN;
		if (pvg->firmware)
			args->value |= PVG_FEATURE_FIRMWARE_SCREEN;
		break;
	case PVG_PARAM_MAX_SYNCS:
		args->value = PVG_MAX_SYNCS;
		break;
	case PVG_PARAM_MAX_BO_BYTES:
		args->value = 256ULL << 20;
		break;
	case PVG_PARAM_PITCH_ALIGN:
		args->value = PVG_SURFACE_PITCH_ALIGN;
		break;
	default:
		return -EINVAL;
	}
	(void)pvg;
	return 0;
}

/*
 * The file is being closed, and its handles are still there. The task is
 * deleted first: the host then holds nothing of it, and what follows can
 * give memory back and unmap surfaces. Then the objects let go of what they
 * name. The handles' end takes the buffer objects out of a task that no
 * device knows any more, which sends nothing.
 */
void pvg_file_release(struct drm_file *file)
{
	struct pvg_file *priv = file->driver_priv;
	struct pvg_task *task = priv->task;
	unsigned int i;

	if (!task)
		return;
	/*
	 * Held-back submits are never sent now. Their fences' callbacks are
	 * off once they are cancelled, so after the work has run once more
	 * nothing schedules it.
	 */
	mutex_lock(&task->lock);
	pvg_task_cancel_held(task);
	mutex_unlock(&task->lock);
	cancel_work_sync(&task->send_work);

	mutex_lock(&task->lock);
	/* Work in flight may use all of it. */
	if (pvg_task_drain(task, PVG_TASK_TIMEOUT, false))
		pvg_task_give_up(task);
	pvg_task_delete(task);
	if (!task->lost) {
		for (i = 1; i < RUXNPVG_MAX_OBJECTS; i++) {
			if (task->objects[i].kind)
				pvg_object_release(task, &task->objects[i]);
		}
	}
	mutex_unlock(&task->lock);
}

void pvg_file_postclose(struct drm_device *drm, struct drm_file *file)
{
	struct pvg_file *priv = file->driver_priv;

	if (priv->task) {
		/* A file that did not come to its end through the release above. */
		pvg_file_release(file);
		pvg_task_free(priv->task);
	}
	mutex_destroy(&priv->lock);
	kfree(priv);
	file->driver_priv = NULL;
}

const struct drm_ioctl_desc pvg_ioctls[PVG_IOCTL_COUNT] = {
	DRM_IOCTL_DEF_DRV(PVG_GET_PARAM, pvg_get_param_ioctl, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(PVG_BO_CREATE, pvg_bo_create_ioctl, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(PVG_OBJECT_CREATE, pvg_object_create_ioctl, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(PVG_OBJECT_DESTROY, pvg_object_destroy_ioctl, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(PVG_OBJECT_PLACEMENT, pvg_object_placement_ioctl, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(PVG_SUBMIT, pvg_submit_ioctl, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(PVG_SYNC, pvg_sync_ioctl, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(PVG_QUEUE, pvg_queue_ioctl, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(PVG_WAIT, pvg_wait_ioctl, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(PVG_BO_INFO, pvg_bo_info_ioctl, DRM_RENDER_ALLOW),
};
