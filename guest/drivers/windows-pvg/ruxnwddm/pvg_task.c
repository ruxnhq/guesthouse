// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// A task for each open handle, and what a program does with it, ported
// from pvg_task.c of the Linux driver at 377700a.
//
// A task is an address space on the host: a root page, a page table of the
// guest's memory, and a list of objects that the host makes for it.
// Commands that carry work name a task, and through it the memory their
// command buffers lie in and the objects those use.
//
// The driver owns a task's layout. From address 0 lie the object list, the
// placement list, which the host writes, and the objects' descriptions.
// Buffer objects lie behind them, from 2 MiB on. A program never writes the
// lists: it hands a description's tail to the driver, which puts before it
// the part that names memory, so that no program can name what is not its
// own.
//
// The commands sent here, each through the first ring and each waited for,
// with their payloads as Linux sends them (all words 32 bits):
//
//   56 define    task id << 1, the task's size (64 bits), the root page
//   32 delete    task id
//   67 exec      task id, resources, buffers, (id, flags) for each resource,
//                (address, length, 64 bits each) for each command buffer
//   53 sync      task id, count, the object numbers
//   37 delete a resource          task id, object number
//   40 delete an object           task id, delete operation, 12, object number
//
// Surface textures, kind 11, name a surface by its mapping id, which the
// surface gets from the mapper when the object is made (surface.c). The
// surface and display controls are dispatched here too, since they are a
// handle's, and are display.c's. A surface in a handle's table is its own
// or one it imported from another handle (surface.c); either may have a
// kind 11 object in the handle's task, be drawn with and be shown.
//
// Not ported: child rings (Linux has them off by default, and the first
// ring carries everything), the older exec 55 (a module parameter, off by
// default), the unmap command 34, which Linux sends only when a buffer
// object leaves a task that lives on (here buffer objects leave a task only
// at its end, after the delete, when Linux sends nothing either), and
// command 37 for a kind 11 object, which this step does not send: such an
// object goes with its task.
//
// Locking: each handle's lock is held across a whole control and across
// its task's teardown. The device's files_lock is taken before a handle's
// lock, never after it; the ring's lock is taken inside both.

#include "pvg_task.h"

static ULONG task_id_alloc(RUXNWDDM_PVG *pvg)
{
	KIRQL irql;
	ULONG id, found = 0;

	KeAcquireSpinLock(&pvg->ids_lock, &irql);
	for (id = TASK_FIRST_ID; id <= TASK_LAST_ID; id++) {
		if (!(pvg->task_ids[id / 32] & (1U << (id % 32)))) {
			pvg->task_ids[id / 32] |= 1U << (id % 32);
			found = id;
			break;
		}
	}
	KeReleaseSpinLock(&pvg->ids_lock, irql);
	return found;
}

static void task_id_free(RUXNWDDM_PVG *pvg, ULONG id)
{
	KIRQL irql;

	KeAcquireSpinLock(&pvg->ids_lock, &irql);
	pvg->task_ids[id / 32] &= ~(1U << (id % 32));
	KeReleaseSpinLock(&pvg->ids_lock, irql);
}

// Where the table's entry for a page of the task is.
static volatile ULONG *task_entry(struct pvg_task *task, ULONG page)
{
	ULONG index = page >> TASK_TABLE_SHIFT;

	if (!task->tables[index].va) {
		volatile ULONG *top = task->top.va;

		if (!NT_SUCCESS(ruxnwddm_alloc_task_pages(&task->tables[index], RUXNWDDM_PAGE_SIZE)))
			return NULL;
		top[index] = ruxnwddm_page_number(&task->tables[index]);
	}
	return (volatile ULONG *)task->tables[index].va + (page & (TASK_TABLE_ENTRIES - 1));
}

static NTSTATUS task_enter(struct pvg_task *task, ULONG page, const RUXNWDDM_PAGES *memory,
			   BOOLEAN writable)
{
	volatile ULONG *entry = task_entry(task, page);

	if (!entry)
		return E_NOMEM;
	*entry = ruxnwddm_page_number(memory) | (writable ? TASK_ENTRY_WRITABLE : 0);
	return STATUS_SUCCESS;
}

// A list's pages, each made and entered from the task's page FIRST on.
static NTSTATUS task_enter_list(struct pvg_task *task, ULONG first, RUXNWDDM_PAGES *pages, ULONG count,
				BOOLEAN writable)
{
	ULONG i;

	for (i = 0; i < count; i++) {
		if (!NT_SUCCESS(ruxnwddm_alloc_task_pages(&pages[i], RUXNWDDM_PAGE_SIZE)))
			return E_NOMEM;
		if (!NT_SUCCESS(task_enter(task, first + i, &pages[i], writable)))
			return E_NOMEM;
	}
	return STATUS_SUCCESS;
}

