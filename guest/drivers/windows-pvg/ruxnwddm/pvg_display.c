// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The display: one handle at a time takes the screen from the early
// framebuffer, shows surfaces on it, and gives it back. Ported from
// pvg_display.c of the Linux driver at 377700a, with the early
// framebuffer's steps of pvg_efb.c around it.
//
// The guest hands the display a page of its memory, the state page. The
// device writes the display's modes there and sets event bits; the guest
// writes a mask for the events. Display commands go through the first ring
// and name the kernel task, task 0, which is defined while the display is
// held. Its page table maps a block for the cursor's picture, as Linux's
// does, though this driver never shows a cursor.
//
// The commands sent here, each waited for (all words 32 bits):
//
//   56 define task 0   0, the task's size (64 bits), the root page
//    1 state page      port 0, the page's number (0 lets the display go)
//    2 acknowledge     port 0, the mode list's sequence
//    8 swap            port 0, task 0, the mapping id (64 bits), six zero
//                      words of a colour table
//   32 delete task 0   0
//
// The early framebuffer, which pvgfb turned on at boot and Windows' basic
// display driver draws into, is off while a handle holds the screen. Its
// bank keeps nothing once off, so it is read before and written back after,
// in pvgfb's order.
//
// Where this departs from Linux: a refused command 56 or 1 is not retried.
// Linux deletes and defines task 0 again, or lets the display go and hands
// it over again, for a driver before it that left them; a failed hand-over
// spoils the display for the guard, and the next reset would end the host's
// process. Any failed display command here marks the display spoiled for
// the rest of the driver's life instead.

#include "pvg.h"

// The driver uses the first display only.
#define DISPLAY_PORT		0

#define KERNEL_TASK		0
#define KERNEL_TASK_BYTES	0x100000ULL
#define TASK_ROOT_TABLE		0	// words of a task's root page
#define TASK_ROOT_DEPTH		1
#define KERNEL_TASK_DEPTH	1

// The cursor's picture has its own memory at the start of task 0, large
// enough for the largest the device takes: 256 by 256 pixels.
#define CURSOR_MAX_SIDE		256
#define GLYPH_BYTES		(CURSOR_MAX_SIDE * CURSOR_MAX_SIDE * 4)
#define GLYPH_PAGES		(GLYPH_BYTES / RUXNWDDM_PAGE_SIZE)

// Offsets in the state page, as indices of 32-bit words.
#define STATE_EVENTS		(0x100 / 4)
#define STATE_EVENT_MASK	(0x104 / 4)
#define STATE_SEQUENCE		(0x200 / 4)
#define STATE_MODE_COUNT	(0x208 / 4)	// 16 bits
#define STATE_MODES		(0x210 / 4)
#define STATE_MODE_WORDS	4
#define STATE_LIST_STATE	(0xfc0 / 4)
#define MODE_COUNT_MASK		0xffff
#define MAX_MODES		16

// What the device says of the mode list in the page: not yet acknowledged.
#define LIST_WAITING		1

// Event bits. The device sets them and the guest clears them.
#define EVENT_SWAP_DONE		(1U << 1)
#define EVENT_MODES		(1U << 3)

// The swap's payload: port, task, the id in 64 bits, six colour words.
#define SWAP_WORDS		10

// The early framebuffer's bank, in the order pvgfb writes it.
enum { BANK_PAGE, BANK_BYTES, BANK_DEPTH, BANK_MODE, BANK_REGS };
static const ULONG bank_regs[BANK_REGS] = {
	REG_EFB_PAGE, REG_EFB_BYTES, REG_EFB_DEPTH, REG_EFB_MODE,
};
#define BANK_ON			1
#define BANK_OFF		0
#define BANK_BITS_PER_PIXEL	32
#define MODE_SIDE_MASK		0xffff

void pvg_display_init(RUXNWDDM_PVG *pvg)
{
	ruxnwddm_lock_init(&pvg->display->lock);
}

static volatile ULONG *state_words(struct pvg_display *display)
{
	return display->state_page.va;
}

