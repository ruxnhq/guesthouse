// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Surfaces, and their mapping ids from the mapper, ported from
// pvg_surface.c of the Linux driver at 377700a.
//
// Linux makes a surface of separate device pages and lists each in its
// list page. Here a surface is one block of contiguous memory, as buffer
// objects were up to 0.3.57, and its list page still names each 16 KiB page
// on its own, so the device sees what Linux's scattered pages give it, and
// a surface could be built of separate pages as buffer objects are now. It is mapped
// into the process that made it with an MDL and unmapped there.
//
// A surface gets a mapping id when a kind 11 object first names it or it is
// first shown. Mapping over an id that is mapped is no error for the device
// and changes nothing, so the id is unmapped first. An id the device will
// not unmap is one a driver before this one left on a display; it is passed
// over. The device will not unmap a surface a display shows: such a surface,
// once given up, goes on the mapper's held list until a swap or the display's
// release lets it go.
//
// A surface may be shared: its maker exports it for a token, which is a
// random number of 64 bits, and other handles import it by the token. The
// token is a capability rather than a name, so that a program cannot reach
// a surface it was not given by counting; the device is open to
// administrators only, as before. Each handle that holds the surface counts
// once in its holders; the memory, the list page and the mapping id go when
// the last holder lets go, and not before. Only the maker has it mapped,
// so only the maker's process can write its pixels from the CPU; importers
// reach it through the GPU alone. A kind 11 object in an importer's task
// names the same mapping id as one in the maker's: one mapping, two users.

#include "pvg.h"

// After the kernel's headers, which it needs.
#include <bcrypt.h>

#define POOL_TAG_SURFACE	'sxur'

// Words of a surface's descriptor.
#define DESC_PAGE_LIST		0
#define DESC_FORMAT		1
#define DESC_BYTES		4
#define DESC_WIDTH		5	// an element's width, and the width above it
#define DESC_HEIGHT		6
#define DESC_PITCH		7
#define DESC_PIXEL_BYTES	8

// The pixel format's code: the characters "BGRA".
#define FORMAT_BGRA		0x42475241
#define PIXEL_BYTES		4
// An element is one pixel wide and high.
#define ELEMENT_SIDE		1
#define SIDE_SHIFT		8

static ULONG map_entry(ULONG page)
{
	return page << 2 | 1;
}

static void surface_free(RUXNWDDM_PVG *pvg, struct pvg_surface *surface)
{
	if (surface->mdl)
		IoFreeMdl(surface->mdl);
	ruxnwddm_free_pages(&surface->memory);
	ruxnwddm_free_pages(&surface->list);
	InterlockedAdd64(&pvg->screen.surface_bytes, -(LONG64)surface->bytes);
	InterlockedIncrement64(&pvg->screen.surfaces_freed);
	ruxnwddm_klog_event("surface-free", surface->width, surface->height, surface->bytes, 0);
	ExFreePoolWithTag(surface, POOL_TAG_SURFACE);
}

// Gives up the bookkeeping of a surface the device may still read, and
// leaves its memory allocated for good.
void pvg_surface_forget(RUXNWDDM_PVG *pvg, struct pvg_surface *surface)
{
	InterlockedIncrement64(&pvg->screen.surfaces_kept);
	ExFreePoolWithTag(surface, POOL_TAG_SURFACE);
}

// Zeroed memory for width by height pixels and its list page, not yet
// mapped anywhere, held by the handle that makes it.
NTSTATUS pvg_surface_create(RUXNWDDM_CLIENT *client, ULONG width, ULONG height,
				struct pvg_surface **out)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_surface *surface;
	volatile ULONG *list;
	ULONG pages, first, i;
	ULONG64 bytes;

	*out = NULL;
	if (!width || !height || width > RUXNPVG_SURFACE_MAX_SIDE ||
	    height > RUXNPVG_SURFACE_MAX_SIDE)
		return STATUS_INVALID_PARAMETER;
	bytes = ((ULONG64)width * PIXEL_BYTES * height + RUXNWDDM_PAGE_SIZE - 1) &
		~((ULONG64)RUXNWDDM_PAGE_SIZE - 1);
	if (bytes > RUXNPVG_SURFACE_MAX_BYTES)
		return STATUS_INVALID_PARAMETER;

	surface = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*surface), POOL_TAG_SURFACE);
	if (!surface)
		return STATUS_INSUFFICIENT_RESOURCES;
	surface->width = width;
	surface->height = height;
	surface->pitch = width * PIXEL_BYTES;
	surface->bytes = bytes;
	surface->maker = client;
	surface->holders = 1;
	InitializeListHead(&surface->link);
	InitializeListHead(&surface->exported);

	if (!NT_SUCCESS(ruxnwddm_alloc_mapper_pages(&surface->memory, (SIZE_T)bytes)) ||
	    !NT_SUCCESS(ruxnwddm_alloc_mapper_pages(&surface->list, RUXNWDDM_PAGE_SIZE))) {
		ruxnwddm_free_pages(&surface->memory);
		ruxnwddm_free_pages(&surface->list);
		ExFreePoolWithTag(surface, POOL_TAG_SURFACE);
		return STATUS_INSUFFICIENT_RESOURCES;
	}
	surface->mdl = IoAllocateMdl(surface->memory.va, (ULONG)bytes, FALSE, FALSE, NULL);
	if (!surface->mdl) {
		ruxnwddm_free_pages(&surface->memory);
		ruxnwddm_free_pages(&surface->list);
		ExFreePoolWithTag(surface, POOL_TAG_SURFACE);
		return STATUS_INSUFFICIENT_RESOURCES;
	}
	MmBuildMdlForNonPagedPool(surface->mdl);

	pages = (ULONG)(bytes >> RUXNWDDM_PAGE_SHIFT);
	first = ruxnwddm_page_number(&surface->memory);
	list = surface->list.va;
	for (i = 0; i < pages; i++)
		list[i] = map_entry(first + i);

	InterlockedIncrement64(&pvg->screen.surfaces);
	InterlockedAdd64(&pvg->screen.surface_bytes, (LONG64)bytes);
	InterlockedIncrement64(&pvg->screen.holders_now);
	*out = surface;
	return STATUS_SUCCESS;
}