static void task_write_root(struct pvg_task *task)
{
	volatile ULONG *root = task->root.va;
	ULONG64 objects = (ULONG64)TASK_OBJECTS_PAGE << RUXNWDDM_PAGE_SHIFT;
	ULONG64 placements = (ULONG64)TASK_PLACEMENTS_PAGE << RUXNWDDM_PAGE_SHIFT;

	RtlZeroMemory(task->root.va, RUXNWDDM_PAGE_SIZE);
	root[ROOT_TABLE] = ruxnwddm_page_number(&task->top);
	root[ROOT_DEPTH] = TASK_DEPTH;
	root[ROOT_OBJECTS] = lower_32(objects);
	root[ROOT_OBJECTS + 1] = upper_32(objects);
	root[ROOT_OBJECTS_BYTES] = OBJECTS_BYTES;
	root[ROOT_PLACEMENTS] = lower_32(placements);
	root[ROOT_PLACEMENTS + 1] = upper_32(placements);
	root[ROOT_PLACEMENTS_BYTES] = PLACEMENTS_BYTES;
}

// Tells the device of the task. A task of this id that a driver before this
// one left behind is deleted first.
//
// The task's size (pvg.h): read once, at the first task's definition.
ULONG64 pvg_task_bytes(void)
{
	static ULONG64 bytes;

	if (!bytes) {
		ULONG mib = KeGetCurrentIrql() == PASSIVE_LEVEL ? ruxnwddm_record_read(L"TaskMiB") : 0;

		if (mib < 1024 || mib > (ULONG)(TASK_MOST_BYTES >> 20) || mib % 64)
			mib = (ULONG)(TASK_BYTES >> 20);
		bytes = (ULONG64)mib << 20;
	}
	return bytes;
}

static NTSTATUS task_define(RUXNWDDM_PVG *pvg, struct pvg_task *task)
{
	volatile ULONG *root = task->root.va;
	ULONG payload[4];
	ULONG id = task->id;
	NTSTATUS status;

	payload[0] = task->id << 1;
	payload[1] = lower_32(pvg_task_bytes());
	payload[2] = upper_32(pvg_task_bytes());
	payload[3] = ruxnwddm_page_number(&task->root);

	task_write_root(task);
	status = ruxnwddm_ring_send(pvg->adapter, RUXNPVG_CMD_TASK_DEFINE, payload, 4, 0);
	if (status == E_IO) {
		// The task is there already: defining it anew.
		status = ruxnwddm_ring_send(pvg->adapter, RUXNPVG_CMD_TASK_DELETE, &id, 1, 0);
		if (!NT_SUCCESS(status))
			return status;
		task_write_root(task);
		status = ruxnwddm_ring_send(pvg->adapter, RUXNPVG_CMD_TASK_DEFINE, payload, 4, 0);
	}
	if (!NT_SUCCESS(status))
		return status;
	task->defined = TRUE;
	InterlockedIncrement64(&pvg->tasks.defined);

	// The device zeroes the lists' words if it does not take the lists.
	if (!root[ROOT_OBJECTS_BYTES] || !root[ROOT_PLACEMENTS_BYTES])
		return E_NODEV;
	return STATUS_SUCCESS;
}

// Has the device forget the task. Its memory is the guest's again after.
static void task_delete(RUXNWDDM_PVG *pvg, struct pvg_task *task)
{
	NTSTATUS status;
	ULONG id = task->id;

	if (!task->defined)
		return;
	task->defined = FALSE;
	status = ruxnwddm_ring_send(pvg->adapter, RUXNPVG_CMD_TASK_DELETE, &id, 1, 0);
	if (!NT_SUCCESS(status)) {
		// Its memory is left to the device.
		task->lost = TRUE;
		InterlockedIncrement64(&pvg->tasks.lost);
	} else {
		InterlockedIncrement64(&pvg->tasks.deleted);
	}
}

// A task the device would not let go may still be read and written by it:
// its pages, its id and what its objects name all stay.
void task_free(RUXNWDDM_PVG *pvg, struct pvg_task *task)
{
	ULONG i;

	if (task->lost)
		return;
	for (i = 0; i < TASK_DESCRIPTION_PAGES; i++)
		ruxnwddm_free_pages(&task->descriptions[i]);
	for (i = 0; i < TASK_TABLES; i++)
		ruxnwddm_free_pages(&task->tables[i]);
	for (i = 0; i < TASK_PLACEMENT_LIST_PAGES; i++)
		ruxnwddm_free_pages(&task->placement_pages[i]);
	for (i = 0; i < TASK_OBJECT_LIST_PAGES; i++)
		ruxnwddm_free_pages(&task->object_pages[i]);
	ruxnwddm_free_pages(&task->top);
	ruxnwddm_free_pages(&task->root);
	if (task->id)
		task_id_free(pvg, task->id);
	ExFreePoolWithTag(task, POOL_TAG_TASK);
}

