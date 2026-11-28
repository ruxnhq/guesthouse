// SPDX-License-Identifier: GPL-2.0
/*
 * Child rings: a command ring of its own for a task.
 *
 * The first ring has one sender at a time, and a command that hands the GPU
 * work is stamped when the work is done. So on the first ring a program's
 * long work holds up every other program and the display. A child ring is
 * 64 KiB of guest memory that the device reads commands from in the same
 * form. Its positions are not registers: they lie in the device's root page,
 * in a descriptor of 20 bytes for each ring from 0x400 on. The guest writes
 * how far it has written and wakes the ring by its id; the device writes how
 * far it has read, stamps the ring's cell, which is a word at the start of
 * the first ring's memory, and sets the ring's bit in the done register.
 *
 * A ring holds several commands at once. Each is a job: where it starts, how
 * long it is and its stamp. The device stamps a ring's commands in order. A
 * command it refuses is named by its place in the descriptor; the device
 * reads on, holds the later stamps back, and hands them over when the ring
 * is resumed. So the driver marks the job at that place as refused before
 * the resume, and every job whose stamp the cell has reached after that is
 * done. Whoever looks first does this: the interrupt handler, a sender that
 * waits, or the device's work item, which looks every 20 ms while jobs are in
 * flight, for an interrupt that comes late. Since version 3 a timer looks
 * every millisecond as well while a fence that something waits on has not
 * signalled and jobs are in flight: a page flip, a sync_file's poll() or a
 * fence going into another submit then waits no longer than that.
 *
 * Observed with a played guest, and from a Linux guest with a task's
 * commands in it, one at a time and many in flight.
 */

#include <linux/jiffies.h>
#include <linux/slab.h>
#include <linux/workqueue.h>

#include "pvg.h"

/*
 * Ring ids run from RUXNPVG_CHILD_FIRST_ID to RUXNPVG_CHILD_LAST_ID, and a
 * ring's stamp cell is the word of its id. Its descriptor in the device's
 * root page is in words here, so the header's byte offsets are divided by
 * four.
 */
#define PVG_CHILD_WORDS		(RUXNPVG_CHILD_BYTES / 4)

/*
 * How often the work item looks at jobs in flight with no interrupt and no
 * waiter: one in a thousand comes late under QEMU with HVF's interrupt
 * controller. A waiter looks as often as pvg_recheck() says.
 */
#define PVG_CHILD_RECHECK	msecs_to_jiffies(RUXNPVG_RECHECK_MS)
/* The most jobs a ring has in flight; a sender then waits for room. */
#define PVG_CHILD_MAX_JOBS	512

struct pvg_child {
	u32 id;
	spinlock_t lock;		/* the fields below; also taken in the interrupt */
	struct page *pages;
	struct page *list_page;
	__le32 *mem;
	__le32 *descriptor;
	u32 written;			/* a counter that runs on, in bytes */
	u32 last_stamp;
	/* The refused command the device was last told to skip, until it says none. */
	u32 resumed_at;
	u32 resumed_stamp;
	struct list_head jobs;		/* in flight, oldest first */
	unsigned int job_count;
	struct list_head link;		/* among the device's rings */
	u64 refused;			/* for debugfs */
	u64 queued;
};

static bool pvg_child_look(struct pvg_device *pvg, struct pvg_child *child, bool late);

/*
 * Looks at every ring, LATE as for pvg_child_look(). Returns whether jobs
 * are in flight. From any context.
 */
bool pvg_children_look(struct pvg_device *pvg, bool late)
{
	struct pvg_child *child;
	unsigned long flags;
	bool busy = false;

	spin_lock_irqsave(&pvg->children_lock, flags);
	list_for_each_entry(child, &pvg->children, link) {
		spin_lock(&child->lock);
		busy |= pvg_child_look(pvg, child, late);
		spin_unlock(&child->lock);
	}
	spin_unlock_irqrestore(&pvg->children_lock, flags);
	return busy;
}

