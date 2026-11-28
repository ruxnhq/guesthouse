// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The device's first command ring, ported from ruxnpvg's ring.c (itself a
// port of pvg_ring.c of the Linux driver) with KMDF taken out. Unchanged:
// the layout, the start sequence, the stamps, the rewind before the 32-bit
// counters wrap, the 20 ms look when an interrupt is late, the resume after
// a refused command.
//
// A command is three 32-bit words, its number, the length of the whole in
// bytes and a stamp, and then its payload. For each command that went
// through the device writes the command's stamp into the ring's first word
// and raises MSI message 0.
//
// Unlike ruxnpvg, the ring is not turned on at power-up: the RING_START
// escape turns it on, or the first task a program asks for (pvg_task.c),
// so that a boot that draws nothing writes nothing to the device.

#include "ruxnwddm.h"

#define RING_BYTES		0x10000
#define RING_COMMANDS_AT	0x1000
#define RING_SPACE		(RING_BYTES - RING_COMMANDS_AT)
#define COMMAND_HEADER		12
// The longest payload the ring takes, in words, as the Linux driver has it.
#define RING_MAX_WORDS		2048
#define PROTOCOL_VERSION	252
#define CMD_NOP			30

#define RING_TIMEOUT_MS		2000
#define RING_NOT_REFUSED	0xffffffffU
#define RING_RECHECK_MS		20
#define TICKS_PER_MS		10000LL

void ruxnwddm_ring_init(RUXNWDDM_ADAPTER *a)
{
	ruxnwddm_lock_init(&a->ring.lock);
	KeInitializeEvent(&a->ring.wake, SynchronizationEvent, FALSE);
}

// Only once the ring is off.
void ruxnwddm_ring_free(RUXNWDDM_ADAPTER *a)
{
	RUXNWDDM_RING *ring = &a->ring;

	ring->mem = NULL;
	ruxnwddm_free_pages(&ring->pages);
	ruxnwddm_free_pages(&ring->root);
}

// The writes that turn the ring on. The device then reads from position 0.
static void ring_on(RUXNWDDM_ADAPTER *a)
{
	RUXNWDDM_RING *ring = &a->ring;

	ruxnwddm_write(a, REG_ROOT_PAGE, ruxnwddm_page_number(&ring->root));
	ruxnwddm_write(a, REG_RING_PAGE, ruxnwddm_page_number(&ring->pages));
	ruxnwddm_write(a, REG_RING_BYTES, RING_BYTES);
	ruxnwddm_write(a, REG_RING_COMMANDS, RING_COMMANDS_AT);
	ruxnwddm_write(a, REG_RING_ON, 1);
	ring->written = 0;
}

// At PASSIVE_LEVEL, with the device started and powered.
NTSTATUS ruxnwddm_ring_start(RUXNWDDM_ADAPTER *a)
{
	RUXNWDDM_RING *ring = &a->ring;
	NTSTATUS status = STATUS_SUCCESS;

	if (!a->regs || a->adapter_power != PowerDeviceD0)
		return STATUS_DEVICE_NOT_READY;
	ruxnwddm_lock(&ring->lock);
	if (ring->on)
		goto out;
	if (!ring->mem) {
		status = ruxnwddm_alloc_pages(&ring->root, RUXNWDDM_PAGE_SIZE);
		if (NT_SUCCESS(status))
			status = ruxnwddm_alloc_pages(&ring->pages, RING_BYTES);
		if (!NT_SUCCESS(status)) {
			ruxnwddm_ring_free(a);
			goto out;
		}
		ring->mem = ring->pages.va;
	}

	// A driver before this one may have left the ring on.
	ruxnwddm_write(a, REG_RING_ON, 0);
	ruxnwddm_write(a, REG_VERSION, PROTOCOL_VERSION);
	ring->version = ruxnwddm_read(a, REG_VERSION);
	if (ring->version != PROTOCOL_VERSION) {
		status = STATUS_DEVICE_PROTOCOL_ERROR;
		goto out;
	}
	RtlZeroMemory(ring->root.va, ring->root.bytes);
	RtlZeroMemory(ring->pages.va, ring->pages.bytes);
	ring->waited = 0;
	ring->last_stamp = 0;
	ring->rewind_at = MAXULONG - RING_SPACE;
	InterlockedExchange(&ring->irq_stamp, 0);
	InterlockedExchange(&ring->faulted, 0);
	ring_on(a);
	ring->on = TRUE;
out:
	ruxnwddm_unlock(&ring->lock);
	ruxnwddm_record_ulong(L"RingStartStatus", (ULONG)status);
	return status;
}

