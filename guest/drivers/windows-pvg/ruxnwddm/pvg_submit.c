// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// A task's submits and syncs, through the task escapes (pvg_object.c
// dispatches them): command buffers handed to the device with command 67,
// counted for each process, and textures written back with command 53.
// Ported with the rest from ruxnpvg's task.c.

#include "pvg_task.h"

// The process's image file name as the kernel keeps it (15 characters);
// exported by the kernel, declared in no header the display headers mix
// with.
NTKERNELAPI PCHAR PsGetProcessImageFileName(PEPROCESS process);

// Counts a submit under the process that sent it, for the PVG_INFO escape:
// the run's proof of whose work reached the device.
static void count_submit(RUXNWDDM_PVG *pvg, NTSTATUS status)
{
	ULONG pid = (ULONG)(ULONG_PTR)PsGetCurrentProcessId();
	PCHAR image = PsGetProcessImageFileName(PsGetCurrentProcess());
	KIRQL irql;
	ULONG i, j;

	KeAcquireSpinLock(&pvg->submit_lock, &irql);
	for (i = 0; i < RUXNWDDM_SUBMIT_PROCESSES; i++) {
		if (pvg->submit_pid[i] == pid || !pvg->submit_pid[i]) {
			if (!pvg->submit_pid[i] && image)
				for (j = 0; j + 1 < RUXNWDDM_IMAGE_NAME && image[j]; j++)
					pvg->submit_image[i][j] = image[j];
			pvg->submit_pid[i] = pid;
			if (NT_SUCCESS(status))
				pvg->submit_done[i]++;
			else
				pvg->submit_refused[i]++;
			break;
		}
	}
	KeReleaseSpinLock(&pvg->submit_lock, irql);
}

// Hands the device command buffers: the task, the two counts, for each
// resource its number and its flags, and for each command buffer its
// address in the task and its length, 64 bits each. A command the device
// refuses faults the ring, which the ring's sender resumes, and comes back
// as STATUS_DEVICE_DATA_ERROR. The ring has one sender at a time, so the
// refusal is this command's.
NTSTATUS pvg_task_submit(RUXNWDDM_CLIENT *client, const struct ruxnpvg_submit *args,
		       const struct ruxnpvg_resource *resources,
		       const struct ruxnpvg_command_buffer *buffers)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_task *task;
	ULONG *payload, *word, words, i;
	NTSTATUS status;

	words = 3 + 2 * args->resource_count + 4 * args->buffer_count;
	payload = ExAllocatePool2(POOL_FLAG_NON_PAGED, words * sizeof(ULONG), POOL_TAG_TEMP);
	if (!payload)
		return E_NOMEM;

	status = client_task(client, &task);
	if (!NT_SUCCESS(status))
		goto out;

	word = payload;
	*word++ = task->id;
	*word++ = args->resource_count;
	*word++ = args->buffer_count;
	status = E_INVAL;
	for (i = 0; i < args->resource_count; i++) {
		struct pvg_object *object = task_object(task, resources[i].id);

		if (!object || !kind_is_resource(object->kind) ||
		    (resources[i].flags & ~RUXNPVG_RESOURCE_WRITTEN))
			goto out;
		*word++ = resources[i].id;
		*word++ = resources[i].flags;
	}
	for (i = 0; i < args->buffer_count; i++) {
		struct pvg_bo *bo;
		ULONG64 address;

		if (buffers[i].pad) {
			status = E_INVAL;
			goto out;
		}
		status = task_memory(client, buffers[i].handle, buffers[i].offset, buffers[i].size,
				     &bo);
		if (!NT_SUCCESS(status))
			goto out;
		address = ((ULONG64)bo->first_page << RUXNWDDM_PAGE_SHIFT) + buffers[i].offset;
		*word++ = lower_32(address);
		*word++ = upper_32(address);
		*word++ = lower_32(buffers[i].size);
		*word++ = upper_32(buffers[i].size);
	}

	if (!task->defined) {
		status = E_NODEV;
		goto out;
	}
	status = ruxnwddm_ring_send(pvg->adapter, RUXNPVG_CMD_TASK_EXEC, payload, words, TASK_TIMEOUT_MS);
	InterlockedIncrement64(&pvg->tasks.submits);
	count_submit(pvg, status);
	if (status == E_IO)
		InterlockedIncrement64(&pvg->tasks.refused);
out:
	ExFreePoolWithTag(payload, POOL_TAG_TEMP);
	return status;
}

// Has the host write textures back into the memory they name.
NTSTATUS pvg_task_sync(RUXNWDDM_CLIENT *client, const struct ruxnpvg_sync *args, const ULONG *ids)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_task *task = client->task;
	ULONG *payload, i;
	NTSTATUS status;

	if (!task)
		return E_NOENT;

	payload = ExAllocatePool2(POOL_FLAG_NON_PAGED, (args->count + 2) * sizeof(ULONG),
				  POOL_TAG_TEMP);
	if (!payload)
		return E_NOMEM;
	RtlCopyMemory(payload + 2, ids, args->count * sizeof(ULONG));
	payload[0] = task->id;
	payload[1] = args->count;
	status = E_INVAL;
	for (i = 0; i < args->count; i++) {
		struct pvg_object *object = task_object(task, payload[2 + i]);

		if (!object || !kind_is_resource(object->kind))
			goto out;
	}
	if (!task->defined) {
		status = E_NODEV;
		goto out;
	}
	status = ruxnwddm_ring_send(pvg->adapter, RUXNPVG_CMD_TASK_SYNC, payload, args->count + 2,
				   TASK_TIMEOUT_MS);
out:
	ExFreePoolWithTag(payload, POOL_TAG_TEMP);
	return status;
}