/*
 * From the interrupt handler: every ring is looked at, so that jobs end and
 * fences signal without a detour. The work item stays for interrupts that
 * come late.
 */
void pvg_child_irq(struct pvg_device *pvg)
{
	pvg_children_look(pvg, false);
	wake_up_all(&pvg->child_wait);
}

/*
 * The millisecond look. It runs while a fence that something waits on has
 * not signalled and jobs are in flight; what it ends is counted as late, as
 * the work item's is. It only looks: a fence's callbacks only schedule work.
 */
static enum hrtimer_restart pvg_signal_timer_fn(struct hrtimer *timer)
{
	struct pvg_device *pvg = container_of(timer, struct pvg_device, signal_timer);
	bool busy;

	atomic64_inc(&pvg->signal_looks);
	busy = pvg_children_look(pvg, true);
	if (busy && atomic_read(&pvg->fences_wanted) > 0) {
		hrtimer_forward_now(timer, pvg_recheck());
		return HRTIMER_RESTART;
	}
	return HRTIMER_NORESTART;
}

/*
 * Starts the millisecond look if it does not run: a fence is waited on, or
 * a job went into a ring while one is. From any context.
 */
void pvg_signal_soon(struct pvg_device *pvg)
{
	if (!hrtimer_active(&pvg->signal_timer))
		hrtimer_start(&pvg->signal_timer, pvg_recheck(), HRTIMER_MODE_REL);
}

static bool pvg_child_reached(struct pvg_device *pvg, struct pvg_child *child,
			      u32 stamp)
{
	u32 seen = le32_to_cpu(READ_ONCE(pvg->ring.mem[child->id]));

	return (s32)(seen - stamp) >= 0;
}

/*
 * Whether a look would end the job or find a refusal: read without the
 * ring's lock, so that a waiter woken by the interrupt need not wait for the
 * work item.
 */
bool pvg_child_job_due(struct pvg_device *pvg, struct pvg_child *child,
		       struct pvg_job *job)
{
	u32 at = le32_to_cpu(READ_ONCE(child->descriptor[RUXNPVG_CHILD_REFUSED]));

	return pvg_job_ended(job) || pvg_child_reached(pvg, child, job->stamp) ||
	       (at != RUXNPVG_NOT_REFUSED && at != READ_ONCE(child->resumed_at));
}

/* The job in flight whose bytes lie at a place of the ring. */
static struct pvg_job *pvg_child_job_at(struct pvg_child *child, u32 at)
{
	struct pvg_job *job;

	list_for_each_entry(job, &child->jobs, link) {
		u32 start = job->start % RUXNPVG_CHILD_BYTES;
		u32 into = (at - start + RUXNPVG_CHILD_BYTES) % RUXNPVG_CHILD_BYTES;

		if (into < job->length)
			return job;
	}
	return NULL;
}

/*
 * Looks at the ring: has the device skip a refused command after marking its
 * job, then ends every job whose stamp has come. Under the ring's lock.
 * LATE says the look comes from a recheck with no interrupt behind it, so
 * that the jobs it ends are counted as found late. Returns whether jobs are
 * still in flight.
 */
