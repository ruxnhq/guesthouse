// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The device's MSI messages, as the graphics kernel delivers them. The
// handling is ruxnpvg's interrupt.c: message 0 is the graphics device's,
// and three registers say what it was for (the done bits and the display
// events clear when read, the fault stays until the ring is resumed);
// message 1 is the mapper's and reads nothing. An MSI message is never
// shared, so every one is claimed.
//
// In milestone 1 nothing is expected: the ring is off unless the RING_START
// escape turned it on, and the display events the early framebuffer raises
// are only counted.

#include "pvg.h"

BOOLEAN ruxnwddm_interrupt(RUXNWDDM_ADAPTER *a, ULONG message)
{
	RUXNWDDM_IRQ_COUNTS *counts = &a->irq;
	ULONG done, events, fault;

	if (message == RUXNWDDM_MSG_MAPPER) {
		InterlockedIncrement64(&counts->other);
		if (a->pvg) {
			pvg_interrupt(a->pvg, message, 0);
			a->dxgk.DxgkCbQueueDpc(a->dxgk.DeviceHandle);
		}
		return TRUE;
	}
	if (!a->regs)
		return TRUE;

	done = ruxnwddm_read(a, REG_DONE);
	events = ruxnwddm_read(a, REG_DISPLAY_EVENTS);
	fault = ruxnwddm_read(a, REG_FAULT);

	InterlockedIncrement64(&counts->taken);
	if (done)
		InterlockedIncrement64(&counts->done);
	if (events)
		InterlockedIncrement64(&counts->events);
	if (fault)
		InterlockedIncrement64(&counts->faults);
	if (!done && !events && !fault)
		InterlockedIncrement64(&counts->empty);

	ruxnwddm_ring_irq(a, fault != 0);
	if (a->pvg)
		pvg_interrupt(a->pvg, message, events);
	a->dxgk.DxgkCbQueueDpc(a->dxgk.DeviceHandle);
	return TRUE;
}

// Wakes a sender waiting on the ring; the interrupt latched what it saw.
// The full driver must also hand the graphics kernel the notifications it
// raised (fences done, vertical blanks): until DxgkCbNotifyDpc they are
// not acted on.
void ruxnwddm_dpc(RUXNWDDM_ADAPTER *a)
{
	KeSetEvent(&a->ring.wake, IO_NO_INCREMENT, FALSE);
	if (a->pvg)
		pvg_dpc(a->pvg);
	if (a->full)
		a->dxgk.DxgkCbNotifyDpc(a->dxgk.DeviceHandle);
}
