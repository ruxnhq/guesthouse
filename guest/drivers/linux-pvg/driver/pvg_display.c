// SPDX-License-Identifier: GPL-2.0
/*
 * A display of the device: its state page, its modes and events, and the
 * commands that show a mapped surface on it.
 *
 * The guest hands the display a page of its memory. The device writes the
 * display's modes there and sets event bits, the guest writes a mask for
 * the events and where the cursor is. Display commands go through the
 * first ring and name the kernel task, which therefore has to be defined.
 *
 * The cursor is the display's too: a picture the device reads through the
 * kernel task's page table, and a place and a flag in the state page.
 *
 * All of this is observed.
 */

#include <linux/interrupt.h>
#include <linux/slab.h>
#include <linux/workqueue.h>

#include "pvg.h"

/*
 * The driver uses the first display only, RUXNPVG_DISPLAY_PORT. The kernel
 * task's page table is read once, when the task is defined, so what the
 * device is to read through it is in the table before.
 *
 * The cursor's picture has its own memory at the start of the kernel task,
 * large enough for the largest the device takes (RUXNPVG_GLYPH_MOST_BYTES).
 * A host that takes the picture reads it through the table, and its process
 * ends if the table does not map all of what a command names; so the block
 * is in the table from the start, and no command names anything else.
 *
 * The state page is an array of 32-bit words here, so its offsets
 * (RUXNPVG_STATE_...) are divided by four.
 */

struct pvg_display {
	struct pvg_device *pvg;
	struct page *task_root_page;
	struct page *task_table_page;
	struct page *state_page;
	struct page *glyph_pages;
	__le32 *state;
	__le32 *glyph;
	struct work_struct modes_work;
	u32 sequence;
	u64 swaps;
	u64 mode_lists;
};

/*
 * Reads the mode list and acknowledges it, if there is one the device waits
 * to have acknowledged. It holds a newer list back until then, so the page
 * does not change while it is read here, and it refuses an acknowledgement
 * it does not wait for. Returns whether a list was read.
 */
static bool pvg_display_read_modes(struct pvg_device *pvg)
{
	struct pvg_display *display = pvg->display;
	struct pvg_scanout *scanout = &pvg->scanout;
	u32 payload[2] = { RUXNPVG_DISPLAY_PORT };
	u32 count, i, taken = 0;
	int ret;

	if (le32_to_cpu(READ_ONCE(display->state[RUXNPVG_STATE_LIST_STATE / 4])) !=
	    RUXNPVG_LIST_WAITING)
		return false;

	mutex_lock(&scanout->modes_lock);
	display->sequence = le32_to_cpu(READ_ONCE(display->state[RUXNPVG_STATE_SEQUENCE / 4]));
	count = le32_to_cpu(READ_ONCE(display->state[RUXNPVG_STATE_MODE_COUNT / 4])) & 0xffff;
	for (i = 0; i < count && taken < PVG_MAX_MODES; i++) {
		const __le32 *entry = display->state + (RUXNPVG_STATE_MODES / 4) +
				      i * (RUXNPVG_STATE_MODE_BYTES / 4);
		u32 size = le32_to_cpu(READ_ONCE(entry[0]));
		/* The refresh rate is 16.16 fixed point. */
		u32 refresh = le32_to_cpu(READ_ONCE(entry[1]));
		struct pvg_mode mode = {
			.width = size & 0xffff,
			.height = size >> 16,
			.refresh = (refresh + 0x8000) >> 16,
		};

		if (!mode.width || !mode.height)
			continue;
		dev_info(&pvg->pdev->dev, "display: mode %u: %ux%u at %u Hz (%#x)\n",
			 i, mode.width, mode.height, mode.refresh, refresh);
		/* Seen on an M5 Mac: the host shows such a surface sheared. */
		if (mode.width % 4)
			dev_info(&pvg->pdev->dev,
				 "display: mode %u is %u wide, no multiple of 4: the host shows it sheared\n",
				 i, mode.width);
		scanout->modes[taken++] = mode;
	}
	scanout->mode_count = taken;
	mutex_unlock(&scanout->modes_lock);
	display->mode_lists++;

	dev_info(&pvg->pdev->dev, "display: mode list %u has %u modes, %u taken\n",
		 display->sequence, count, taken);

	payload[1] = display->sequence;
	ret = pvg_ring_send(pvg, RUXNPVG_CMD_DISPLAY_ACK, payload, ARRAY_SIZE(payload));
	if (ret)
		dev_warn(&pvg->pdev->dev, "display: mode list %u not acknowledged: %d\n",
			 display->sequence, ret);
	return true;
}

