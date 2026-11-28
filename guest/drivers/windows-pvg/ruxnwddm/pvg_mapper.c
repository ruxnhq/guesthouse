// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The IOSurface mapper's device, ported from pvg_surface.c of the Linux
// driver at 377700a.
//
// The mapper is a second device behind the same PCI function: its registers
// are BAR 2, all 64 bits wide, and its interrupt is MSI message 1. It gives
// a surface, a list of device pages anywhere in guest memory, a mapping id
// that display commands then name. Its ring is its own: a command is 16
// bytes, the command, a mapping id and a word the device writes the outcome
// to.
//
// The registers written here and nothing else: the ring's address, its
// length, the table's address (once a start, before the first command) and
// the written count, which is the kick. Read: the written and done counts.
// The device keeps both counts across drivers and QEMU does not reset the
// mapper, so a start goes on from where the last driver stopped: a smaller
// count must never reach it.
//
// Unlike Linux, which starts the mapper with its display at probe, this
// driver starts it when a surface is first mapped, so that a driver that
// shows nothing writes nothing to BAR 2.

#include "pvg.h"

#define MAPPER_REG_RING		0x1000
#define MAPPER_REG_SLOTS	0x1008	// the ring's length in commands
#define MAPPER_REG_TABLE	0x1010
#define MAPPER_REG_WRITTEN	0x1018	// writing it is the kick
#define MAPPER_REG_DONE		0x1020

// The ring is one device page. The length the device is told must be a
// multiple of 1024 commands: the host's process ends on another value. So
// this is a constant, and nothing else is ever written to that register.
#define MAPPER_COMMAND_BYTES	16
#define MAPPER_SLOTS		(RUXNWDDM_PAGE_SIZE / MAPPER_COMMAND_BYTES)
#define MAPPER_COMMAND_WORDS	(MAPPER_COMMAND_BYTES / 4)
// What the outcome word holds until the device has written it.
#define MAPPER_OUTCOME_UNSET	0xffffffffU

#define MAPPER_TIMEOUT_MS	2000
// Message 1 can be late, so a waiting sender looks at the done count
// itself this often, as the ring's sender looks at its stamp cell.
#define MAPPER_RECHECK_MS	20
#define TICKS_PER_MS		10000LL

// How the mapper names a device page wherever it wants one: the page's
// number shifted left by two, with bit 0 for valid.
static ULONG map_entry(const RUXNWDDM_PAGES *page)
{
	return ruxnwddm_page_number(page) << 2 | 1;
}

void pvg_mapper_init(RUXNWDDM_PVG *pvg)
{
	struct pvg_mapper *mapper = pvg->mapper;

	KeInitializeEvent(&mapper->wake, SynchronizationEvent, FALSE);
	InitializeListHead(&mapper->mapped);
	InitializeListHead(&mapper->held);
	InitializeListHead(&mapper->exported);
	ruxnwddm_lock_init(&mapper->lock);
}

