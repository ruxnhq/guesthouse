// SPDX-License-Identifier: GPL-2.0
/*
 * The device's first command ring.
 *
 * A command is three 32-bit words, its number, the length of the whole in
 * bytes and a stamp, and then its payload. The guest writes commands one
 * behind the other and tells the device how far it has written. For each
 * command that went through the device writes the command's stamp into the
 * ring's first word and raises the interrupt.
 */

#include <linux/delay.h>
#include <linux/jiffies.h>
#include <linux/ktime.h>
#include <linux/module.h>

#include "pvg.h"

static bool ring_log;
module_param(ring_log, bool, 0644);
MODULE_PARM_DESC(ring_log, "Log every command the driver writes into the ring, with its payload");

#define PVG_RING_TIMEOUT	msecs_to_jiffies(RUXNPVG_RING_TIMEOUT_MS)

/*
 * How often a sender that sleeps on a stamp looks at the stamp cell on its
 * own, in microseconds. An interrupt can come late: under QEMU with HVF's
 * interrupt controller about one in a thousand is only delivered together
 * with the next, and then the look is what finds the stamp. The sleep is on
 * a high-resolution timer, so it costs a wake-up a look, and only while a
 * sender waits. Late interrupts come in bursts, where nearly every one is
 * late: then a waited submit took the look's 20 ms, and with 1 ms it takes
 * about one. A child ring's waiters look as often; its work item, which runs
 * with no waiter, every 20 ms.
 */
unsigned int pvg_recheck_us = 1000;
module_param_named(recheck_us, pvg_recheck_us, uint, 0644);
MODULE_PARM_DESC(recheck_us, "How often a sleeping sender looks for its stamp itself, in microseconds (1000)");

ktime_t pvg_recheck(void)
{
	return us_to_ktime(clamp(READ_ONCE(pvg_recheck_us), 100U, 100000U));
}

/* For the other senders: says a command in the kernel's log, if that is asked for. */
void pvg_ring_log(struct pvg_device *pvg, unsigned int ring, u32 number, u32 stamp,
		  const u32 *payload, unsigned int words)
{
	if (ring_log)
		dev_info(&pvg->pdev->dev,
			 "ring %u: command %u, stamp %#x, %u words: %*ph\n", ring,
			 number, stamp, words, (int)min(4 * words, 64U), payload);
}

/*
 * The writes that turn the ring on. The device then reads from position 0.
 * Turning the ring off puts both positions back there, and makes the device
 * forget its tasks, child rings and the display's state page, so it is done
 * only at the driver's start. The positions count on past 2^32 instead: the
 * ring's room divides 2^32, and the device goes on there.
 */
static void pvg_ring_on(struct pvg_device *pvg)
{
	struct pvg_ring *ring = &pvg->ring;

	/* The root page has to be named before the ring is turned on. */
	pvg_write(pvg, RUXNPVG_REG_ROOT_PAGE, pvg_page_number(ring->root_page));
	pvg_write(pvg, RUXNPVG_REG_RING_PAGE, pvg_page_number(ring->pages));
	pvg_write(pvg, RUXNPVG_REG_RING_BYTES, RUXNPVG_RING_BYTES);
	pvg_write(pvg, RUXNPVG_REG_RING_COMMANDS, RUXNPVG_RING_COMMANDS_AT);
	pvg_write(pvg, RUXNPVG_REG_RING_ON, 1);
	ring->written = 0;
}

int pvg_ring_start(struct pvg_device *pvg)
{
	struct pvg_ring *ring = &pvg->ring;
	u32 version;

	mutex_init(&ring->lock);
	init_waitqueue_head(&ring->wait);
	spin_lock_init(&ring->irq_lock);

	ring->root_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	ring->pages = pvg_alloc_pages(pvg, RUXNPVG_RING_BYTES);
	if (!ring->root_page || !ring->pages) {
		pvg_ring_free(pvg);
		return -ENOMEM;
	}
	ring->root = page_address(ring->root_page);
	ring->mem = page_address(ring->pages);
	ring->last_stamp = 0;

	/*
	 * A driver before this one may have left the ring on. Off first, so
	 * that the device forgets where that ring was.
	 */
	pvg_write(pvg, RUXNPVG_REG_RING_ON, 0);

	pvg_write(pvg, RUXNPVG_REG_VERSION, RUXNPVG_VERSION);
	version = pvg_read(pvg, RUXNPVG_REG_VERSION);
	if (version != RUXNPVG_VERSION) {
		dev_err(&pvg->pdev->dev, "version %u asked for, the device says %#x\n",
			RUXNPVG_VERSION, version);
		pvg_ring_free(pvg);
		return -ENODEV;
	}

	pvg_ring_on(pvg);
	return 0;
}