void ruxnwddm_ring_stop(RUXNWDDM_ADAPTER *a)
{
	RUXNWDDM_RING *ring = &a->ring;

	ruxnwddm_lock(&ring->lock);
	if (ring->on && a->regs) {
		ruxnwddm_write(a, REG_RING_ON, 0);
		ring->on = FALSE;
	}
	ruxnwddm_unlock(&ring->lock);
}

// From the interrupt routine, at its IRQL.
void ruxnwddm_ring_irq(RUXNWDDM_ADAPTER *a, BOOLEAN fault)
{
	RUXNWDDM_RING *ring = &a->ring;

	if (ring->mem)
		InterlockedExchange(&ring->irq_stamp, (LONG)ring->mem[0]);
	if (fault)
		InterlockedExchange(&ring->faulted, 1);
}

static BOOLEAN stamp_reached(ULONG seen, ULONG stamp)
{
	return (LONG)(seen - stamp) >= 0;
}

// Writes one command behind the last. The caller holds the ring's lock.
static NTSTATUS ring_queue(RUXNWDDM_ADAPTER *a, ULONG number, const ULONG *payload, ULONG words,
			   ULONG *stamp)
{
	RUXNWDDM_RING *ring = &a->ring;
	volatile ULONG *commands = ring->mem + RING_COMMANDS_AT / 4;
	const ULONG ring_words = RING_SPACE / 4;
	ULONG length = COMMAND_HEADER + 4 * words;
	ULONG at = (ring->written % RING_SPACE) / 4;
	ULONG i;

	if (words > RING_MAX_WORDS)
		return STATUS_INVALID_PARAMETER;

	// Before the 32-bit counter would wrap, start afresh at a moment when
	// the device has read all there is.
	if (ring->written >= ring->rewind_at && ring->written == ring->waited) {
		ruxnwddm_write(a, REG_RING_ON, 0);
		ring_on(a);
		ring->waited = 0;
		ring->rewinds++;
		at = 0;
	}
	// Nothing the device has not read may be written over.
	if (ring->written - ring->waited + length > RING_SPACE) {
		ring->waited = ruxnwddm_read(a, REG_RING_READ);
		if (ring->written - ring->waited + length > RING_SPACE)
			return STATUS_DEVICE_BUSY;
	}
	*stamp = ++ring->last_stamp;
	commands[at] = number;
	commands[(at + 1) % ring_words] = length;
	commands[(at + 2) % ring_words] = *stamp;
	// A command may lie across the ring's end, so each word wraps alone.
	for (i = 0; i < words; i++)
		commands[(at + 3 + i) % ring_words] = payload[i];
	ring->written += length;
	ring->sent++;
	return STATUS_SUCCESS;
}

static void ring_kick(RUXNWDDM_ADAPTER *a)
{
	InterlockedExchange(&a->ring.faulted, 0);
	KeMemoryBarrier();
	ruxnwddm_write(a, REG_RING_WRITTEN, a->ring.written);
}

static BOOLEAN wait_woken(RUXNWDDM_RING *ring, ULONG stamp)
{
	LONGLONG deadline = (LONGLONG)KeQueryInterruptTime() + RING_RECHECK_MS * TICKS_PER_MS;
	LARGE_INTEGER timeout;

	for (;;) {
		LONGLONG left;

		if (stamp_reached((ULONG)ring->irq_stamp, stamp) || ring->faulted)
			return TRUE;
		left = deadline - (LONGLONG)KeQueryInterruptTime();
		if (left <= 0)
			return FALSE;
		timeout.QuadPart = -left;
		KeWaitForSingleObject(&ring->wake, Executive, KernelMode, FALSE, &timeout);
	}
}