static void bank_read(RUXNWDDM_PVG *pvg, ULONG bank[BANK_REGS])
{
	ULONG i;

	for (i = 0; i < BANK_REGS; i++)
		bank[i] = ruxnwddm_read(pvg->adapter, bank_regs[i]);
	InterlockedIncrement64(&pvg->screen.bank_reads);
	ruxnwddm_record_ulong(L"BankPage", bank[BANK_PAGE]);
	ruxnwddm_record_ulong(L"BankBytes", bank[BANK_BYTES]);
	ruxnwddm_record_ulong(L"BankDepth", bank[BANK_DEPTH]);
	ruxnwddm_record_ulong(L"BankMode", bank[BANK_MODE]);
}

// Whether the bank reads as pvgfb could have written it: a page, whole
// device pages, 32 bits a pixel and a mode that fits. If not, it cannot be
// put back, so the early framebuffer is not turned off.
static BOOLEAN bank_usable(const ULONG bank[BANK_REGS])
{
	ULONG width = bank[BANK_MODE] & MODE_SIDE_MASK, height = bank[BANK_MODE] >> 16;

	return bank[BANK_PAGE] && bank[BANK_BYTES] &&
	       !(bank[BANK_BYTES] & (RUXNWDDM_PAGE_SIZE - 1)) &&
	       bank[BANK_DEPTH] == BANK_BITS_PER_PIXEL && width && height &&
	       (ULONG64)width * height * 4 <= bank[BANK_BYTES];
}

// Reads the bank and, if it can be put back, turns the early framebuffer
// off. The caller holds the display's lock.
static NTSTATUS bank_off(RUXNWDDM_PVG *pvg)
{
	struct pvg_display *display = pvg->display;

	bank_read(pvg, display->bank);
	if (!bank_usable(display->bank))
		return STATUS_DEVICE_CONFIGURATION_ERROR;
	ruxnwddm_write(pvg->adapter, REG_EFB_ON, BANK_OFF);
	display->bank_off = TRUE;
	InterlockedIncrement64(&pvg->screen.bank_offs);
	return STATUS_SUCCESS;
}

// The early framebuffer on again with the values it had, in pvgfb's
// order: on, then the page, bytes, depth and mode. The picture appears
// with the last write.
static void bank_restore(RUXNWDDM_PVG *pvg)
{
	struct pvg_display *display = pvg->display;
	ULONG i;

	if (!display->bank_off)
		return;
	ruxnwddm_write(pvg->adapter, REG_EFB_ON, BANK_ON);
	for (i = 0; i < BANK_REGS; i++)
		ruxnwddm_write(pvg->adapter, bank_regs[i], display->bank[i]);
	display->bank_off = FALSE;
	InterlockedIncrement64(&pvg->screen.bank_restores);
}

static NTSTATUS display_send(RUXNWDDM_PVG *pvg, ULONG number, const ULONG *payload, ULONG words)
{
	NTSTATUS status = ruxnwddm_ring_send(pvg->adapter, number, payload, words, 0);

	if (!NT_SUCCESS(status)) {
		pvg->display->spoiled = TRUE;
		InterlockedIncrement64(&pvg->screen.display_failed);
		ruxnwddm_record_ulong(L"DisplayFailedCommand", number);
		ruxnwddm_record_ulong(L"DisplayFailedStatus", (ULONG)status);
	}
	return status;
}

// Reads the mode list and acknowledges it, if there is one the device
// waits to have acknowledged. It holds a newer list back until then, and
// refuses an acknowledgement it does not wait for. Returns how many modes
// were taken, 0 if no list waited. At PASSIVE_LEVEL, with the display live.
static ULONG read_modes(RUXNWDDM_PVG *pvg)
{
	struct pvg_display *display = pvg->display;
	volatile ULONG *state = state_words(display);
	ULONG payload[2] = { DISPLAY_PORT, 0 };
	ULONG count, i, taken = 0;

	InterlockedExchange(&display->modes_waiting, 0);
	if (state[STATE_LIST_STATE] != LIST_WAITING)
		return 0;
	display->sequence = state[STATE_SEQUENCE];
	count = state[STATE_MODE_COUNT] & MODE_COUNT_MASK;
	for (i = 0; i < count && taken < MAX_MODES; i++) {
		ULONG size = state[STATE_MODES + i * STATE_MODE_WORDS];

		if (!(size & MODE_SIDE_MASK) || !(size >> 16))
			continue;
		if (!taken)
			display->first_mode = size;
		taken++;
	}
	InterlockedIncrement64(&pvg->screen.mode_lists);
	ruxnwddm_record_ulong(L"ModeListSequence", display->sequence);
	ruxnwddm_record_ulong(L"ModeListCount", count);

	payload[1] = display->sequence;
	// Linux goes on if this fails; the failure is counted and spoils.
	if (NT_SUCCESS(display_send(pvg, RUXNPVG_CMD_DISPLAY_ACK, payload, 2)))
		InterlockedIncrement64(&pvg->screen.acknowledgements);
	return taken;
}