// Maps the memory into the current process, which must be the opener.
NTSTATUS pvg_surface_map_user(struct pvg_surface *surface)
{
	PVOID user = NULL;

	// A user mapping that fails raises an exception rather than return.
	__try {
		user = MmMapLockedPagesSpecifyCache(surface->mdl, UserMode, MmCached, NULL, FALSE,
						    NormalPagePriority | MdlMappingNoExecute);
	} __except (EXCEPTION_EXECUTE_HANDLER) {
		user = NULL;
	}
	if (!user)
		return STATUS_INSUFFICIENT_RESOURCES;
	surface->user = user;
	return STATUS_SUCCESS;
}

static void id_set(struct pvg_mapper *mapper, ULONG id)
{
	mapper->ids[id / 32] |= 1U << (id % 32);
}

static void id_clear(struct pvg_mapper *mapper, ULONG id)
{
	mapper->ids[id / 32] &= ~(1U << (id % 32));
}

static ULONG id_free(struct pvg_mapper *mapper)
{
	ULONG id;

	for (id = 1; id < MAPPER_IDS; id++) {
		if (!(mapper->ids[id / 32] & (1U << (id % 32))))
			return id;
	}
	return 0;
}

// Gives the surface a mapping id, if it has none.
NTSTATUS pvg_surface_map(RUXNWDDM_PVG *pvg, struct pvg_surface *surface)
{
	struct pvg_mapper *mapper = pvg->mapper;
	volatile ULONG *descriptor;
	ULONG id, outcome, i;
	NTSTATUS status;

	ruxnwddm_lock(&mapper->lock);
	if (surface->id) {
		status = STATUS_SUCCESS;
		goto out;
	}
	status = pvg_mapper_start(pvg);
	if (!NT_SUCCESS(status))
		goto out;

	for (;;) {
		id = id_free(mapper);
		if (!id) {
			status = STATUS_INSUFFICIENT_RESOURCES;
			goto out;
		}
		descriptor = pvg_mapper_descriptor(pvg, id);
		if (!descriptor) {
			status = STATUS_INSUFFICIENT_RESOURCES;
			goto out;
		}
		status = pvg_mapper_command(pvg, MAPPER_CMD_UNMAP, id, &outcome);
		if (!NT_SUCCESS(status))
			goto out;
		if (!outcome)
			break;
		InterlockedIncrement64(&pvg->screen.ids_passed);
		id_set(mapper, id);
	}

	for (i = 0; i < MAPPER_DESCRIPTOR / 4; i++)
		descriptor[i] = 0;
	descriptor[DESC_PAGE_LIST] = map_entry(ruxnwddm_page_number(&surface->list));
	descriptor[DESC_FORMAT] = FORMAT_BGRA;
	descriptor[DESC_BYTES] = (ULONG)surface->bytes;
	descriptor[DESC_WIDTH] = surface->width << SIDE_SHIFT | ELEMENT_SIDE;
	descriptor[DESC_HEIGHT] = surface->height << SIDE_SHIFT | ELEMENT_SIDE;
	descriptor[DESC_PITCH] = surface->pitch;
	descriptor[DESC_PIXEL_BYTES] = PIXEL_BYTES;
	KeMemoryBarrier();

	status = pvg_mapper_command(pvg, MAPPER_CMD_MAP, id, &outcome);
	if (NT_SUCCESS(status) && outcome) {
		InterlockedIncrement64(&pvg->screen.outcomes_other);
		ruxnwddm_record_ulong(L"MapRefusedOutcome", outcome);
		status = STATUS_DEVICE_DATA_ERROR;
	}
	if (NT_SUCCESS(status)) {
		id_set(mapper, id);
		surface->id = id;
		InsertHeadList(&mapper->mapped, &surface->link);
		InterlockedIncrement64(&pvg->screen.maps);
		InterlockedIncrement64(&pvg->screen.mapped_now);
	}
out:
	ruxnwddm_unlock(&mapper->lock);
	return status;
}