static NTSTATUS task_create(RUXNWDDM_PVG *pvg, struct pvg_task **out)
{
	struct pvg_task *task;
	NTSTATUS status;

	*out = NULL;
	task = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*task), POOL_TAG_TASK);
	if (!task)
		return E_NOMEM;
	task->next_page = TASK_FIRST_BO_PAGE;

	status = ruxnwddm_alloc_task_pages(&task->root, RUXNWDDM_PAGE_SIZE);
	if (NT_SUCCESS(status))
		status = ruxnwddm_alloc_task_pages(&task->top, RUXNWDDM_PAGE_SIZE);
	if (!NT_SUCCESS(status)) {
		status = E_NOMEM;
		goto err;
	}
	// The host reads the object list and writes the placement list.
	status = task_enter_list(task, TASK_OBJECTS_PAGE, task->object_pages, TASK_OBJECT_LIST_PAGES, FALSE);
	if (NT_SUCCESS(status))
		status = task_enter_list(task, TASK_PLACEMENTS_PAGE, task->placement_pages,
					 TASK_PLACEMENT_LIST_PAGES, TRUE);
	if (!NT_SUCCESS(status))
		goto err;

	task->id = task_id_alloc(pvg);
	if (!task->id) {
		status = E_NOSPC;
		goto err;
	}

	status = task_define(pvg, task);
	if (!NT_SUCCESS(status))
		task_delete(pvg, task);
	if (!NT_SUCCESS(status))
		goto err;
	*out = task;
	return STATUS_SUCCESS;

err:
	task_free(pvg, task);
	return status == E_IO ? E_NODEV : status;
}

// The client's task. It is made when the client first needs it, and only
// while the adapter is started and powered. A client whose task the device
// was powered down under gets none again. The caller holds the client's
// lock.
NTSTATUS client_task(RUXNWDDM_CLIENT *client, struct pvg_task **task)
{
	NTSTATUS status;

	*task = NULL;
	if (client->dead)
		return E_NODEV;
	*task = client->task;
	if (*task)
		return STATUS_SUCCESS;
	if (!InterlockedCompareExchange(&client->pvg->task_gate, 0, 0))
		return E_NODEV;
	// The ring goes on with the first task, not at start (ring.c).
	status = ruxnwddm_ring_start(client->pvg->adapter);
	if (!NT_SUCCESS(status))
		return status;
	status = task_create(client->pvg, task);
	if (NT_SUCCESS(status))
		client->task = *task;
	return status;
}

// A place of this many pages: the first given back that holds them, else
// behind the last; or E_NOSPC.
static NTSTATUS take_place(struct pvg_task *task, ULONG pages, ULONG *first)
{
	ULONG i;

	for (i = 0; i < task->free_place_count; i++) {
		struct pvg_place *p = &task->free_places[i];

		if (p->pages < pages)
			continue;
		*first = p->first;
		p->first += pages;
		p->pages -= pages;
		if (!p->pages) {
			RtlMoveMemory(p, p + 1, (task->free_place_count - i - 1) * sizeof(*p));
			task->free_place_count--;
		}
		return STATUS_SUCCESS;
	}
	if (pages > (ULONG)(pvg_task_bytes() >> RUXNWDDM_PAGE_SHIFT) - task->next_page)
		return E_NOSPC;
	*first = task->next_page;
	task->next_page += pages;
	return STATUS_SUCCESS;
}

// A place back into the list, joined to its neighbours. With the list full
// it is not used again, which costs room in the task and nothing else.
static void give_place(struct pvg_task *task, ULONG first, ULONG pages)
{
	struct pvg_place *list = task->free_places;
	ULONG i;

	for (i = 0; i < task->free_place_count && list[i].first < first; i++)
		;
	if (i > 0 && list[i - 1].first + list[i - 1].pages == first) {
		list[i - 1].pages += pages;
		if (i < task->free_place_count && first + pages == list[i].first) {
			list[i - 1].pages += list[i].pages;
			RtlMoveMemory(&list[i], &list[i + 1], (task->free_place_count - i - 1) * sizeof(*list));
			task->free_place_count--;
		}
	} else if (i < task->free_place_count && first + pages == list[i].first) {
		list[i].first = first;
		list[i].pages += pages;
	} else if (task->free_place_count < TASK_FREE_PLACES) {
		RtlMoveMemory(&list[i + 1], &list[i], (task->free_place_count - i) * sizeof(*list));
		list[i].first = first;
		list[i].pages = pages;
		task->free_place_count++;
	}
}