void pvg_ring_stop(struct pvg_device *pvg)
{
	pvg_write(pvg, RUXNPVG_REG_RING_ON, 0);
}

void pvg_ring_free(struct pvg_device *pvg)
{
	struct pvg_ring *ring = &pvg->ring;

	/*
	 * With tasks left on the device for a test (reload.diff), the device
	 * may still note them in this root page: it and the ring stay.
	 */
	if (READ_ONCE(pvg_test_leave)) {
		ring->pages = NULL;
		ring->root_page = NULL;
		return;
	}
	pvg_free_pages(ring->pages, RUXNPVG_RING_BYTES);
	pvg_free_pages(ring->root_page, PVG_PAGE_SIZE);
	ring->pages = NULL;
	ring->root_page = NULL;
}

/* Called from the interrupt handler with what the fault register read. */
void pvg_ring_irq(struct pvg_device *pvg, bool fault)
{
	struct pvg_ring *ring = &pvg->ring;
	unsigned long flags;

	spin_lock_irqsave(&ring->irq_lock, flags);
	ring->irq_stamp = le32_to_cpu(READ_ONCE(ring->mem[0]));
	if (fault)
		ring->faulted = true;
	spin_unlock_irqrestore(&ring->irq_lock, flags);
	wake_up(&ring->wait);
}

/*
 * The stamp cell holds the stamp of the last command that went through, and
 * stamps go up, so a command is done once the cell holds its stamp or a
 * later one.
 */
static bool pvg_stamp_reached(u32 seen, u32 stamp)
{
	return (s32)(seen - stamp) >= 0;
}

static bool pvg_ring_stamped(struct pvg_ring *ring, u32 stamp)
{
	return pvg_stamp_reached(le32_to_cpu(READ_ONCE(ring->mem[0])), stamp);
}

/*
 * Writes one command behind the last without telling the device.
 * The caller holds the ring's lock.
 */
int pvg_ring_queue(struct pvg_device *pvg, u32 number, const u32 *payload,
		   unsigned int words, u32 *stamp)
{
	struct pvg_ring *ring = &pvg->ring;
	__le32 *commands = (void *)ring->mem + RUXNPVG_RING_COMMANDS_AT;
	u32 length = RUXNPVG_COMMAND_HEADER_BYTES + 4 * words;
	u32 at = (ring->written % RUXNPVG_RING_ROOM) / 4;
	unsigned int i;

	lockdep_assert_held(&ring->lock);
	if (words > (RUXNPVG_RING_ROOM - RUXNPVG_COMMAND_HEADER_BYTES) / 4)
		return -EINVAL;

	/*
	 * Nothing the device has not read may be written over. The device's
	 * own read position is asked only when the driver's count says full.
	 */
	if (ring->written - ring->waited + length > RUXNPVG_RING_ROOM) {
		ring->waited = pvg_read(pvg, RUXNPVG_REG_RING_READ);
		if (ring->written - ring->waited + length > RUXNPVG_RING_ROOM) {
			atomic64_inc(&pvg->ring_full);
			return -ENOSPC;
		}
	}

	*stamp = ++ring->last_stamp;
	pvg_ring_log(pvg, 0, number, *stamp, payload, words);

	/* A command may lie across the ring's end, so each word wraps alone. */
	commands[at] = cpu_to_le32(number);
	commands[(at + 1) % (RUXNPVG_RING_ROOM / 4)] = cpu_to_le32(length);
	commands[(at + 2) % (RUXNPVG_RING_ROOM / 4)] = cpu_to_le32(*stamp);
	for (i = 0; i < words; i++)
		commands[(at + 3 + i) % (RUXNPVG_RING_ROOM / 4)] = cpu_to_le32(payload[i]);
	ring->written += length;
	return 0;
}

/* Tells the device how far the guest has written. */
void pvg_ring_kick(struct pvg_device *pvg)
{
	struct pvg_ring *ring = &pvg->ring;

	lockdep_assert_held(&ring->lock);
	spin_lock_irq(&ring->irq_lock);
	ring->faulted = false;
	spin_unlock_irq(&ring->irq_lock);
	pvg_write(pvg, RUXNPVG_REG_RING_WRITTEN, ring->written);
}

/*
 * Sleeps until the device has stamped the command with this stamp.
 * Returns 0, -ETIMEDOUT if no stamp came, or -EIO if the device refused a
 * command on the way.
 */