/* The device has put a new mode list into the state page. */
static void pvg_display_modes_work(struct work_struct *work)
{
	struct pvg_display *display =
		container_of(work, struct pvg_display, modes_work);

	if (pvg_display_read_modes(display->pvg))
		pvg_kms_modes_changed(display->pvg);
}

/*
 * Takes the display's events and clears them. Called from the interrupt
 * handler when the device says a display has events, and by a sender that
 * has seen its swap stamped, since an interrupt can be late.
 */
static void pvg_display_events(struct pvg_device *pvg, bool from_irq)
{
	struct pvg_display *display = pvg->display;
	u32 events = le32_to_cpu(xchg(&display->state[RUXNPVG_STATE_EVENTS / 4], 0));

	if (events & RUXNPVG_EVENT_SWAP_DONE)
		pvg_kms_frame_shown(pvg, from_irq);
	/*
	 * The event bit can be lost: the device sets it and the guest clears
	 * it with nothing between them to keep one from undoing the other.
	 * The page's own word for a list that waits can not.
	 */
	if ((events & RUXNPVG_EVENT_MODES) ||
	    le32_to_cpu(READ_ONCE(display->state[RUXNPVG_STATE_LIST_STATE / 4])) ==
	    RUXNPVG_LIST_WAITING)
		schedule_work(&display->modes_work);
}

/* For the debugfs instrument: the state page, if the display has one. */
__le32 *pvg_display_state(struct pvg_device *pvg)
{
	return pvg->display ? pvg->display->state : NULL;
}

void pvg_display_irq(struct pvg_device *pvg)
{
	pvg_display_events(pvg, true);
}

/*
 * Shows a mapping on the display. The device shows a frame for each of
 * these and for nothing else: pixels changed in the mapping on show reach
 * the host's screen only with the next swap, which may name the same id.
 */
int pvg_display_swap(struct pvg_device *pvg, u32 mapping)
{
	/*
	 * The port, the task, the mapping's id in 64 bits, and six words for
	 * a gamma table, which this driver has none of.
	 */
	u32 payload[10] = { RUXNPVG_DISPLAY_PORT, RUXNPVG_KERNEL_TASK, mapping };
	int ret;

	/* What earlier frames left is not this frame's. */
	pvg_display_events(pvg, false);
	pvg_kms_frame_kicked(pvg);
	ret = pvg_ring_send(pvg, RUXNPVG_CMD_DISPLAY_SWAP, payload, ARRAY_SIZE(payload));
	pvg->display->swaps++;
	/* The event is set before the stamp is. */
	pvg_display_events(pvg, false);
	return ret;
}

/*
 * Hands the display a picture for its cursor: rows of four bytes a pixel,
 * blue first, alpha last. The command carries the sum of the picture's
 * words, which a host that takes the picture checks.
 */
int pvg_display_cursor_glyph(struct pvg_device *pvg, const void *pixels,
			     unsigned int pitch, u32 width, u32 height,
			     u32 hot_x, u32 hot_y)
{
	struct pvg_display *display = pvg->display;
	u32 stride = width * 4, bytes = stride * height;
	u32 payload[11] = {
		RUXNPVG_DISPLAY_PORT, RUXNPVG_KERNEL_TASK,
		lower_32_bits(RUXNPVG_GLYPH_ADDRESS), upper_32_bits(RUXNPVG_GLYPH_ADDRESS),
		bytes, 0,	/* how much of the task the picture takes */
		stride, 0,
		height << 16 | width,
	};
	u32 i, sum = 0;

	if (!width || !height || width > RUXNPVG_CURSOR_MAX_SIDE ||
	    height > RUXNPVG_CURSOR_MAX_SIDE)
		return -EINVAL;
	payload[9] = min(hot_y, height - 1) << 16 | min(hot_x, width - 1);

	/* The rows lie one behind the other with no gap. */
	for (i = 0; i < height; i++)
		memcpy(display->glyph + i * width, pixels + i * pitch, stride);
	for (i = 0; i < width * height; i++)
		sum += le32_to_cpu(display->glyph[i]);
	payload[10] = sum;

	pvg->cursors.glyphs++;
	return pvg_ring_send(pvg, RUXNPVG_CMD_CURSOR_GLYPH, payload,
			     ARRAY_SIZE(payload));
}