// Puts a buffer object into the task: its device pages, each on its own,
// writable, in a place given back before or behind the last.
NTSTATUS task_place(struct pvg_task *task, struct pvg_bo *bo)
{
	ULONG pages = (ULONG)(bo->bytes >> RUXNWDDM_PAGE_SHIFT);
	ULONG first, i;
	NTSTATUS status;

	status = take_place(task, pages, &first);
	if (!NT_SUCCESS(status))
		return status;
	for (i = 0; i < pages; i++) {
		volatile ULONG *entry = task_entry(task, first + i);

		if (!entry) {
			// Linux clears what it entered; so does this.
			while (i--)
				*task_entry(task, first + i) = 0;
			give_place(task, first, pages);
			return E_NOMEM;
		}
		*entry = pvg_bo_page(bo, i) | TASK_ENTRY_WRITABLE;
	}
	bo->first_page = first;
	return STATUS_SUCCESS;
}

// Takes a buffer object out of the task, as Linux's pvg_placement_put:
// its entries cleared, then the device told to drop what it has mapped of
// it (command 34), and only if the device says it did is the place handed
// out again. FALSE if it did not: the memory must then stay.
BOOLEAN task_unplace(RUXNWDDM_PVG *pvg, struct pvg_task *task, struct pvg_bo *bo)
{
	ULONG pages = (ULONG)(bo->bytes >> RUXNWDDM_PAGE_SHIFT);
	ULONG64 address = (ULONG64)bo->first_page << RUXNWDDM_PAGE_SHIFT;
	ULONG64 bytes = bo->bytes;
	ULONG payload[5], i;
	NTSTATUS status = E_NODEV;

	for (i = 0; i < pages; i++)
		*task_entry(task, bo->first_page + i) = 0;
	if (task->defined) {
		payload[0] = task->id;
		payload[1] = (ULONG)address;
		payload[2] = (ULONG)(address >> 32);
		payload[3] = (ULONG)bytes;
		payload[4] = (ULONG)(bytes >> 32);
		status = ruxnwddm_ring_send(pvg->adapter, RUXNPVG_CMD_TASK_UNMAP, payload, 5, TASK_TIMEOUT_MS);
	}
	if (!NT_SUCCESS(status))
		return FALSE;
	give_place(task, bo->first_page, pages);
	return TRUE;
}

// The buffer object a handle names, with the part offset and size checked
// against it, as pvg_task_memory.
NTSTATUS task_memory(RUXNWDDM_CLIENT *client, ULONG handle, ULONG64 offset, ULONG64 size,
			    struct pvg_bo **bo)
{
	if (!handle || handle > client->bo_count || !client->bos[handle - 1])
		return E_NOENT;
	*bo = client->bos[handle - 1];
	if (!size || offset > (*bo)->bytes || size > (*bo)->bytes - offset)
		return E_INVAL;
	return STATUS_SUCCESS;
}

// Room for a description, as its first piece; the pages come as needed.
// The first place with room, as Linux's bitmap search finds it.
NTSTATUS description_alloc(struct pvg_task *task, ULONG bytes, ULONG *slot_out,
				  ULONG *slots_out)
{
	ULONG slots = (bytes + TASK_DESCRIPTION_SLOT - 1) / TASK_DESCRIPTION_SLOT;
	ULONG slot, run = 0, i, page, last;

	for (slot = 0; slot < TASK_DESCRIPTION_SLOTS; slot++) {
		if (task->description_slots[slot / 32] & (1U << (slot % 32)))
			run = 0;
		else if (++run == slots)
			break;
	}
	if (slot >= TASK_DESCRIPTION_SLOTS)
		return E_NOSPC;
	slot = slot + 1 - slots;

	last = ((slot + slots) * TASK_DESCRIPTION_SLOT - 1) >> RUXNWDDM_PAGE_SHIFT;
	for (page = slot * TASK_DESCRIPTION_SLOT >> RUXNWDDM_PAGE_SHIFT; page <= last; page++) {
		RUXNWDDM_PAGES *memory = &task->descriptions[page];

		if (memory->va)
			continue;
		if (!NT_SUCCESS(ruxnwddm_alloc_task_pages(memory, RUXNWDDM_PAGE_SIZE)))
			return E_NOMEM;
		if (!NT_SUCCESS(task_enter(task, TASK_DESCRIPTIONS_PAGE + page, memory, FALSE))) {
			ruxnwddm_free_pages(memory);
			return E_NOMEM;
		}
	}
	for (i = slot; i < slot + slots; i++)
		task->description_slots[i / 32] |= 1U << (i % 32);
	*slot_out = slot;
	*slots_out = slots;
	return STATUS_SUCCESS;
}