// Takes the surface's mapping away. STATUS_DEVICE_BUSY says a display still
// shows it: it stays mapped. The caller holds the mapper's lock.
static NTSTATUS unmap_locked(RUXNWDDM_PVG *pvg, struct pvg_surface *surface)
{
	struct pvg_mapper *mapper = pvg->mapper;
	ULONG outcome;
	NTSTATUS status;

	if (!surface->id)
		return STATUS_SUCCESS;
	status = pvg_mapper_command(pvg, MAPPER_CMD_UNMAP, surface->id, &outcome);
	if (NT_SUCCESS(status) && outcome) {
		if (outcome == MAPPER_OUTCOME_BUSY) {
			InterlockedIncrement64(&pvg->screen.outcomes_busy);
			status = STATUS_DEVICE_BUSY;
		} else {
			InterlockedIncrement64(&pvg->screen.outcomes_other);
			ruxnwddm_record_ulong(L"UnmapRefusedOutcome", outcome);
			status = STATUS_DEVICE_DATA_ERROR;
		}
	}
	if (NT_SUCCESS(status)) {
		id_clear(mapper, surface->id);
		surface->id = 0;
		RemoveEntryList(&surface->link);
		InitializeListHead(&surface->link);
		InterlockedIncrement64(&pvg->screen.unmaps);
		InterlockedDecrement64(&pvg->screen.mapped_now);
	}
	return status;
}

// A token no exported surface has, and never 0. The caller holds the
// mapper's lock.
static NTSTATUS new_token(struct pvg_mapper *mapper, ULONG64 *token)
{
	PLIST_ENTRY link;
	NTSTATUS status;

	for (;;) {
		status = BCryptGenRandom(NULL, (PUCHAR)token, sizeof(*token),
					 BCRYPT_USE_SYSTEM_PREFERRED_RNG);
		if (!NT_SUCCESS(status))
			return status;
		if (!*token)
			continue;
		for (link = mapper->exported.Flink; link != &mapper->exported; link = link->Flink) {
			if (CONTAINING_RECORD(link, struct pvg_surface, exported)->token == *token)
				break;
		}
		if (link == &mapper->exported)
			return STATUS_SUCCESS;
	}
}

// The maker hands out a token for the surface, the same one each time.
NTSTATUS pvg_surface_export(RUXNWDDM_CLIENT *client, struct pvg_surface *surface,
				ULONG64 *token)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_mapper *mapper = pvg->mapper;
	NTSTATUS status = STATUS_SUCCESS;

	*token = 0;
	if (surface->maker != client)
		return STATUS_ACCESS_DENIED;
	ruxnwddm_lock(&mapper->lock);
	if (!surface->token) {
		status = new_token(mapper, &surface->token);
		if (NT_SUCCESS(status)) {
			InsertTailList(&mapper->exported, &surface->exported);
			InterlockedIncrement64(&pvg->screen.exports);
			InterlockedIncrement64(&pvg->screen.exported_now);
		} else {
			surface->token = 0;
		}
	}
	*token = surface->token;
	ruxnwddm_unlock(&mapper->lock);
	return status;
}

// Another handle takes a hold on the surface that has the token. The
// caller puts it in the handle's table.
NTSTATUS pvg_surface_import(RUXNWDDM_CLIENT *client, ULONG64 token, struct pvg_surface **out)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_mapper *mapper = pvg->mapper;
	struct pvg_surface *found = NULL;
	PLIST_ENTRY link;

	*out = NULL;
	if (!token)
		return STATUS_INVALID_PARAMETER;
	ruxnwddm_lock(&mapper->lock);
	for (link = mapper->exported.Flink; link != &mapper->exported; link = link->Flink) {
		struct pvg_surface *surface = CONTAINING_RECORD(link, struct pvg_surface,
								    exported);

		if (surface->token == token) {
			found = surface;
			break;
		}
	}
	if (!found) {
		ruxnwddm_unlock(&mapper->lock);
		InterlockedIncrement64(&pvg->screen.imports_refused);
		return STATUS_NOT_FOUND;
	}
	// The maker has it already, and a second entry of its own would undo
	// its mapping when given up.
	if (found->maker == client) {
		ruxnwddm_unlock(&mapper->lock);
		return STATUS_INVALID_PARAMETER;
	}
	found->holders++;
	ruxnwddm_unlock(&mapper->lock);
	InterlockedIncrement64(&pvg->screen.imports);
	InterlockedIncrement64(&pvg->screen.holders_now);
	*out = found;
	return STATUS_SUCCESS;
}