/*
 * Shows or hides the cursor. The command does it at once, and the word in
 * the state page keeps it so when the device next looks at the page.
 */
int pvg_display_cursor_show(struct pvg_device *pvg, bool show)
{
	struct pvg_display *display = pvg->display;
	u32 payload[2] = { RUXNPVG_DISPLAY_PORT, show };

	WRITE_ONCE(display->state[RUXNPVG_STATE_CURSOR_SHOWN / 4], cpu_to_le32(show));
	pvg->cursors.shows++;
	return pvg_ring_send(pvg, RUXNPVG_CMD_CURSOR_SHOW, payload,
			     ARRAY_SIZE(payload));
}

/* Puts the cursor's place into the state page and has the device look. */
void pvg_display_cursor_move(struct pvg_device *pvg, u32 x, u32 y)
{
	struct pvg_display *display = pvg->display;

	WRITE_ONCE(display->state[RUXNPVG_STATE_CURSOR_PLACE / 4],
		   cpu_to_le32(min(y, U16_MAX) << 16 | min(x, U16_MAX)));
	pvg->cursors.moves++;
	pvg_write(pvg, RUXNPVG_REG_DISPLAY_LOOK, RUXNPVG_DISPLAY_PORT);
}

static int pvg_display_define_task(struct pvg_device *pvg)
{
	struct pvg_display *display = pvg->display;
	__le32 *root = page_address(display->task_root_page);
	__le32 *table = page_address(display->task_table_page);
	u32 payload[4] = {
		RUXNPVG_KERNEL_TASK << 1,
		lower_32_bits(RUXNPVG_KERNEL_TASK_BYTES),
		upper_32_bits(RUXNPVG_KERNEL_TASK_BYTES),
		pvg_page_number(display->task_root_page),
	};
	u32 id = RUXNPVG_KERNEL_TASK;
	unsigned int i;
	int ret;

	/* One level: an entry is the number of the page at that place. */
	for (i = 0; i < RUXNPVG_GLYPH_MOST_BYTES >> RUXNPVG_PAGE_SHIFT; i++)
		table[(RUXNPVG_GLYPH_ADDRESS >> RUXNPVG_PAGE_SHIFT) + i] =
			cpu_to_le32(pvg_page_number(display->glyph_pages) + i);
	root[RUXNPVG_TASK_ROOT_TABLE] = cpu_to_le32(pvg_page_number(display->task_table_page));
	root[RUXNPVG_TASK_ROOT_DEPTH] = cpu_to_le32(1);

	ret = pvg_ring_send(pvg, RUXNPVG_CMD_TASK_DEFINE, payload, ARRAY_SIZE(payload));
	if (ret != -EIO)
		return ret;
	if (READ_ONCE(pvg_test_no_retry)) {
		dev_info(&pvg->pdev->dev, "display: command 56 refused, not tried again (test_no_retry)\n");
		return ret;
	}
	/* A driver before this one may have left the task defined. */
	dev_info(&pvg->pdev->dev, "display: the kernel task is there already, defining it anew\n");
	ret = pvg_ring_send(pvg, RUXNPVG_CMD_TASK_DELETE, &id, 1);
	if (ret)
		return ret;
	return pvg_ring_send(pvg, RUXNPVG_CMD_TASK_DEFINE, payload, ARRAY_SIZE(payload));
}

static int pvg_display_hand_over(struct pvg_device *pvg, struct page *page)
{
	u32 payload[2] = { RUXNPVG_DISPLAY_PORT, page ? pvg_page_number(page) : 0 };

	return pvg_ring_send(pvg, RUXNPVG_CMD_DISPLAY_STATE_PAGE, payload,
			     ARRAY_SIZE(payload));
}