static bool pvg_child_look(struct pvg_device *pvg, struct pvg_child *child, bool late)
{
	struct pvg_job *job, *next;
	unsigned int ended = 0;
	u32 at;

	lockdep_assert_held(&child->lock);
	at = le32_to_cpu(READ_ONCE(child->descriptor[RUXNPVG_CHILD_REFUSED]));
	if (at == RUXNPVG_NOT_REFUSED) {
		child->resumed_at = RUXNPVG_NOT_REFUSED;
	} else {
		job = pvg_child_job_at(child, at);
		/*
		 * The place stays named until the device has taken the
		 * resume. A new refusal at the same place is another job's.
		 */
		if (at != child->resumed_at ||
		    (job && job->stamp != child->resumed_stamp)) {
			if (job) {
				job->refused = true;
				child->resumed_stamp = job->stamp;
			} else {
				child->resumed_stamp = 0;
			}
			child->resumed_at = at;
			child->refused++;
			dev_err(&pvg->pdev->dev,
				"the device refused the command at %#x of ring %u%s\n",
				at, child->id, job ? "" : ", which is no command in flight");
			/*
			 * As on the first ring: the device has read past the
			 * command it refused, and a resume has it hand over
			 * the stamp it holds back, the refused command's own
			 * if nothing came after.
			 */
			pvg_write(pvg, RUXNPVG_REG_CHILD_RESUME, child->id);
		}
	}

	/* The cell is read after the refusal: a stamp past it means a resume. */
	smp_rmb();
	list_for_each_entry_safe(job, next, &child->jobs, link) {
		if (!pvg_child_reached(pvg, child, job->stamp))
			break;
		list_del_init(&job->link);
		child->job_count--;
		if (job->done)
			job->done(job, job->refused ? -EIO : 0);
		/* After this the job is its owner's again. */
		smp_store_release(&job->result, job->refused ? -EIO : 0);
		ended++;
	}
	if (ended && late)
		atomic64_add(ended, &pvg->child_late);
	if (ended) {
		atomic_inc(&pvg->child_ends);
		wake_up_all(&pvg->child_wait);
	}
	return !list_empty(&child->jobs);
}

/*
 * Looks at the ring from outside its lock, LATE as for pvg_child_look().
 * Returns whether jobs are in flight.
 */
bool pvg_child_retire(struct pvg_device *pvg, struct pvg_child *child, bool late)
{
	unsigned long flags;
	bool busy;

	spin_lock_irqsave(&child->lock, flags);
	busy = pvg_child_look(pvg, child, late);
	spin_unlock_irqrestore(&child->lock, flags);
	return busy;
}

/* The device's work item: every ring with jobs is looked at. */
static void pvg_child_work(struct work_struct *work)
{
	struct pvg_device *pvg = container_of(to_delayed_work(work),
					      struct pvg_device, child_work);
	bool busy = pvg_children_look(pvg, true);

	if (busy)
		queue_delayed_work(system_wq, &pvg->child_work, PVG_CHILD_RECHECK);
}

void pvg_children_init(struct pvg_device *pvg)
{
	INIT_LIST_HEAD(&pvg->children);
	spin_lock_init(&pvg->children_lock);
	INIT_DELAYED_WORK(&pvg->child_work, pvg_child_work);
	init_waitqueue_head(&pvg->child_wait);
	hrtimer_setup(&pvg->signal_timer, pvg_signal_timer_fn, CLOCK_MONOTONIC,
		      HRTIMER_MODE_REL);
}

void pvg_children_fini(struct pvg_device *pvg)
{
	hrtimer_cancel(&pvg->signal_timer);
	cancel_delayed_work_sync(&pvg->child_work);
}

/*
 * Writes one command into the ring and wakes the ring. The job is the
 * caller's memory, and stays the ring's until its result is set: see
 * pvg_job_ended(). Returns 0, -EAGAIN if the ring has no room now, or
 * -EINVAL.
 */