// The handle lets its hold on the surface go. If it is the maker, the
// mapping into its process is undone, which must happen in that process.
// When the last holder lets go, the token stops working, the mapping id is
// taken away and the memory goes. A surface on show then goes on the
// held list; one the device would not unmap for another reason, or that a
// lost task's object still names, or whose user mapping could not be
// undone, keeps its memory for good.
void pvg_surface_release(RUXNWDDM_CLIENT *client, struct pvg_surface *surface)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_mapper *mapper = pvg->mapper;
	BOOLEAN maker = surface->maker == client, keep;
	NTSTATUS status;

	if (maker && surface->user) {
		if (PsGetCurrentProcess() == client->process) {
			MmUnmapLockedPages(surface->user, surface->mdl);
			surface->user = NULL;
		} else {
			surface->kept = TRUE;
		}
	}

	ruxnwddm_lock(&mapper->lock);
	if (maker) {
		surface->maker = NULL;
		if (surface->holders > 1)
			InterlockedIncrement64(&pvg->screen.outlived_maker);
	} else {
		InterlockedIncrement64(&pvg->screen.import_releases);
	}
	InterlockedDecrement64(&pvg->screen.holders_now);
	if (--surface->holders) {
		ruxnwddm_unlock(&mapper->lock);
		return;
	}
	// The token works as long as anyone holds the surface, the maker or
	// not: the compositor often opens a window's surface after the
	// program has let its own handle go, while the allocation over it
	// still holds it (runs rn40 to rn55: such imports failed with
	// STATUS_NOT_FOUND and dwm drew nothing for them).
	if (surface->token) {
		RemoveEntryList(&surface->exported);
		InitializeListHead(&surface->exported);
		surface->token = 0;
		InterlockedDecrement64(&pvg->screen.exported_now);
	}

	// The last holder. No new kind 11 object can name it now; one that
	// still does is a lost task's.
	keep = surface->kept || InterlockedCompareExchange(&surface->objects, 0, 0) != 0;
	if (surface->id && !keep) {
		status = unmap_locked(pvg, surface);
		if (status == STATUS_DEVICE_BUSY) {
			RemoveEntryList(&surface->link);
			InsertTailList(&mapper->held, &surface->link);
			InterlockedIncrement64(&pvg->screen.held_now);
			ruxnwddm_unlock(&mapper->lock);
			return;
		}
		if (!NT_SUCCESS(status))
			keep = TRUE;
	}
	if (keep) {
		// Out of the mapper's sight, with its memory left to the device.
		if (surface->id) {
			RemoveEntryList(&surface->link);
			InterlockedDecrement64(&pvg->screen.mapped_now);
		}
		ruxnwddm_unlock(&mapper->lock);
		pvg_surface_forget(pvg, surface);
		return;
	}
	ruxnwddm_unlock(&mapper->lock);
	surface_free(pvg, surface);
}

// Frees the surfaces that were given up while on show and no longer are.
void pvg_surface_reap(RUXNWDDM_PVG *pvg)
{
	struct pvg_mapper *mapper = pvg->mapper;
	PLIST_ENTRY link, next;

	ruxnwddm_lock(&mapper->lock);
	for (link = mapper->held.Flink; link != &mapper->held; link = next) {
		struct pvg_surface *surface = CONTAINING_RECORD(link, struct pvg_surface, link);

		next = link->Flink;
		if (NT_SUCCESS(unmap_locked(pvg, surface))) {
			InterlockedDecrement64(&pvg->screen.held_now);
			surface_free(pvg, surface);
		}
	}
	ruxnwddm_unlock(&mapper->lock);
}

// At power-down, once the display is let go: every mapping is taken away.
// The memory stays its handle's until the handle's end.
void pvg_surfaces_unmap_all(RUXNWDDM_PVG *pvg)
{
	struct pvg_mapper *mapper = pvg->mapper;
	PLIST_ENTRY link, next;

	pvg_surface_reap(pvg);
	ruxnwddm_lock(&mapper->lock);
	for (link = mapper->mapped.Flink; link != &mapper->mapped; link = next) {
		next = link->Flink;
		unmap_locked(pvg, CONTAINING_RECORD(link, struct pvg_surface, link));
	}
	ruxnwddm_unlock(&mapper->lock);
}

// Its holders, for the allocation list (alloc_list.c); read without the lock.
ULONG pvg_surface_holders(const struct pvg_surface *surface)
{
	return surface->holders;
}