// Takes the display's events and clears them, from the message 0 DPC or a
// sender; the exchange keeps the two from taking one twice. The event bit
// can be lost between the device's setting and the guest's clearing, so a
// waiting mode list is also known by the page's own word.
static void take_events(RUXNWDDM_PVG *pvg)
{
	struct pvg_display *display = pvg->display;
	volatile ULONG *state = state_words(display);
	ULONG events = (ULONG)InterlockedExchange((volatile LONG *)&state[STATE_EVENTS], 0);

	if (events)
		InterlockedIncrement64(&pvg->screen.events_taken);
	if (events & EVENT_SWAP_DONE)
		InterlockedIncrement64(&pvg->screen.swap_done);
	if ((events & EVENT_MODES) || state[STATE_LIST_STATE] == LIST_WAITING)
		InterlockedExchange(&display->modes_waiting, 1);
}

// From the interrupt handler, with what 0x1014 read.
void pvg_display_irq(RUXNWDDM_PVG *pvg, ULONG events)
{
	struct pvg_display *display = pvg->display;

	if (events && InterlockedCompareExchange(&display->live, 0, 0)) {
		InterlockedExchange(&display->events_pending, 1);
		InterlockedIncrement64(&pvg->screen.display_events);
	}
}

// From the message 0 DPC. The let-go clears live and then waits out the
// DPCs before the state page goes.
void pvg_display_dpc(RUXNWDDM_PVG *pvg)
{
	struct pvg_display *display = pvg->display;

	if (!InterlockedExchange(&display->events_pending, 0))
		return;
	if (InterlockedCompareExchange(&display->live, 0, 0))
		take_events(pvg);
}

static void free_display_pages(struct pvg_display *display, BOOLEAN state_page)
{
	ruxnwddm_free_pages(&display->glyph);
	ruxnwddm_free_pages(&display->task_table);
	ruxnwddm_free_pages(&display->task_root);
	if (state_page)
		ruxnwddm_free_pages(&display->state_page);
	else
		RtlZeroMemory(&display->state_page, sizeof(display->state_page));
}

static NTSTATUS define_kernel_task(RUXNWDDM_PVG *pvg)
{
	struct pvg_display *display = pvg->display;
	volatile ULONG *root = display->task_root.va;
	volatile ULONG *table = display->task_table.va;
	ULONG payload[4];
	ULONG i;
	NTSTATUS status;

	// One level: an entry is the number of the page at that place.
	for (i = 0; i < GLYPH_PAGES; i++)
		table[i] = ruxnwddm_page_number(&display->glyph) + i;
	root[TASK_ROOT_TABLE] = ruxnwddm_page_number(&display->task_table);
	root[TASK_ROOT_DEPTH] = KERNEL_TASK_DEPTH;

	payload[0] = KERNEL_TASK << 1;
	payload[1] = (ULONG)KERNEL_TASK_BYTES;
	payload[2] = (ULONG)(KERNEL_TASK_BYTES >> 32);
	payload[3] = ruxnwddm_page_number(&display->task_root);
	status = display_send(pvg, RUXNPVG_CMD_TASK_DEFINE, payload, 4);
	if (NT_SUCCESS(status))
		InterlockedIncrement64(&pvg->screen.task0_defined);
	return status;
}

static NTSTATUS delete_kernel_task(RUXNWDDM_PVG *pvg)
{
	ULONG id = KERNEL_TASK;
	NTSTATUS status = display_send(pvg, RUXNPVG_CMD_TASK_DELETE, &id, 1);

	if (NT_SUCCESS(status))
		InterlockedIncrement64(&pvg->screen.task0_deleted);
	return status;
}

static NTSTATUS hand_over(RUXNWDDM_PVG *pvg, const RUXNWDDM_PAGES *page)
{
	ULONG payload[2] = { DISPLAY_PORT, page ? ruxnwddm_page_number(page) : 0 };
	NTSTATUS status = display_send(pvg, RUXNPVG_CMD_DISPLAY_STATE_PAGE, payload, 2);

	if (NT_SUCCESS(status))
		InterlockedIncrement64(page ? &pvg->screen.hand_overs : &pvg->screen.let_gos);
	return status;
}