int pvg_child_queue(struct pvg_device *pvg, struct pvg_child *child, u32 number,
		    const u32 *payload, unsigned int words, struct pvg_job *job)
{
	u32 length = RUXNPVG_COMMAND_HEADER_BYTES + 4 * words;
	unsigned long flags;
	unsigned int i;
	u32 at, stamp;

	if (words > RUXNPVG_RING_MAX_WORDS)
		return -EINVAL;

	spin_lock_irqsave(&child->lock, flags);
	pvg_child_look(pvg, child, false);
	/*
	 * Nothing the device has not read is written over. Its read position
	 * only goes up, and so does the written one.
	 */
	if (child->job_count >= PVG_CHILD_MAX_JOBS ||
	    child->written - le32_to_cpu(READ_ONCE(child->descriptor[RUXNPVG_CHILD_READ])) +
	    length > RUXNPVG_CHILD_BYTES) {
		if (child->job_count >= PVG_CHILD_MAX_JOBS)
			atomic64_inc(&pvg->child_full_jobs);
		else
			atomic64_inc(&pvg->child_full_bytes);
		spin_unlock_irqrestore(&child->lock, flags);
		return -EAGAIN;
	}

	stamp = ++child->last_stamp;
	job->stamp = stamp;
	job->start = child->written;
	job->length = length;
	job->refused = false;
	job->result = -EINPROGRESS;
	list_add_tail(&job->link, &child->jobs);
	child->job_count++;
	child->queued++;

	pvg_ring_log(pvg, child->id, number, stamp, payload, words);
	at = (child->written % RUXNPVG_CHILD_BYTES) / 4;
	child->mem[at] = cpu_to_le32(number);
	child->mem[(at + 1) % PVG_CHILD_WORDS] = cpu_to_le32(length);
	child->mem[(at + 2) % PVG_CHILD_WORDS] = cpu_to_le32(stamp);
	for (i = 0; i < words; i++)
		child->mem[(at + 3 + i) % PVG_CHILD_WORDS] = cpu_to_le32(payload[i]);
	child->written += length;
	/* The command's words are there before the device is told of them. */
	wmb();
	WRITE_ONCE(child->descriptor[RUXNPVG_CHILD_WRITTEN], cpu_to_le32(child->written));
	pvg_write(pvg, RUXNPVG_REG_CHILD_WAKE, child->id);
	spin_unlock_irqrestore(&child->lock, flags);

	/* A look within the recheck's time, in case no interrupt comes. */
	queue_delayed_work(system_wq, &pvg->child_work, PVG_CHILD_RECHECK);
	if (atomic_read(&pvg->fences_wanted) > 0)
		pvg_signal_soon(pvg);
	return 0;
}

/*
 * Takes a job back from the ring before it ended, when its sender gives up
 * waiting. The command may still run; nothing is told of it then.
 */
static void pvg_child_drop(struct pvg_child *child, struct pvg_job *job)
{
	spin_lock_irq(&child->lock);
	if (!list_empty(&job->link)) {
		list_del_init(&job->link);
		child->job_count--;
		job->result = -ETIMEDOUT;
	}
	spin_unlock_irq(&child->lock);
}

/*
 * Sleeps until the job has ended, a signal comes if interruptible, or the
 * deadline in jiffies has passed. Returns the job's result, -ETIMEDOUT or
 * -ERESTARTSYS; on the last two the job is still the ring's.
 */
static int pvg_child_wait(struct pvg_device *pvg, struct pvg_child *child,
			  struct pvg_job *job, unsigned long deadline,
			  bool interruptible)
{
	bool timed_out = false;

	for (;;) {
		long ret;

		pvg_child_retire(pvg, child, timed_out);
		if (pvg_job_ended(job))
			return job->result;
		if (time_after_eq(jiffies, deadline))
			return -ETIMEDOUT;
		/* 0 when woken for the job, -ETIME at the look's time. */
		if (interruptible) {
			ret = wait_event_interruptible_hrtimeout(pvg->child_wait,
					pvg_child_job_due(pvg, child, job),
					pvg_recheck());
			if (ret == -ERESTARTSYS)
				return ret;
		} else {
			ret = wait_event_hrtimeout(pvg->child_wait,
						   pvg_child_job_due(pvg, child, job),
						   pvg_recheck());
		}
		timed_out = ret == -ETIME;
	}
}

/*
 * Sleeps until the ring has room for a command, as pvg_child_wait() does
 * for a job. Returns 0, -ETIMEDOUT or -ERESTARTSYS.
 */