// Tells the device of the ring and the table, once a start. The caller
// holds the mapper's lock.
NTSTATUS pvg_mapper_start(RUXNWDDM_PVG *pvg)
{
	struct pvg_mapper *mapper = pvg->mapper;
	ULONG64 written, done;
	NTSTATUS status;
	ULONG i;

	if (mapper->started)
		return STATUS_SUCCESS;
	if (!pvg->mapper_regs || !InterlockedCompareExchange(&pvg->task_gate, 0, 0))
		return STATUS_DEVICE_NOT_READY;

	// Pages kept from before a power-down are used again.
	status = STATUS_SUCCESS;
	if (!mapper->ring_page.allocation)
		status = ruxnwddm_alloc_mapper_pages(&mapper->ring_page, RUXNWDDM_PAGE_SIZE);
	if (NT_SUCCESS(status) && !mapper->table_page.allocation)
		status = ruxnwddm_alloc_mapper_pages(&mapper->table_page, RUXNWDDM_PAGE_SIZE);
	if (!NT_SUCCESS(status)) {
		ruxnwddm_free_pages(&mapper->ring_page);
		ruxnwddm_free_pages(&mapper->table_page);
		return status;
	}

	// Nothing of a device before a power-down is to be read again.
	RtlZeroMemory(mapper->ring_page.va, RUXNWDDM_PAGE_SIZE);
	RtlZeroMemory(mapper->table_page.va, RUXNWDDM_PAGE_SIZE);
	// Descriptor pages kept from before are named in the table again
	// (run rn23b: without this the guard found no descriptor for a map).
	for (i = 0; i < MAPPER_DESCRIPTOR_PAGES; i++)
		if (mapper->descriptor_pages[i].va)
			((volatile ULONG *)mapper->table_page.va)[i] = map_entry(&mapper->descriptor_pages[i]);
	written = pvg_mapper_read(pvg, MAPPER_REG_WRITTEN);
	done = pvg_mapper_read(pvg, MAPPER_REG_DONE);
	mapper->written = max(written, done);
	ruxnwddm_record_ulong64(L"MapperWrittenBefore", written);
	ruxnwddm_record_ulong64(L"MapperDoneBefore", done);

	pvg_mapper_write(pvg, MAPPER_REG_RING, (ULONG64)mapper->ring_page.phys.QuadPart);
	pvg_mapper_write(pvg, MAPPER_REG_SLOTS, MAPPER_SLOTS);
	pvg_mapper_write(pvg, MAPPER_REG_TABLE, (ULONG64)mapper->table_page.phys.QuadPart);
	mapper->started = TRUE;
	InterlockedIncrement64(&pvg->screen.mapper_started);
	return STATUS_SUCCESS;
}

static BOOLEAN mapper_done(RUXNWDDM_PVG *pvg)
{
	return (LONG64)(pvg_mapper_read(pvg, MAPPER_REG_DONE) - pvg->mapper->written) >= 0;
}

// Puts one command into the mapper's ring and waits until the device has
// done it. The outcome is 0 or an error of the host's. The caller holds the
// mapper's lock, and the mapper is started. At PASSIVE_LEVEL.
NTSTATUS pvg_mapper_command(RUXNWDDM_PVG *pvg, ULONG command, ULONG id, ULONG *outcome)
{
	struct pvg_mapper *mapper = pvg->mapper;
	volatile ULONG *slot;
	LONGLONG deadline;
	LARGE_INTEGER timeout;

	*outcome = MAPPER_OUTCOME_UNSET;
	if (!mapper->started || mapper->broken)
		return STATUS_DEVICE_NOT_READY;

	slot = (volatile ULONG *)mapper->ring_page.va +
	       (mapper->written % MAPPER_SLOTS) * MAPPER_COMMAND_WORDS;
	slot[0] = command;
	slot[1] = id;
	slot[2] = MAPPER_OUTCOME_UNSET;
	slot[3] = 0;
	KeClearEvent(&mapper->wake);
	KeMemoryBarrier();
	// The count only ever goes up by one from what the device was told.
	mapper->written++;
	InterlockedIncrement64(&pvg->screen.mapper_commands);
	pvg_mapper_write(pvg, MAPPER_REG_WRITTEN, mapper->written);

	if (mapper_done(pvg)) {
		InterlockedIncrement64(&pvg->screen.mapper_early);
	} else {
		deadline = (LONGLONG)KeQueryInterruptTime() + MAPPER_TIMEOUT_MS * TICKS_PER_MS;
		for (;;) {
			NTSTATUS waited;

			timeout.QuadPart = -MAPPER_RECHECK_MS * TICKS_PER_MS;
			waited = KeWaitForSingleObject(&mapper->wake, Executive, KernelMode, FALSE,
						       &timeout);
			if (mapper_done(pvg)) {
				if (waited == STATUS_TIMEOUT)
					InterlockedIncrement64(&pvg->screen.mapper_late);
				break;
			}
			if ((LONGLONG)KeQueryInterruptTime() > deadline) {
				// Nothing more goes to a mapper that did not answer.
				mapper->broken = TRUE;
				InterlockedIncrement64(&pvg->screen.mapper_timeouts);
				ruxnwddm_record_ulong(L"MapperTimeoutCommand", command);
				ruxnwddm_record_ulong(L"MapperTimeoutId", id);
				return STATUS_IO_TIMEOUT;
			}
		}
	}
	KeMemoryBarrier();
	*outcome = slot[2];
	ruxnwddm_klog_event("mapper", command, id, *outcome, STATUS_SUCCESS);
	return STATUS_SUCCESS;
}

