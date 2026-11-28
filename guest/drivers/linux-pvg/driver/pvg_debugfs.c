// SPDX-License-Identifier: GPL-2.0
/*
 * An instrument for asking the device questions from the guest's shell.
 *
 * Under /sys/kernel/debug/pvg/<device>/:
 *
 *   poke      takes one line; the answer goes to the kernel log
 *               r REG              read a register
 *               w REG VALUE        write a register
 *               get OFFSET [N]     N words of the ring's memory
 *               put OFFSET WORD... write words into the ring's memory
 *               root OFFSET [N]    N words of the root page
 *               cmd NUMBER WORD... send a command through the ring and wait
 *               queue NUMBER WORD... write a command into the ring, no more
 *               kick               tell the device of the queued commands
 *               wait               wait for the last queued command
 *               burst N [WORDS]    send N commands with no effect and a
 *                                  payload of WORDS zero words, and count
 *               flood MIB [BYTES]  send commands with no effect of BYTES
 *                                  bytes each until MIB MiB have gone
 *                                  through, as many behind a kick as the
 *                                  ring has room for, and count
 *               state OFFSET [N]   N words of the display's state page
 *               sput OFFSET WORD... write words into the state page
 *               mr REG             read a register of the mapper's device
 *               mw REG VALUE       write one; the two writes that are known
 *                                  to end the host's process or to make the
 *                                  device read old commands are refused
 *   counts    what the interrupt handler has seen
 *   test_fence  for tests of version 3, as the kernel's sw_sync: each
 *             ioctl PVG_TEST_FENCE_NEW on an open file of it gives a
 *             sync_file of a new fence, and a write signals every fence of
 *             that file. Closing the file signals the rest with -ENOENT.
 *             The device is told nothing.
 *   irq_skip  bits for registers the handler leaves unread: 1 the done
 *             bits, 2 the display events, 4 the fault
 *   irq_log   1 has the handler log every interrupt
 */

#include <linux/debugfs.h>
#include <linux/delay.h>
#include <linux/dma-fence.h>
#include <linux/file.h>
#include <linux/io-64-nonatomic-lo-hi.h>
#include <linux/kstrtox.h>
#include <linux/ktime.h>
#include <linux/mm.h>
#include <linux/sched/signal.h>
#include <linux/seq_file.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/sync_file.h>
#include <linux/uaccess.h>

#include "pvg.h"

static struct dentry *pvg_debugfs_root;

static int pvg_counts_show(struct seq_file *m, void *unused)
{
	struct pvg_device *pvg = m->private;
	struct pvg_irq_counts counts;
	unsigned int mapped, held, rings;
	u64 queued, refused;

	spin_lock_irq(&pvg->counts_lock);
	counts = pvg->counts;
	spin_unlock_irq(&pvg->counts_lock);

	seq_printf(m, "taken %llu done %llu events %llu faults %llu empty %llu late %llu\n",
		   counts.taken, counts.done, counts.events, counts.faults,
		   counts.empty, pvg->ring.late);
	seq_printf(m, "late: the first ring's stamps %llu, child rings' jobs ended by a look, not an interrupt, %lld\n",
		   pvg->ring.late, (long long)atomic64_read(&pvg->child_late));
	seq_printf(m, "first ring: written %#x, read %#x\n",
		   READ_ONCE(pvg->ring.written), pvg_read(pvg, RUXNPVG_REG_RING_READ));
	seq_printf(m, "flips: by the interrupt %llu, by a look %llu, timed out %llu, by the timer %llu\n",
		   pvg->flips.by_irq, pvg->flips.by_look, pvg->flips.timed_out,
		   pvg->flips.by_timer);
	seq_printf(m, "frames: copied %llu, shown from the buffer %llu, of them with a write not done %llu\n",
		   pvg->frames.copied, pvg->frames.direct, pvg->frames.undrawn);
	seq_printf(m, "cursor: pictures %llu, shown or hidden %llu, moved %llu\n",
		   pvg->cursors.glyphs, pvg->cursors.shows, pvg->cursors.moves);
	pvg_mapper_counts(pvg, &mapped, &held);
	seq_printf(m, "surfaces: mapped %u, given up and still held %u\n", mapped, held);
	seq_printf(m, "tasks: %d, submits %lld, refused %lld\n",
		   atomic_read(&pvg->tasks), (long long)atomic64_read(&pvg->submits),
		   (long long)atomic64_read(&pvg->submits_refused));
	pvg_child_counts(pvg, &rings, &queued, &refused);
	seq_printf(m, "rings: %u of the tasks', commands %llu, refused %llu; submits queued %lld\n",
		   rings, queued, refused, (long long)atomic64_read(&pvg->queued));
	seq_printf(m, "held back: submits %lld, never sent %lld; fences waited on now %d, looks of a millisecond %lld\n",
		   (long long)atomic64_read(&pvg->held_back),
		   (long long)atomic64_read(&pvg->held_cancelled),
		   atomic_read(&pvg->fences_wanted),
		   (long long)atomic64_read(&pvg->signal_looks));
	seq_printf(m, "full: the first ring %lld, a task's ring by bytes %lld, by jobs %lld\n",
		   (long long)atomic64_read(&pvg->ring_full),
		   (long long)atomic64_read(&pvg->child_full_bytes),
		   (long long)atomic64_read(&pvg->child_full_jobs));
	return 0;
}
DEFINE_SHOW_ATTRIBUTE(pvg_counts);