// Who may take the screen now. The caller holds the display's lock.
static NTSTATUS may_take(RUXNWDDM_CLIENT *client)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_display *display = pvg->display;

	if (display->spoiled)
		return STATUS_DEVICE_HARDWARE_ERROR;
	if (display->owner)
		return STATUS_DEVICE_BUSY;
	if (client->dead || !InterlockedCompareExchange(&pvg->task_gate, 0, 0))
		return STATUS_DEVICE_NOT_READY;
	return STATUS_SUCCESS;
}

static void become_owner(RUXNWDDM_CLIENT *client, ULONG mode)
{
	struct pvg_display *display = client->pvg->display;

	display->owner = client;
	display->mode = mode;
	// The display's own client belongs to no process.
	display->owner_process = client->process ?
		(ULONG64)(ULONG_PTR)PsGetProcessId(client->process) : 0;
	InterlockedIncrement64(&client->pvg->screen.takes);
}

NTSTATUS pvg_efb_read(RUXNWDDM_PVG *pvg, struct ruxnpvg_efb *out)
{
	struct pvg_display *display = pvg->display;

	ruxnwddm_lock(&display->lock);
	// While the screen is held the bank is off and reads nothing of use,
	// and what was read before it went off is what will be put back.
	if (display->bank_off) {
		ruxnwddm_unlock(&display->lock);
		return STATUS_DEVICE_BUSY;
	}
	bank_read(pvg, display->bank);
	out->page = display->bank[BANK_PAGE];
	out->bytes = display->bank[BANK_BYTES];
	out->depth = display->bank[BANK_DEPTH];
	out->mode = display->bank[BANK_MODE];
	ruxnwddm_unlock(&display->lock);
	return STATUS_SUCCESS;
}

// The screen held with the early framebuffer off and no display.
NTSTATUS pvg_efb_off(RUXNWDDM_CLIENT *client, struct ruxnpvg_efb *out)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_display *display = pvg->display;
	NTSTATUS status;

	ruxnwddm_lock(&display->lock);
	status = may_take(client);
	if (NT_SUCCESS(status))
		status = bank_off(pvg);
	if (NT_SUCCESS(status))
		become_owner(client, SCREEN_EFB_OFF);
	out->page = display->bank[BANK_PAGE];
	out->bytes = display->bank[BANK_BYTES];
	out->depth = display->bank[BANK_DEPTH];
	out->mode = display->bank[BANK_MODE];
	ruxnwddm_unlock(&display->lock);
	return status;
}

// Takes the display: the early framebuffer off, task 0, the state page,
// the mode list, the event mask.
NTSTATUS pvg_display_take(RUXNWDDM_CLIENT *client)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_display *display = pvg->display;
	NTSTATUS status;

	ruxnwddm_lock(&display->lock);
	status = may_take(client);
	if (!NT_SUCCESS(status))
		goto out;

	status = ruxnwddm_alloc_pages(&display->task_root, RUXNWDDM_PAGE_SIZE);
	if (NT_SUCCESS(status))
		status = ruxnwddm_alloc_pages(&display->task_table, RUXNWDDM_PAGE_SIZE);
	if (NT_SUCCESS(status))
		status = ruxnwddm_alloc_pages(&display->glyph, GLYPH_BYTES);
	// The device reads the page as it gets it, and a mode count it does
	// not like there ends the host's process. The page is zeroed memory.
	if (NT_SUCCESS(status))
		status = ruxnwddm_alloc_pages(&display->state_page, RUXNWDDM_PAGE_SIZE);
	if (!NT_SUCCESS(status)) {
		free_display_pages(display, TRUE);
		goto out;
	}

	status = bank_off(pvg);
	if (!NT_SUCCESS(status)) {
		free_display_pages(display, TRUE);
		goto out;
	}

	status = define_kernel_task(pvg);
	if (!NT_SUCCESS(status)) {
		// Not defined again: the task's table may be the device's now.
		bank_restore(pvg);
		goto out;
	}

	status = hand_over(pvg, &display->state_page);
	if (!NT_SUCCESS(status)) {
		// Never handed over again. Task 0 goes, as in Linux; the state
		// page is kept, since the device may write to it.
		delete_kernel_task(pvg);
		free_display_pages(display, FALSE);
		bank_restore(pvg);
		goto out;
	}

	if (!read_modes(pvg)) {
		// The device names no mode: let go, as Linux does.
		ruxnwddm_record_ulong(L"DisplayNoMode", 1);
		display->spoiled = TRUE;
		status = hand_over(pvg, NULL);
		delete_kernel_task(pvg);
		free_display_pages(display, NT_SUCCESS(status));
		bank_restore(pvg);
		status = STATUS_DEVICE_CONFIGURATION_ERROR;
		goto out;
	}

	// The mask is looked at when an event happens, so a list that came
	// before this write raised nothing; the look after it finds that one.
	state_words(display)[STATE_EVENT_MASK] = EVENT_SWAP_DONE | EVENT_MODES;
	InterlockedExchange(&display->live, 1);
	take_events(pvg);
	become_owner(client, SCREEN_DISPLAY);
	status = STATUS_SUCCESS;