int pvg_child_wait_room(struct pvg_device *pvg, struct pvg_child *child,
			unsigned long deadline, bool interruptible)
{
	int ends = atomic_read(&pvg->child_ends);
	long ret;

	if (time_after_eq(jiffies, deadline))
		return -ETIMEDOUT;
	/* Room comes when a job ends, and a look that ends one wakes the waiters. */
	pvg_child_retire(pvg, child, false);
	if (interruptible) {
		ret = wait_event_interruptible_hrtimeout(pvg->child_wait,
				atomic_read(&pvg->child_ends) != ends, pvg_recheck());
		if (ret == -ERESTARTSYS)
			return ret;
	} else {
		wait_event_hrtimeout(pvg->child_wait,
				     atomic_read(&pvg->child_ends) != ends, pvg_recheck());
	}
	return 0;
}

/*
 * Sends one command through the ring and sleeps until the device has
 * stamped it. Returns 0, -ETIMEDOUT, or -EIO if the device refused it.
 */
int pvg_child_send(struct pvg_device *pvg, struct pvg_child *child, u32 number,
		   const u32 *payload, unsigned int words, unsigned long timeout)
{
	unsigned long deadline = jiffies + timeout;
	struct pvg_job job = { .done = NULL };
	int ret;

	INIT_LIST_HEAD(&job.link);
	for (;;) {
		ret = pvg_child_queue(pvg, child, number, payload, words, &job);
		if (ret != -EAGAIN)
			break;
		ret = pvg_child_wait_room(pvg, child, deadline, false);
		if (ret)
			return ret;
	}
	if (ret)
		return ret;
	ret = pvg_child_wait(pvg, child, &job, deadline, false);
	if (ret == -ETIMEDOUT && !pvg_job_ended(&job)) {
		pvg_child_drop(child, &job);
		/* It may have ended while it was taken back. */
		if (job.result != -ETIMEDOUT)
			ret = job.result;
	}
	return ret;
}

/* Returns once whoever looks at the ring now is done. */
void pvg_child_barrier(struct pvg_child *child)
{
	spin_lock_irq(&child->lock);
	spin_unlock_irq(&child->lock);
}

/*
 * Lets go of every job in flight without an answer, for a task that is given
 * up. Their owners are told -ETIMEDOUT.
 */
void pvg_child_abandon_jobs(struct pvg_child *child)
{
	struct pvg_job *job, *next;

	spin_lock_irq(&child->lock);
	list_for_each_entry_safe(job, next, &child->jobs, link) {
		list_del_init(&job->link);
		child->job_count--;
		if (job->done)
			job->done(job, -ETIMEDOUT);
		smp_store_release(&job->result, -ETIMEDOUT);
	}
	spin_unlock_irq(&child->lock);
}

void pvg_child_counts(struct pvg_device *pvg, unsigned int *rings, u64 *queued,
		      u64 *refused)
{
	struct pvg_child *child;

	*rings = 0;
	*queued = 0;
	*refused = 0;
	spin_lock_irq(&pvg->children_lock);
	list_for_each_entry(child, &pvg->children, link) {
		(*rings)++;
		*queued += child->queued;
		*refused += child->refused;
	}
	spin_unlock_irq(&pvg->children_lock);
}

static void pvg_child_free(struct pvg_device *pvg, struct pvg_child *child,
			   bool keep_memory)
{
	if (!keep_memory) {
		pvg_free_pages(child->list_page, PVG_PAGE_SIZE);
		pvg_free_pages(child->pages, RUXNPVG_CHILD_BYTES);
		ida_free(&pvg->child_ids, child->id);
	}
	kfree(child);
}

/*
 * Makes a ring and tells the device of it. NULL if there is none to be had:
 * the caller then uses the first ring.
 */
struct pvg_child *pvg_child_create(struct pvg_device *pvg)
{
	struct pvg_child *child;
	__le32 *list;
	unsigned int i;
	int ret;

