// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Clients of the tasks, and the escape that carries ruxnpvg's task
// controls to them.
//
// ruxnpvg took its task controls as I/O controls on \\.\ruxnpvg, one task
// per open handle. Under the graphics kernel the device has no handle of
// its own to open, so a program reaches its task through D3DKMTEscape on a
// kernel device it made on our adapter: the escape's private data is a
// header, the control's code and its input, and the driver writes the
// control's output back into the same buffer. The controls, their checks
// and their structures are ruxnpvg's (ruxnpvg_ioctl.h), unchanged, so a
// program written for ruxnpvg needs only its transport changed.
//
// Escapes come at PASSIVE_LEVEL in the caller's thread, so a buffer object
// is mapped into the process that asked, as ruxnpvg's controls did. A
// client belongs to the process that made its kernel device, and only that
// process may use it. The device's end, which the graphics kernel brings
// about when the program closes it or exits, is the handle's end: the task
// is deleted and the memory unmapped and freed.
//
// Any process may make tasks: this is what a GPU driver offers every
// program. The display controls are not offered: the graphics kernel's
// flips drive the display (flip.c), through the display's own client.

#include "pvg_task.h"

#define POOL_TAG_CLIENT		'lxur'
#define POOL_TAG_ESCAPE		'exur'

// The longest input a task control takes: OBJECT_CREATE with the longest
// tail.
#define MAX_INPUT		(sizeof(struct ruxnpvg_object_create) + MAX_DESCRIPTION)

void pvg_init(RUXNWDDM_PVG *pvg, RUXNWDDM_ADAPTER *a)
{
	pvg->adapter = a;
	pvg->mapper = &pvg->mapper_state;
	pvg->display = &pvg->display_state;
	InitializeListHead(&pvg->clients);
	ruxnwddm_lock_init(&pvg->clients_lock);
	KeInitializeSpinLock(&pvg->ids_lock);
	KeInitializeSpinLock(&pvg->submit_lock);
	pvg->task_gate = 0;
	pvg_mapper_init(pvg);
	pvg_display_init(pvg);
}

// At adapter start: BAR 2 is mapped (only mapped; the mapper hears nothing
// until a surface is first mapped) and the display's client is made.
NTSTATUS pvg_start(RUXNWDDM_PVG *pvg)
{
	RUXNWDDM_ADAPTER *a = pvg->adapter;

	if (a->mapper_phys.QuadPart && a->mapper_length > RUXNPVG_MAPPER_REG_LAST) {
		pvg->mapper_regs = MmMapIoSpaceEx(a->mapper_phys, a->mapper_length,
						  PAGE_READWRITE | PAGE_NOCACHE);
		ruxnwddm_record_ulong(L"MapperMapped", pvg->mapper_regs != NULL);
	}
	if (!pvg->display_client)
		pvg->display_client = pvg_client_create(pvg, FALSE);
	if (!pvg->display_client)
		return STATUS_INSUFFICIENT_RESOURCES;
	pvg_power_up(pvg);
	return STATUS_SUCCESS;
}

void pvg_power_up(RUXNWDDM_PVG *pvg)
{
	InterlockedIncrement64(&pvg->power_ups);
	// The kernel's own client for the display has no process to make a
	// new one, so it comes back with the device (run rn25b: left dead
	// after a resume, every take failed and the screen stayed black).
	if (pvg->display_client) {
		ruxnwddm_lock(&pvg->display_client->lock);
		pvg->display_client->dead = FALSE;
		ruxnwddm_unlock(&pvg->display_client->lock);
	}
	InterlockedExchange(&pvg->task_gate, 1);
}