// Where an id's descriptor is; its page is made when its first id is used.
// The caller holds the mapper's lock.
volatile ULONG *pvg_mapper_descriptor(RUXNWDDM_PVG *pvg, ULONG id)
{
	struct pvg_mapper *mapper = pvg->mapper;
	ULONG index = id / MAPPER_IDS_PER_PAGE;
	RUXNWDDM_PAGES *page = &mapper->descriptor_pages[index];

	if (!page->va) {
		if (!NT_SUCCESS(ruxnwddm_alloc_mapper_pages(page, RUXNWDDM_PAGE_SIZE)))
			return NULL;
		((volatile ULONG *)mapper->table_page.va)[index] = map_entry(page);
	}
	return (volatile ULONG *)((PUCHAR)page->va + (id % MAPPER_IDS_PER_PAGE) * MAPPER_DESCRIPTOR);
}

// From the DPC after message 1: one interrupt for each command done, nothing to
// read or clear.
void pvg_mapper_irq(RUXNWDDM_PVG *pvg)
{
	KeSetEvent(&pvg->mapper->wake, IO_NO_INCREMENT, FALSE);
}

// Lets go of every mapping the device made: held surfaces are forgotten,
// mapped ones lose their ids and are mapped again when next named, and
// the mapper counts as not started, so its next command first tells the
// device of the ring and the table. The caller holds the mapper's lock.
static void forget_mappings(RUXNWDDM_PVG *pvg)
{
	struct pvg_mapper *mapper = pvg->mapper;

	while (!IsListEmpty(&mapper->held)) {
		struct pvg_surface *surface =
			CONTAINING_RECORD(mapper->held.Flink, struct pvg_surface, link);

		RemoveEntryList(&surface->link);
		InterlockedDecrement64(&pvg->screen.held_now);
		InterlockedDecrement64(&pvg->screen.mapped_now);
		pvg_surface_forget(pvg, surface);
	}
	while (!IsListEmpty(&mapper->mapped)) {
		struct pvg_surface *surface =
			CONTAINING_RECORD(mapper->mapped.Flink, struct pvg_surface, link);

		// Still its handle's; the handle's end keeps its memory.
		RemoveEntryList(&surface->link);
		InitializeListHead(&surface->link);
		InterlockedDecrement64(&pvg->screen.mapped_now);
		surface->id = 0;
		surface->kept = TRUE;
	}
	RtlZeroMemory(mapper->ids, sizeof(mapper->ids));
	mapper->started = FALSE;
	mapper->broken = FALSE;
}

// At power-down: the device that comes back may be a new one with no
// ring, table or mappings (run app3: Windows' fast startup hibernated the
// session, the next boot resumed it on a new device, and the first mapper
// command kicked a mapper that had never been told of its ring; the guard
// refused it and the screen stayed black). The pages are kept for the next
// start.
void pvg_mapper_power_down(RUXNWDDM_PVG *pvg)
{
	struct pvg_mapper *mapper = pvg->mapper;

	ruxnwddm_lock(&mapper->lock);
	if (mapper->started)
		InterlockedIncrement64(&pvg->screen.mapper_power_downs);
	forget_mappings(pvg);
	ruxnwddm_unlock(&mapper->lock);
}

// At release hardware, once the display and every surface have let go.
// The device keeps the ring's and the table's addresses; it reads neither
// before the next kick, and the next start names new ones. A surface the
// device would not let go keeps its memory, since the device may read it.
void pvg_mapper_release(RUXNWDDM_PVG *pvg)
{
	struct pvg_mapper *mapper = pvg->mapper;
	ULONG i;

	ruxnwddm_lock(&mapper->lock);
	forget_mappings(pvg);
	for (i = 0; i < MAPPER_DESCRIPTOR_PAGES; i++)
		ruxnwddm_free_pages(&mapper->descriptor_pages[i]);
	ruxnwddm_free_pages(&mapper->ring_page);
	ruxnwddm_free_pages(&mapper->table_page);
	ruxnwddm_unlock(&mapper->lock);
}