out:
	ruxnwddm_unlock(&display->lock);
	return status;
}

// Shows a surface: one command 8 is one frame on the host. The surface is
// the caller's, under its handle's lock.
NTSTATUS pvg_display_swap(RUXNWDDM_CLIENT *client, struct pvg_surface *surface,
			      ULONG *swap_done)
{
	RUXNWDDM_PVG *pvg = client->pvg;
	struct pvg_display *display = pvg->display;
	ULONG payload[SWAP_WORDS] = { 0 };
	LONG64 done_before;
	NTSTATUS status;

	*swap_done = 0;
	ruxnwddm_lock(&display->lock);
	if (display->owner != client || display->mode != SCREEN_DISPLAY) {
		status = STATUS_ACCESS_DENIED;
		goto out;
	}
	if (display->spoiled) {
		status = STATUS_DEVICE_HARDWARE_ERROR;
		goto out;
	}
	status = pvg_surface_map(pvg, surface);
	if (!NT_SUCCESS(status))
		goto out;

	// What earlier frames left is not this frame's.
	take_events(pvg);
	if (InterlockedCompareExchange(&display->modes_waiting, 0, 0))
		read_modes(pvg);
	if (display->spoiled) {
		status = STATUS_DEVICE_HARDWARE_ERROR;
		goto out;
	}

	done_before = pvg->screen.swap_done;
	payload[0] = DISPLAY_PORT;
	payload[1] = KERNEL_TASK;
	payload[2] = surface->id;
	status = display_send(pvg, RUXNPVG_CMD_DISPLAY_SWAP, payload, SWAP_WORDS);
	if (!NT_SUCCESS(status))
		goto out;
	InterlockedIncrement64(&pvg->screen.swaps);
	display->shown_id = surface->id;
	// The event is set before the stamp is.
	take_events(pvg);
	*swap_done = pvg->screen.swap_done != done_before;
	pvg_surface_reap(pvg);
out:
	ruxnwddm_unlock(&display->lock);
	return status;
}

// Lets the display go and puts the early framebuffer back. The caller
// holds the display's lock.
static void let_go(RUXNWDDM_PVG *pvg)
{
	struct pvg_display *display = pvg->display;
	NTSTATUS let_go_status;

	if (display->mode == SCREEN_DISPLAY) {
		state_words(display)[STATE_EVENT_MASK] = 0;
		// Page number 0 lets the display go; it also unmaps the mapping
		// on show.
		let_go_status = hand_over(pvg, NULL);
		InterlockedExchange(&display->live, 0);
		// The DPC that may be reading the page is waited out.
		KeFlushQueuedDpcs();
		delete_kernel_task(pvg);
		// The device may still write to a page it did not let go.
		free_display_pages(display, NT_SUCCESS(let_go_status));
		display->shown_id = 0;
		pvg_surface_reap(pvg);
	}
	bank_restore(pvg);
	display->owner = NULL;
	display->mode = SCREEN_FREE;
	display->owner_process = 0;
	InterlockedIncrement64(&pvg->screen.releases);
}

NTSTATUS pvg_display_release(RUXNWDDM_CLIENT *client)
{
	struct pvg_display *display = client->pvg->display;
	NTSTATUS status = STATUS_SUCCESS;

	ruxnwddm_lock(&display->lock);
	if (display->owner != client)
		status = STATUS_ACCESS_DENIED;
	else
		let_go(client->pvg);
	ruxnwddm_unlock(&display->lock);
	return status;
}