static void pvg_poke_words(struct pvg_device *pvg, const char *what,
			   __le32 *base, size_t bytes, u32 offset, u32 count)
{
	u32 i;

	for (i = 0; i < count && offset + 4 * i + 4 <= bytes; i++)
		dev_info(&pvg->pdev->dev, "poke: %s %#x = %#010x\n", what,
			 offset + 4 * i,
			 le32_to_cpu(READ_ONCE(base[offset / 4 + i])));
}

static void pvg_poke_burst(struct pvg_device *pvg, u32 count, u32 words)
{
	static const u32 zeroes[PVG_COMMAND_MAX_WORDS];
	struct pvg_irq_counts before, after;
	u64 late = pvg->ring.late;
	u32 i, failed = 0;
	int ret;

	spin_lock_irq(&pvg->counts_lock);
	before = pvg->counts;
	spin_unlock_irq(&pvg->counts_lock);

	for (i = 0; i < count; i++) {
		ret = pvg_ring_send(pvg, RUXNPVG_CMD_NOP, zeroes, words);
		if (ret)
			failed++;
	}

	spin_lock_irq(&pvg->counts_lock);
	after = pvg->counts;
	spin_unlock_irq(&pvg->counts_lock);

	dev_info(&pvg->pdev->dev,
		 "poke: burst of %u: %u failed, interrupts %llu, done %llu, faults %llu, empty %llu, late %llu\n",
		 count, failed, after.taken - before.taken,
		 after.done - before.done, after.faults - before.faults,
		 after.empty - before.empty, pvg->ring.late - late);
}

/*
 * Commands with no effect, LENGTH bytes each, until MIB MiB have gone
 * through the first ring: as many behind one kick as it has room for, then a
 * wait for the last. The ring's lock is let go after each round, so that the
 * display and programs send in between. It is how the written count gets to
 * its wrap at 2^32 by the driver's own path.
 */