void description_write(struct pvg_task *task, ULONG slot, const UCHAR *data,
			      ULONG bytes)
{
	ULONG at = slot * TASK_DESCRIPTION_SLOT;

	while (bytes) {
		ULONG in_page = at & (RUXNWDDM_PAGE_SIZE - 1);
		ULONG chunk = min(bytes, RUXNWDDM_PAGE_SIZE - in_page);

		RtlCopyMemory((PUCHAR)task->descriptions[at >> RUXNWDDM_PAGE_SHIFT].va + in_page,
			      data, chunk);
		data += chunk;
		at += chunk;
		bytes -= chunk;
	}
}

// A word of an object's entry. Entries are 12 bytes, so one may lie across
// two pages of the list; a word never does.
volatile ULONG *object_word(struct pvg_task *task, ULONG id, ULONG word)
{
	ULONG at = (id * OBJECT_WORDS + word) * 4;

	return (volatile ULONG *)((UCHAR *)task->object_pages[at >> RUXNWDDM_PAGE_SHIFT].va +
				  (at & (RUXNWDDM_PAGE_SIZE - 1)));
}

// An object's placement, two 64-bit words, which never lie across pages.
volatile ULONG64 *placement_words(struct pvg_task *task, ULONG id)
{
	ULONG at = id * PLACEMENT_WORDS * 8;

	return (volatile ULONG64 *)((UCHAR *)task->placement_pages[at >> RUXNWDDM_PAGE_SHIFT].va +
				    (at & (RUXNWDDM_PAGE_SIZE - 1)));
}

// Gives up what the object holds. The number is free after. The buffer
// object it named stays in the task: its handle holds it there.
void object_release(struct pvg_task *task, struct pvg_object *object)
{
	ULONG i;

	if (object->bo)
		object->bo->objects--;
	if (object->surface)
		InterlockedDecrement(&object->surface->objects);
	for (i = object->slot; i < (ULONG)object->slot + object->slots; i++)
		task->description_slots[i / 32] &= ~(1U << (i % 32));
	RtlZeroMemory(object, sizeof(*object));
}

// An operation of the serializer's starts with its number, its length and
// the object's number.
BOOLEAN tail_is_operation(const UCHAR *tail, ULONG bytes, ULONG id)
{
	return bytes >= 12 && !(bytes % 4) && get_le32(tail + 4) == bytes &&
	       get_le32(tail + 8) == id;
}

struct pvg_object *task_object(struct pvg_task *task, ULONG id)
{
	struct pvg_object *object;

	if (!id || id >= TASK_MAX_OBJECTS)
		return NULL;
	object = &task->objects[id];
	return object->kind && !object->retired ? object : NULL;
}

// Whether a command's resource list may name an object of this kind.
BOOLEAN kind_is_resource(UCHAR kind)
{
	return kind == RUXNPVG_OBJECT_BUFFER || kind == RUXNPVG_OBJECT_TEXTURE ||
	       kind == RUXNPVG_OBJECT_SURFACE_TEXTURE;
}

struct pvg_surface *client_surface(RUXNWDDM_CLIENT *client, ULONG handle)
{
	if (!handle || handle > client->surface_count)
		return NULL;
	return client->surfaces[handle - 1];
}

// Whether a kind 11 object of the handle's own task names the surface.
BOOLEAN task_names_surface(RUXNWDDM_CLIENT *client, const struct pvg_surface *surface)
{
	ULONG i;

	if (!client->task)
		return FALSE;
	for (i = 1; i < TASK_MAX_OBJECTS; i++) {
		if (client->task->objects[i].kind && client->task->objects[i].surface == surface)
			return TRUE;
	}
	return FALSE;
}

// The task is deleted first: the host then holds nothing of it, and the
// objects can let go of what they name. The pages stay until task_free.
// The caller holds the handle's lock.
void task_end(RUXNWDDM_PVG *pvg, struct pvg_task *task)
{
	ULONG i;

	task_delete(pvg, task);
	if (task->lost)
		return;
	for (i = 1; i < TASK_MAX_OBJECTS; i++) {
		if (task->objects[i].kind)
			object_release(task, &task->objects[i]);
	}
}