	child = kzalloc(sizeof(*child), GFP_KERNEL);
	if (!child)
		return NULL;
	spin_lock_init(&child->lock);
	INIT_LIST_HEAD(&child->jobs);
	INIT_LIST_HEAD(&child->link);
	ret = ida_alloc_range(&pvg->child_ids, RUXNPVG_CHILD_FIRST_ID, RUXNPVG_CHILD_LAST_ID,
			      GFP_KERNEL);
	if (ret < 0) {
		kfree(child);
		return NULL;
	}
	child->id = ret;
	child->resumed_at = RUXNPVG_NOT_REFUSED;
	child->pages = pvg_alloc_pages(pvg, RUXNPVG_CHILD_BYTES);
	child->list_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	if (!child->pages || !child->list_page)
		goto err;
	child->mem = page_address(child->pages);
	list = page_address(child->list_page);
	for (i = 0; i < RUXNPVG_CHILD_BYTES >> RUXNPVG_PAGE_SHIFT; i++)
		list[i] = cpu_to_le32(pvg_page_number(child->pages) + i);

	/*
	 * The device reads the descriptor when the ring is defined, and runs
	 * at once whatever lies before the written position. The cell still
	 * holds the last stamp of whoever had this id before.
	 */
	child->descriptor = pvg->ring.root + (RUXNPVG_ROOT_CHILD_DESCRIPTORS / 4) +
			    (child->id - 1) * (RUXNPVG_CHILD_DESCRIPTOR_BYTES / 4);
	child->descriptor[RUXNPVG_CHILD_WRITTEN] = 0;
	child->descriptor[RUXNPVG_CHILD_READ] = 0;
	child->descriptor[RUXNPVG_CHILD_REFUSED] = cpu_to_le32(RUXNPVG_NOT_REFUSED);
	child->descriptor[RUXNPVG_CHILD_CELL] = cpu_to_le32(child->id);
	child->descriptor[RUXNPVG_CHILD_PAGE_LIST] = cpu_to_le32(pvg_page_number(child->list_page));
	WRITE_ONCE(pvg->ring.mem[child->id], 0);

	ret = pvg_ring_send(pvg, RUXNPVG_CMD_CHILD_DEFINE, &child->id, 1);
	if (ret == -EIO) {
		/* A driver before this one left a ring of this id. */
		ret = pvg_ring_send(pvg, RUXNPVG_CMD_CHILD_DELETE, &child->id, 1);
		if (!ret)
			ret = pvg_ring_send(pvg, RUXNPVG_CMD_CHILD_DEFINE, &child->id, 1);
	}
	if (ret || !(le32_to_cpu(READ_ONCE(child->descriptor[RUXNPVG_CHILD_CELL])) &
		     RUXNPVG_CHILD_DEFINED)) {
		dev_warn(&pvg->pdev->dev, "ring %u not defined: %d, descriptor %#x\n",
			 child->id, ret,
			 le32_to_cpu(READ_ONCE(child->descriptor[RUXNPVG_CHILD_CELL])));
		goto err;
	}
	spin_lock_irq(&pvg->children_lock);
	list_add_tail(&child->link, &pvg->children);
	spin_unlock_irq(&pvg->children_lock);
	return child;

err:
	pvg_child_free(pvg, child, false);
	return NULL;
}

/*
 * Has the device forget the ring. Its memory is the guest's again after.
 * With keep set the device is told nothing and the ring's memory and id
 * stay the device's: for a task that was given up with work in flight.
 */
void pvg_child_destroy(struct pvg_device *pvg, struct pvg_child *child,
		       bool device_there, bool keep)
{
	int ret = 0;

	spin_lock_irq(&pvg->children_lock);
	list_del_init(&child->link);
	spin_unlock_irq(&pvg->children_lock);
	if (keep) {
		dev_err(&pvg->pdev->dev,
			"ring %u given up with work in flight. Its memory is left to the device\n",
			child->id);
		pvg_child_free(pvg, child, true);
		return;
	}
	if (device_there)
		ret = pvg_ring_send(pvg, RUXNPVG_CMD_CHILD_DELETE, &child->id, 1);
	if (ret)
		dev_err(&pvg->pdev->dev,
			"ring %u not deleted: %d. Its memory is left to the device\n",
			child->id, ret);
	pvg_child_free(pvg, child, ret != 0);
}