static void pvg_poke_flood(struct pvg_device *pvg, u32 mib, u32 length)
{
	struct pvg_ring *ring = &pvg->ring;
	unsigned int words = (length - RUXNPVG_COMMAND_HEADER_BYTES) / 4;
	u64 total = (u64)mib << 20, sent = 0, late = ring->late;
	u32 commands = 0, rounds = 0, failed = 0, idle = 0, wraps = 0;
	struct pvg_irq_counts before, after;
	u32 *zeroes, start, end;
	ktime_t began;

	zeroes = kvzalloc(4 * words, GFP_KERNEL);
	if (!zeroes)
		return;
	spin_lock_irq(&pvg->counts_lock);
	before = pvg->counts;
	spin_unlock_irq(&pvg->counts_lock);
	start = READ_ONCE(ring->written);
	began = ktime_get();

	while (sent < total && !fatal_signal_pending(current)) {
		unsigned int queued = 0;
		u32 from, stamp;
		int ret = 0;

		mutex_lock(&ring->lock);
		from = ring->written;
		while (sent < total) {
			ret = pvg_ring_queue(pvg, RUXNPVG_CMD_NOP, zeroes, words, &stamp);
			if (ret)
				break;
			sent += length;
			queued++;
		}
		if (queued) {
			pvg_ring_kick(pvg);
			if (pvg_ring_poll(pvg, 2000) &&
			    pvg_ring_wait(pvg, ring->last_stamp))
				failed++;
		}
		if (ring->written < from)
			wraps++;
		mutex_unlock(&ring->lock);

		commands += queued;
		rounds++;
		if (ret && ret != -ENOSPC) {
			failed++;
			break;
		}
		/* No room after a wait that went well would be the device stuck. */
		if (!queued && ++idle > 100)
			break;
		if (!queued)
			msleep(10);
		cond_resched();
	}

	end = READ_ONCE(ring->written);
	spin_lock_irq(&pvg->counts_lock);
	after = pvg->counts;
	spin_unlock_irq(&pvg->counts_lock);
	dev_info(&pvg->pdev->dev,
		 "poke: flood of %llu bytes in %u commands of %u bytes, %u rounds, %u failed, %u with no room, written %#x to %#x, %u wraps, %lld ms, interrupts %llu, done %llu, faults %llu, late %llu\n",
		 sent, commands, length, rounds, failed, idle, start, end, wraps,
		 ktime_ms_delta(ktime_get(), began), after.taken - before.taken,
		 after.done - before.done, after.faults - before.faults,
		 ring->late - late);
	kvfree(zeroes);
}