// The handle's end, in the closing process: the screen goes back if this
// handle held it, before its surfaces go.
void pvg_display_file_closing(RUXNWDDM_CLIENT *client)
{
	struct pvg_display *display = client->pvg->display;

	ruxnwddm_lock(&display->lock);
	if (display->owner == client)
		let_go(client->pvg);
	ruxnwddm_unlock(&display->lock);
}

// Power-down, while the ring still runs: the screen goes back, then every
// surface is unmapped. Their memory stays their handles'.
void pvg_display_power_down(RUXNWDDM_PVG *pvg)
{
	struct pvg_display *display = pvg->display;

	ruxnwddm_lock(&display->lock);
	if (display->owner)
		let_go(pvg);
	ruxnwddm_unlock(&display->lock);
	if (pvg->mapper->started)
		pvg_surfaces_unmap_all(pvg);
}

void pvg_display_info(RUXNWDDM_PVG *pvg, struct ruxnpvg_display_info *info)
{
	struct pvg_display *display = pvg->display;
	RUXNWDDM_SCREEN_COUNTS *c = &pvg->screen;

	RtlZeroMemory(info, sizeof(*info));
	info->mapper_started = (ULONG64)c->mapper_started;
	info->mapper_commands = (ULONG64)c->mapper_commands;
	info->mapper_irqs = (ULONG64)c->mapper_irqs;
	info->mapper_early = (ULONG64)c->mapper_early;
	info->mapper_late = (ULONG64)c->mapper_late;
	info->mapper_timeouts = (ULONG64)c->mapper_timeouts;
	info->maps = (ULONG64)c->maps;
	info->unmaps = (ULONG64)c->unmaps;
	info->outcomes_busy = (ULONG64)c->outcomes_busy;
	info->outcomes_other = (ULONG64)c->outcomes_other;
	info->ids_passed = (ULONG64)c->ids_passed;
	info->mapped_now = (ULONG64)c->mapped_now;
	info->held_now = (ULONG64)c->held_now;
	info->mapper_written = pvg->mapper->written;
	info->surfaces = (ULONG64)c->surfaces;
	info->surfaces_freed = (ULONG64)c->surfaces_freed;
	info->surfaces_kept = (ULONG64)c->surfaces_kept;
	info->surface_bytes = (ULONG64)c->surface_bytes;
	info->takes = (ULONG64)c->takes;
	info->releases = (ULONG64)c->releases;
	info->task0_defined = (ULONG64)c->task0_defined;
	info->task0_deleted = (ULONG64)c->task0_deleted;
	info->hand_overs = (ULONG64)c->hand_overs;
	info->let_gos = (ULONG64)c->let_gos;
	info->mode_lists = (ULONG64)c->mode_lists;
	info->acknowledgements = (ULONG64)c->acknowledgements;
	info->swaps = (ULONG64)c->swaps;
	info->swap_done = (ULONG64)c->swap_done;
	info->events_taken = (ULONG64)c->events_taken;
	info->display_failed = (ULONG64)c->display_failed;
	info->bank_reads = (ULONG64)c->bank_reads;
	info->bank_offs = (ULONG64)c->bank_offs;
	info->bank_restores = (ULONG64)c->bank_restores;
	info->exports = (ULONG64)c->exports;
	info->imports = (ULONG64)c->imports;
	info->imports_refused = (ULONG64)c->imports_refused;
	info->import_releases = (ULONG64)c->import_releases;
	info->exported_now = (ULONG64)c->exported_now;
	info->holders_now = (ULONG64)c->holders_now;
	info->outlived_maker = (ULONG64)c->outlived_maker;

	ruxnwddm_lock(&display->lock);
	info->owner_process = display->owner_process;
	info->owner_mode = display->mode;
	info->spoiled = display->spoiled;
	info->shown_id = display->shown_id;
	info->first_mode = display->first_mode;
	info->bank.page = display->bank[BANK_PAGE];
	info->bank.bytes = display->bank[BANK_BYTES];
	info->bank.depth = display->bank[BANK_DEPTH];
	info->bank.mode = display->bank[BANK_MODE];
	ruxnwddm_unlock(&display->lock);
}