// Waits until the device has stamped the command, as ruxnpvg's
// ring_wait_for: success, STATUS_IO_TIMEOUT, or STATUS_DEVICE_DATA_ERROR if
// the device refused a command on the way.
static NTSTATUS ring_wait_for(RUXNWDDM_ADAPTER *a, ULONG stamp, ULONG timeout_ms, BOOLEAN *late)
{
	RUXNWDDM_RING *ring = &a->ring;
	LONGLONG deadline = (LONGLONG)KeQueryInterruptTime() + (LONGLONG)timeout_ms * TICKS_PER_MS;
	ULONG written = ring->written;
	BOOLEAN refused = FALSE;

	*late = FALSE;
	for (;;) {
		BOOLEAN woken = wait_woken(ring, stamp);

		if (ring->faulted || (!woken && ruxnwddm_read(a, REG_FAULT))) {
			ULONG at = ruxnwddm_read(a, REG_RING_REFUSED);

			InterlockedExchange(&ring->faulted, 0);
			if (at != RING_NOT_REFUSED) {
				ruxnwddm_write(a, REG_RING_RESUME, 0);
				ring->refused++;
				refused = TRUE;
				continue;
			}
			if (stamp_reached(ring->mem[0], stamp))
				break;
			if ((LONGLONG)KeQueryInterruptTime() > deadline)
				return STATUS_IO_TIMEOUT;
			continue;
		}
		if (woken)
			break;
		if (stamp_reached(ring->mem[0], stamp)) {
			ring->late++;
			*late = TRUE;
			break;
		}
		if ((LONGLONG)KeQueryInterruptTime() > deadline)
			return STATUS_IO_TIMEOUT;
	}
	if (stamp == ring->last_stamp)
		ring->waited = written;
	return refused ? STATUS_DEVICE_DATA_ERROR : STATUS_SUCCESS;
}

// Sends one command and waits until the device has stamped it, for at most
// timeout_ms: a command that hands the GPU work is stamped when the work is
// done. At PASSIVE_LEVEL.
static NTSTATUS ring_send(RUXNWDDM_ADAPTER *a, ULONG number, const ULONG *payload, ULONG words,
			  ULONG timeout_ms, BOOLEAN *late)
{
	RUXNWDDM_RING *ring = &a->ring;
	NTSTATUS status;
	ULONG stamp;

	*late = FALSE;
	ruxnwddm_lock(&ring->lock);
	if (!ring->on) {
		status = STATUS_DEVICE_NOT_READY;
	} else {
		status = ring_queue(a, number, payload, words, &stamp);
		if (NT_SUCCESS(status)) {
			ring_kick(a);
			status = ring_wait_for(a, stamp, timeout_ms, late);
		}
	}
	ruxnwddm_unlock(&ring->lock);
	return status;
}

// Sends one command 30, which goes through and has no effect, and waits
// for its stamp. At PASSIVE_LEVEL.
NTSTATUS ruxnwddm_ring_nop(RUXNWDDM_ADAPTER *a, BOOLEAN *late)
{
	return ring_send(a, CMD_NOP, NULL, 0, RING_TIMEOUT_MS, late);
}

// Sends a command of a task or of the display. STATUS_DEVICE_DATA_ERROR
// says the device refused it, as -EIO does in the Linux driver. 0 for
// timeout_ms is the ring's usual wait. At PASSIVE_LEVEL.
NTSTATUS ruxnwddm_ring_send(RUXNWDDM_ADAPTER *a, ULONG number, const ULONG *payload, ULONG words,
			    ULONG timeout_ms)
{
	BOOLEAN late;
	NTSTATUS status = ring_send(a, number, payload, words,
				    timeout_ms ? timeout_ms : RING_TIMEOUT_MS, &late);

	// The command, its first two words (a task's id and an object's, or a
	// submit's resource count) and the answer, in order (klog.c).
	ruxnwddm_klog_event("ring", number, words ? payload[0] : 0, words > 1 ? payload[1] : 0, status);
	return status;
}