// Before the ring goes off: the screen goes back, every task is deleted
// while the ring can still carry the command, and the mappings are taken
// away. The clients stay, with their memory mapped until their end, but get
// no task again.
void pvg_power_down(RUXNWDDM_PVG *pvg)
{
	PLIST_ENTRY link;

	InterlockedExchange(&pvg->task_gate, 0);
	pvg_display_power_down(pvg);
	ruxnwddm_lock(&pvg->clients_lock);
	for (link = pvg->clients.Flink; link != &pvg->clients; link = link->Flink) {
		RUXNWDDM_CLIENT *client = CONTAINING_RECORD(link, RUXNWDDM_CLIENT, link);

		ruxnwddm_lock(&client->lock);
		if (client->task)
			task_end(pvg, client->task);
		client->dead = TRUE;
		ruxnwddm_unlock(&client->lock);
	}
	ruxnwddm_unlock(&pvg->clients_lock);
	if (pvg->mapper)
		pvg_mapper_power_down(pvg);
	InterlockedIncrement64(&pvg->power_downs);
	ruxnwddm_record_count(L"PvgPowerDowns");
}

// At adapter stop, after the power-down: the mapper forgets its ring and
// BAR 2 is unmapped.
void pvg_stop(RUXNWDDM_PVG *pvg)
{
	RUXNWDDM_ADAPTER *a = pvg->adapter;

	pvg_mapper_release(pvg);
	if (pvg->mapper_regs) {
		MmUnmapIoSpace((PVOID)pvg->mapper_regs, a->mapper_length);
		pvg->mapper_regs = NULL;
	}
}

ULONG64 pvg_open_clients(RUXNWDDM_PVG *pvg)
{
	PLIST_ENTRY link;
	ULONG64 count = 0;

	ruxnwddm_lock(&pvg->clients_lock);
	for (link = pvg->clients.Flink; link != &pvg->clients; link = link->Flink)
		count++;
	ruxnwddm_unlock(&pvg->clients_lock);
	return count;
}

// In the maker's context, for a program's client; with own_process FALSE
// for the display's, which belongs to no process.
RUXNWDDM_CLIENT *pvg_client_create(RUXNWDDM_PVG *pvg, BOOLEAN own_process)
{
	RUXNWDDM_CLIENT *client;

	client = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*client), POOL_TAG_CLIENT);
	if (!client)
		return NULL;
	client->pvg = pvg;
	ruxnwddm_lock_init(&client->lock);
	if (own_process) {
		client->process = PsGetCurrentProcess();
		ObReferenceObject(client->process);
	}
	ruxnwddm_lock(&pvg->clients_lock);
	InsertTailList(&pvg->clients, &client->link);
	client->listed = TRUE;
	ruxnwddm_unlock(&pvg->clients_lock);
	return client;
}

// The client's end, as ruxnpvg's file cleanup and close: the task is
// deleted, then the memory unmapped and freed. Mappings are undone only in
// the maker's process; outside it the memory is left allocated (counted as
// kept) rather than freed under a live mapping.
void pvg_client_close(RUXNWDDM_CLIENT *client)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	BOOLEAN lost = FALSE;
	ULONG i;

	if (client->listed) {
		ruxnwddm_lock(&pvg->clients_lock);
		RemoveEntryList(&client->link);
		client->listed = FALSE;
		ruxnwddm_unlock(&pvg->clients_lock);
	}

	ruxnwddm_lock(&client->lock);
	client->dead = TRUE;
	if (client->task) {
		task_end(pvg, client->task);
		lost = client->task->lost;
		task_free(pvg, client->task);
		client->task = NULL;
	}
	// A task the device would not let go keeps what its objects name.
	for (i = 0; i < client->bo_count; i++) {
		if (client->bos[i])
			pvg_bo_release(client, client->bos[i], lost);
		client->bos[i] = NULL;
	}
	client->bo_count = 0;
	// The screen goes back before the surfaces, as letting the display go
	// is what lets the one on show be unmapped.
	pvg_display_file_closing(client);
	for (i = 0; i < client->surface_count; i++) {
		if (client->surfaces[i])
			pvg_surface_release(client, client->surfaces[i]);
		client->surfaces[i] = NULL;
	}
	client->surface_count = 0;
	ruxnwddm_unlock(&client->lock);

	if (client->process)
		ObDereferenceObject(client->process);
	ExFreePoolWithTag(client, POOL_TAG_CLIENT);
}