static ssize_t pvg_poke_write(struct file *file, const char __user *user,
			      size_t length, loff_t *position)
{
	struct pvg_device *pvg = file->private_data;
	u32 words[PVG_COMMAND_MAX_WORDS + 2];
	unsigned int count = 0;
	char *line, *rest, *token, *verb;
	int ret;

	line = memdup_user_nul(user, length);
	if (IS_ERR(line))
		return PTR_ERR(line);

	rest = strim(line);
	verb = strsep(&rest, " ");
	while ((token = strsep(&rest, " ")) != NULL) {
		if (!*token)
			continue;
		if (count == ARRAY_SIZE(words) || kstrtou32(token, 0, &words[count])) {
			kfree(line);
			return -EINVAL;
		}
		count++;
	}

	ret = -EINVAL;
	if (!verb) {
		/* an empty line */
	} else if (!strcmp(verb, "r") && count == 1 && words[0] < 0x4000) {
		dev_info(&pvg->pdev->dev, "poke: reg %#x = %#x\n", words[0],
			 pvg_read(pvg, words[0] & ~3));
		ret = 0;
	} else if (!strcmp(verb, "w") && count == 2 && words[0] < 0x4000) {
		pvg_write(pvg, words[0] & ~3, words[1]);
		ret = 0;
	} else if (!strcmp(verb, "get") && count >= 1) {
		pvg_poke_words(pvg, "ring", pvg->ring.mem, RUXNPVG_RING_BYTES,
			       words[0], count > 1 ? words[1] : 1);
		ret = 0;
	} else if (!strcmp(verb, "root") && count >= 1) {
		pvg_poke_words(pvg, "root", pvg->ring.root, PVG_PAGE_SIZE,
			       words[0], count > 1 ? words[1] : 1);
		ret = 0;
	} else if (!strcmp(verb, "put") && count >= 2) {
		unsigned int i;

		for (i = 1; i < count && words[0] + 4 * i <= RUXNPVG_RING_BYTES; i++)
			pvg->ring.mem[words[0] / 4 + i - 1] = cpu_to_le32(words[i]);
		ret = 0;
	} else if (!strcmp(verb, "cmd") && count >= 1) {
		ret = pvg_ring_send(pvg, words[0], words + 1, count - 1);
		dev_info(&pvg->pdev->dev, "poke: command %u with %u words: %d\n",
			 words[0], count - 1, ret);
		ret = 0;
	} else if (!strcmp(verb, "queue") && count >= 1) {
		u32 stamp;

		mutex_lock(&pvg->ring.lock);
		ret = pvg_ring_queue(pvg, words[0], words + 1, count - 1, &stamp);
		if (ret)
			dev_info(&pvg->pdev->dev, "poke: command %u not queued: %d\n",
				 words[0], ret);
		else
			dev_info(&pvg->pdev->dev,
				 "poke: command %u queued with stamp %#x, written %#x\n",
				 words[0], stamp, pvg->ring.written);
		mutex_unlock(&pvg->ring.lock);
	} else if (!strcmp(verb, "kick") && count == 0) {
		mutex_lock(&pvg->ring.lock);
		pvg_ring_kick(pvg);
		mutex_unlock(&pvg->ring.lock);
		ret = 0;
	} else if (!strcmp(verb, "wait") && count == 0) {
		mutex_lock(&pvg->ring.lock);
		ret = pvg_ring_wait(pvg, pvg->ring.last_stamp);
		dev_info(&pvg->pdev->dev, "poke: wait for stamp %#x: %d\n",
			 pvg->ring.last_stamp, ret);
		mutex_unlock(&pvg->ring.lock);
		ret = 0;
	} else if (!strcmp(verb, "state") && count >= 1 && pvg_display_state(pvg)) {
		pvg_poke_words(pvg, "state", pvg_display_state(pvg), PVG_PAGE_SIZE,
			       words[0], count > 1 ? words[1] : 1);
		ret = 0;
	} else if (!strcmp(verb, "sput") && count >= 2 && pvg_display_state(pvg)) {
		unsigned int i;

		for (i = 1; i < count && words[0] + 4 * i <= PVG_PAGE_SIZE; i++)
			pvg_display_state(pvg)[words[0] / 4 + i - 1] = cpu_to_le32(words[i]);
		ret = 0;
	} else if (!strcmp(verb, "mr") && count == 1 && words[0] < 0x10000 &&
		   pvg_mapper_regs(pvg)) {
		dev_info(&pvg->pdev->dev, "poke: mapper reg %#x = %#llx\n", words[0],
			 readq(pvg_mapper_regs(pvg) + (words[0] & ~7)));
		ret = 0;
	} else if (!strcmp(verb, "mw") && count == 2 && words[0] < 0x10000 &&
		   pvg_mapper_regs(pvg)) {
		/*
		 * 0x1008 is the ring's length, which must be a multiple of
		 * 1024, and 0x1018 the count written, which must not go down.
		 */
		if ((words[0] & ~7) != 0x1018 &&
		    ((words[0] & ~7) != 0x1008 || (words[1] && !(words[1] % 1024)))) {
			writeq(words[1], pvg_mapper_regs(pvg) + (words[0] & ~7));
			ret = 0;
		}
	} else if (!strcmp(verb, "burst") && count >= 1 &&
		   (count == 1 || words[1] <= PVG_COMMAND_MAX_WORDS)) {
		pvg_poke_burst(pvg, words[0], count > 1 ? words[1] : 0);
		ret = 0;
	} else if (!strcmp(verb, "flood") && count >= 1 &&
		   (count == 1 || (words[1] >= RUXNPVG_COMMAND_HEADER_BYTES && !(words[1] % 4) &&
				   words[1] <= RUXNPVG_RING_ROOM))) {
		pvg_poke_flood(pvg, words[0], count > 1 ? words[1] : RUXNPVG_RING_ROOM / 2 - 4);
		ret = 0;
	}

	kfree(line);
	return ret ? ret : length;
}

static const struct file_operations pvg_poke_fops = {
	.owner = THIS_MODULE,
	.open = simple_open,
	.write = pvg_poke_write,
	.llseek = noop_llseek,
};

/*
 * Fences for tests, which signal when a program says so. Each open file is
 * a timeline; it holds a reference on each fence it has not signalled.
 */
#define PVG_TEST_FENCE_NEW	_IOR('P', 1, __s32)

struct pvg_test_timeline {
	spinlock_t lock;
	u64 context;
	u64 seqno;
	struct list_head fences;
};

struct pvg_test_fence {
	struct dma_fence base;
	struct list_head link;
};

static const char *pvg_test_fence_name(struct dma_fence *fence)
{
	return "pvg-test";
}

static const struct dma_fence_ops pvg_test_fence_ops = {
	.get_driver_name = pvg_test_fence_name,
	.get_timeline_name = pvg_test_fence_name,
};