int pvg_display_init(struct pvg_device *pvg)
{
	struct pvg_display *display;
	int ret;

	display = kzalloc(sizeof(*display), GFP_KERNEL);
	if (!display)
		return -ENOMEM;
	display->pvg = pvg;
	INIT_WORK(&display->modes_work, pvg_display_modes_work);
	pvg->display = display;

	display->task_root_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	display->task_table_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	display->state_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	display->glyph_pages = pvg_alloc_pages(pvg, RUXNPVG_GLYPH_MOST_BYTES);
	if (!display->task_root_page || !display->task_table_page ||
	    !display->state_page || !display->glyph_pages) {
		ret = -ENOMEM;
		goto err;
	}
	display->state = page_address(display->state_page);
	display->glyph = page_address(display->glyph_pages);

	ret = pvg_display_define_task(pvg);
	if (ret) {
		dev_err(&pvg->pdev->dev, "display: no kernel task: %d\n", ret);
		goto err;
	}

	/*
	 * The device reads the page as it gets it, and a mode count it does
	 * not like there ends the host's process. The page is zeroed memory.
	 */
	ret = pvg_display_hand_over(pvg, display->state_page);
	if (ret == -EIO) {
		/* Still in use from a driver before this one: let go, take. */
		dev_info(&pvg->pdev->dev, "display: in use already, taking it over\n");
		ret = pvg_display_hand_over(pvg, NULL);
		if (!ret) {
			memset(display->state, 0, PVG_PAGE_SIZE);
			ret = pvg_display_hand_over(pvg, display->state_page);
		}
	}
	if (ret) {
		dev_err(&pvg->pdev->dev, "display: state page not taken: %d\n", ret);
		goto err_task;
	}

	pvg_display_read_modes(pvg);
	if (!pvg->scanout.mode_count) {
		dev_err(&pvg->pdev->dev, "display: the device names no mode\n");
		ret = -ENODEV;
		goto err_page;
	}

	/*
	 * The mask is looked at when an event happens, so a list that came
	 * before this write raised nothing. The look after it finds that one.
	 */
	WRITE_ONCE(display->state[RUXNPVG_STATE_EVENT_MASK / 4],
		   cpu_to_le32(RUXNPVG_EVENT_SWAP_DONE | RUXNPVG_EVENT_MODES));
	WRITE_ONCE(pvg->display_live, true);
	pvg_display_events(pvg, false);
	return 0;

err_page:
	pvg_display_hand_over(pvg, NULL);
err_task:
	{
		u32 id = RUXNPVG_KERNEL_TASK;

		pvg_ring_send(pvg, RUXNPVG_CMD_TASK_DELETE, &id, 1);
	}
err:
	pvg_free_pages(display->glyph_pages, RUXNPVG_GLYPH_MOST_BYTES);
	pvg_free_pages(display->state_page, PVG_PAGE_SIZE);
	pvg_free_pages(display->task_table_page, PVG_PAGE_SIZE);
	pvg_free_pages(display->task_root_page, PVG_PAGE_SIZE);
	pvg->display = NULL;
	kfree(display);
	return ret;
}

void pvg_display_fini(struct pvg_device *pvg)
{
	struct pvg_display *display = pvg->display;
	u32 id = RUXNPVG_KERNEL_TASK;
	int ret;

	if (!display)
		return;

	WRITE_ONCE(display->state[RUXNPVG_STATE_EVENT_MASK / 4], 0);
	if (display->state[RUXNPVG_STATE_CURSOR_SHOWN / 4])
		pvg_display_cursor_show(pvg, false);
	/* Page number 0 lets the display go, so the page can be given back. */
	ret = pvg_display_hand_over(pvg, NULL);
	if (ret)
		dev_warn(&pvg->pdev->dev, "display: not let go: %d\n", ret);
	WRITE_ONCE(pvg->display_live, false);
	synchronize_irq(pci_irq_vector(pvg->pdev, 0));
	cancel_work_sync(&display->modes_work);

	if (READ_ONCE(pvg_test_leave)) {
		/* The device keeps the task, and with it these pages. */
		dev_info(&pvg->pdev->dev, "display: the kernel task left defined (test_leave)\n");
		pvg->display = NULL;
		kfree(display);
		return;
	}
	if (pvg_ring_send(pvg, RUXNPVG_CMD_TASK_DELETE, &id, 1))
		dev_warn(&pvg->pdev->dev, "display: the kernel task stays defined\n");

	dev_info(&pvg->pdev->dev, "display: %llu swaps, %llu mode lists\n",
		 display->swaps, display->mode_lists);
	if (ret) {
		/* The device may still write to the page. */
		display->state_page = NULL;
	}
	pvg_free_pages(display->glyph_pages, RUXNPVG_GLYPH_MOST_BYTES);
	pvg_free_pages(display->state_page, PVG_PAGE_SIZE);
	pvg_free_pages(display->task_table_page, PVG_PAGE_SIZE);
	pvg_free_pages(display->task_root_page, PVG_PAGE_SIZE);
	pvg->display = NULL;
	kfree(display);
}