void pvg_task_info(RUXNWDDM_PVG *pvg, struct ruxnpvg_task_info *info)
{
	RUXNWDDM_TASK_COUNTS *tasks = &pvg->tasks;

	RtlZeroMemory(info, sizeof(*info));
	info->defined = (ULONG64)tasks->defined;
	info->deleted = (ULONG64)tasks->deleted;
	info->lost = (ULONG64)tasks->lost;
	info->submits = (ULONG64)tasks->submits;
	info->refused = (ULONG64)tasks->refused;
	info->bos = (ULONG64)tasks->bos;
	info->bos_freed = (ULONG64)tasks->bos_freed;
	info->bos_kept = (ULONG64)tasks->bos_kept;
	info->unmapped = (ULONG64)tasks->unmapped;
	info->bo_bytes = (ULONG64)tasks->bo_bytes;
	info->open_files = pvg_open_clients(pvg);
}

// A task control in an escape on a kernel device. *client_slot is the
// device's client, made here the first time. The header has been checked
// by escape.c; the rest is checked here before anything is read.
NTSTATUS pvg_escape(RUXNWDDM_PVG *pvg, RUXNWDDM_CLIENT **client_slot, void *data, UINT size)
{
	struct ruxnwddm_escape_pvg *esc = data;
	RUXNWDDM_CLIENT *client;
	ULONG_PTR returned = 0;
	UCHAR *in;
	NTSTATUS status;

	if (size < sizeof(*esc) || esc->capacity != size - sizeof(*esc) ||
	    esc->in_length > esc->capacity || esc->in_length > MAX_INPUT)
		return STATUS_INVALID_PARAMETER;
	esc->out_length = 0;
	if (!client_slot)
		return STATUS_INVALID_DEVICE_REQUEST;	// a task needs a kernel device

	client = *client_slot;
	if (!client) {
		client = pvg_client_create(pvg, TRUE);
		if (!client)
			return STATUS_INSUFFICIENT_RESOURCES;
		// Two threads of one program may race here; the loser's goes.
		if (InterlockedCompareExchangePointer((PVOID volatile *)client_slot, client, NULL)) {
			pvg_client_close(client);
			client = *client_slot;
		}
	}
	// Memory is mapped into the maker only, and undone there.
	if (PsGetCurrentProcess() != client->process)
		return STATUS_ACCESS_DENIED;

	// The input is copied before anything is written: the output goes
	// into the same buffer.
	in = ExAllocatePool2(POOL_FLAG_NON_PAGED, esc->in_length ? esc->in_length : 1,
			     POOL_TAG_ESCAPE);
	if (!in)
		return STATUS_INSUFFICIENT_RESOURCES;
	RtlCopyMemory(in, esc->data, esc->in_length);
	ruxnwddm_lock(&client->lock);
	status = pvg_task_dispatch(client, esc->code, in, esc->in_length, esc->data, esc->capacity,
				   &returned);
	ruxnwddm_unlock(&client->lock);
	ExFreePoolWithTag(in, POOL_TAG_ESCAPE);
	esc->out_length = (unsigned int)returned;
	return status;
}

// From the interrupt routine, at its level: message 1 is the mapper's;
// for message 0, what the display events register read.
void pvg_interrupt(RUXNWDDM_PVG *pvg, ULONG message, ULONG events)
{
	if (message == RUXNWDDM_MSG_MAPPER) {
		InterlockedExchange(&pvg->mapper_irq_pending, 1);
		InterlockedIncrement64(&pvg->screen.mapper_irqs);
		return;
	}
	pvg_display_irq(pvg, events);
}

// From the DPC the interrupt asked for.
void pvg_dpc(RUXNWDDM_PVG *pvg)
{
	if (InterlockedExchange(&pvg->mapper_irq_pending, 0))
		pvg_mapper_irq(pvg);
	pvg_display_dpc(pvg);
}