static void pvg_test_signal_all(struct pvg_test_timeline *timeline, int error)
{
	struct pvg_test_fence *fence, *next;
	LIST_HEAD(signalled);
	unsigned long flags;

	spin_lock_irqsave(&timeline->lock, flags);
	list_for_each_entry_safe(fence, next, &timeline->fences, link) {
		if (error)
			dma_fence_set_error(&fence->base, error);
		dma_fence_signal_locked(&fence->base);
		list_move(&fence->link, &signalled);
	}
	spin_unlock_irqrestore(&timeline->lock, flags);
	list_for_each_entry_safe(fence, next, &signalled, link)
		dma_fence_put(&fence->base);
}

static int pvg_test_fence_open(struct inode *inode, struct file *file)
{
	struct pvg_test_timeline *timeline = kzalloc(sizeof(*timeline), GFP_KERNEL);

	if (!timeline)
		return -ENOMEM;
	spin_lock_init(&timeline->lock);
	INIT_LIST_HEAD(&timeline->fences);
	timeline->context = dma_fence_context_alloc(1);
	file->private_data = timeline;
	return 0;
}

static int pvg_test_fence_release(struct inode *inode, struct file *file)
{
	struct pvg_test_timeline *timeline = file->private_data;

	pvg_test_signal_all(timeline, -ENOENT);
	kfree(timeline);
	return 0;
}

static ssize_t pvg_test_fence_write(struct file *file, const char __user *buf,
				    size_t length, loff_t *pos)
{
	pvg_test_signal_all(file->private_data, 0);
	return length;
}

static long pvg_test_fence_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
	struct pvg_test_timeline *timeline = file->private_data;
	struct pvg_test_fence *fence;
	struct sync_file *sync;
	unsigned long flags;
	int fd;

	if (cmd != PVG_TEST_FENCE_NEW)
		return -ENOTTY;
	fence = kzalloc(sizeof(*fence), GFP_KERNEL);
	if (!fence)
		return -ENOMEM;
	fd = get_unused_fd_flags(O_CLOEXEC);
	if (fd < 0) {
		kfree(fence);
		return fd;
	}
	spin_lock_irqsave(&timeline->lock, flags);
	dma_fence_init(&fence->base, &pvg_test_fence_ops, &timeline->lock,
		       timeline->context, ++timeline->seqno);
	list_add_tail(&fence->link, &timeline->fences);
	spin_unlock_irqrestore(&timeline->lock, flags);
	/* The list's reference is the one init gave; the sync_file takes its own. */
	sync = sync_file_create(&fence->base);
	if (!sync) {
		put_unused_fd(fd);
		return -ENOMEM;
	}
	if (put_user(fd, (__s32 __user *)arg)) {
		fput(sync->file);
		put_unused_fd(fd);
		return -EFAULT;
	}
	fd_install(fd, sync->file);
	return 0;
}

static const struct file_operations pvg_test_fence_fops = {
	.owner = THIS_MODULE,
	.open = pvg_test_fence_open,
	.release = pvg_test_fence_release,
	.write = pvg_test_fence_write,
	.unlocked_ioctl = pvg_test_fence_ioctl,
	.llseek = noop_llseek,
};

void pvg_debugfs_init(struct pvg_device *pvg)
{
	pvg->debugfs = debugfs_create_dir(pci_name(pvg->pdev), pvg_debugfs_root);
	debugfs_create_file("poke", 0200, pvg->debugfs, pvg, &pvg_poke_fops);
	debugfs_create_file("counts", 0400, pvg->debugfs, pvg, &pvg_counts_fops);
	debugfs_create_file("test_fence", 0600, pvg->debugfs, pvg, &pvg_test_fence_fops);
	debugfs_create_x32("irq_skip", 0600, pvg->debugfs, &pvg->irq_skip);
	debugfs_create_bool("irq_log", 0600, pvg->debugfs, &pvg->irq_log);
}

void pvg_debugfs_exit(struct pvg_device *pvg)
{
	debugfs_remove_recursive(pvg->debugfs);
}

void pvg_debugfs_register(void)
{
	pvg_debugfs_root = debugfs_create_dir(KBUILD_MODNAME, NULL);
}

void pvg_debugfs_unregister(void)
{
	debugfs_remove(pvg_debugfs_root);
}