static int pvg_ring_wait_for(struct pvg_device *pvg, u32 stamp,
			     unsigned long timeout)
{
	struct pvg_ring *ring = &pvg->ring;
	unsigned long deadline = jiffies + timeout;
	u32 written = ring->written;
	bool refused = false;

	lockdep_assert_held(&ring->lock);
	/*
	 * The handler wakes this up. If nothing woke it for a while it looks
	 * at the stamp cell and the fault register itself, which are right
	 * whether or not the interrupt arrived.
	 */
	for (;;) {
		bool woken = !wait_event_hrtimeout(ring->wait,
				pvg_stamp_reached(READ_ONCE(ring->irq_stamp), stamp) ||
				READ_ONCE(ring->faulted),
				pvg_recheck());

		if (READ_ONCE(ring->faulted) ||
		    (!woken && pvg_read(pvg, RUXNPVG_REG_FAULT))) {
			u32 at = pvg_read(pvg, RUXNPVG_REG_RING_REFUSED);

			spin_lock_irq(&ring->irq_lock);
			ring->faulted = false;
			spin_unlock_irq(&ring->irq_lock);
			/*
			 * The fault register is up while any ring has a
			 * refused command. This ring has one if its place is
			 * named; a child ring's is its own sender's to resume.
			 */
			if (at != RUXNPVG_NOT_REFUSED) {
				/*
				 * The device read on past the command it
				 * refused and holds all later stamps back. A
				 * resume makes it hand over the last of them,
				 * or the refused command's own if none came
				 * after, with an interrupt.
				 */
				dev_err(&pvg->pdev->dev,
					"the device refused the command at %#x of the ring\n",
					at);
				pvg_write(pvg, RUXNPVG_REG_RING_RESUME, 0);
				refused = true;
				continue;
			}
			if (pvg_ring_stamped(ring, stamp))
				break;
			if (time_after(jiffies, deadline))
				return -ETIMEDOUT;
			continue;
		}
		if (woken)
			break;
		if (pvg_ring_stamped(ring, stamp)) {
			ring->late++;
			dev_warn_ratelimited(&pvg->pdev->dev,
					     "stamp %#x is there and no interrupt came for it\n",
					     stamp);
			break;
		}
		if (time_after(jiffies, deadline))
			return -ETIMEDOUT;
	}
	if (stamp == ring->last_stamp)
		ring->waited = written;
	return refused ? -EIO : 0;
}

int pvg_ring_wait(struct pvg_device *pvg, u32 stamp)
{
	return pvg_ring_wait_for(pvg, stamp, PVG_RING_TIMEOUT);
}

/*
 * Looks at the stamp cell for up to USECS microseconds, for the last command
 * queued, and takes it as done if the stamp is there and no command was
 * refused. Returns 0 then and -EAGAIN otherwise, for the caller to wait as
 * usual. For senders of many commands, the debugfs flood: an interrupt
 * that comes late costs a sleeper the 20 ms of its next look.
 */
int pvg_ring_poll(struct pvg_device *pvg, unsigned int usecs)
{
	struct pvg_ring *ring = &pvg->ring;
	ktime_t until = ktime_add_us(ktime_get(), usecs);

	lockdep_assert_held(&ring->lock);
	do {
		if (pvg_ring_stamped(ring, ring->last_stamp)) {
			if (READ_ONCE(ring->faulted) || pvg_read(pvg, RUXNPVG_REG_FAULT))
				return -EAGAIN;
			ring->waited = ring->written;
			return 0;
		}
		udelay(5);
	} while (ktime_before(ktime_get(), until));
	return -EAGAIN;
}

/*
 * Sends one command and sleeps until the device has stamped it, for as long
 * as the caller says: a command that hands the GPU work is stamped when the
 * work is done.
 */
int pvg_ring_send_slow(struct pvg_device *pvg, u32 number, const u32 *payload,
		       unsigned int words, unsigned long timeout)
{
	struct pvg_ring *ring = &pvg->ring;
	u32 stamp;
	int ret;

	mutex_lock(&ring->lock);
	ret = pvg_ring_queue(pvg, number, payload, words, &stamp);
	if (!ret) {
		pvg_ring_kick(pvg);
		ret = pvg_ring_wait_for(pvg, stamp, timeout);
	}
	mutex_unlock(&ring->lock);
	return ret;
}

/* Sends one command and sleeps until the device has stamped it. */
int pvg_ring_send(struct pvg_device *pvg, u32 number, const u32 *payload,
		  unsigned int words)
{
	return pvg_ring_send_slow(pvg, number, payload, words, PVG_RING_TIMEOUT);
}
