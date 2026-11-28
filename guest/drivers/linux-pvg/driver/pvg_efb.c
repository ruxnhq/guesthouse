// SPDX-License-Identifier: GPL-2.0
/*
 * Scanout through the device's early framebuffer.
 *
 * The device shows one block of guest memory as the picture, with no command
 * involved: a bank of registers names the block and a mode from the device's
 * own list. The block has to be contiguous, so the largest mode this path
 * can show is what the page allocator gives in one piece.
 */

#include <linux/mmzone.h>
#include <linux/slab.h>

#include "pvg.h"

#define PVG_EFB_MAX_BYTES	(PAGE_SIZE << MAX_PAGE_ORDER)

struct pvg_efb {
	struct page *pages;
	size_t bytes;
};

static size_t pvg_efb_mode_bytes(const struct pvg_mode *mode)
{
	return ALIGN((size_t)mode->width * mode->height * 4, PVG_PAGE_SIZE);
}

static int pvg_efb_init(struct pvg_device *pvg)
{
	struct pvg_scanout *scanout = &pvg->scanout;
	struct pvg_efb *efb;
	u32 count, align, i;

	efb = kzalloc(sizeof(*efb), GFP_KERNEL);
	if (!efb)
		return -ENOMEM;

	/* The bank reads zeroes until the early framebuffer is on. */
	pvg_write(pvg, RUXNPVG_REG_EFB_ON, 1);
	count = pvg_read(pvg, RUXNPVG_REG_EFB_MODE_COUNT);
	align = pvg_read(pvg, RUXNPVG_REG_EFB_ROW_ALIGN);
	for (i = 0; i < count && scanout->mode_count < PVG_MAX_MODES; i++) {
		struct pvg_mode mode = { };
		u32 value;

		pvg_write(pvg, RUXNPVG_REG_EFB_MODE_INDEX, i);
		value = pvg_read(pvg, RUXNPVG_REG_EFB_MODE_SIZE);
		if (value == U32_MAX)
			break;
		mode.width = value & 0xffff;
		mode.height = value >> 16;
		if (!mode.width || !mode.height)
			continue;
		if (pvg_efb_mode_bytes(&mode) > PVG_EFB_MAX_BYTES ||
		    (align && (mode.width * 4) % align)) {
			dev_info(&pvg->pdev->dev,
				 "mode %ux%u left out: no room for it in one block\n",
				 mode.width, mode.height);
			continue;
		}
		dev_info(&pvg->pdev->dev, "early framebuffer: mode %u: %ux%u\n", i,
			 mode.width, mode.height);
		scanout->modes[scanout->mode_count++] = mode;
		efb->bytes = max(efb->bytes, pvg_efb_mode_bytes(&mode));
	}
	pvg_write(pvg, RUXNPVG_REG_EFB_ON, 0);

	if (!scanout->mode_count) {
		kfree(efb);
		return -ENODEV;
	}

	/* Taken once and kept: a block this large is hard to get later. */
	efb->pages = pvg_alloc_pages(pvg, efb->bytes);
	if (!efb->pages) {
		kfree(efb);
		return -ENOMEM;
	}
	scanout->backend = efb;
	return 0;
}

static void pvg_efb_fini(struct pvg_device *pvg)
{
	struct pvg_efb *efb = pvg->scanout.backend;

	/* The device must stop reading the block before it is given back. */
	pvg_write(pvg, RUXNPVG_REG_EFB_ON, 0);
	pvg_free_pages(efb->pages, efb->bytes);
	kfree(efb);
	pvg->scanout.backend = NULL;
}

static int pvg_efb_enable(struct pvg_device *pvg, const struct pvg_mode *mode)
{
	struct pvg_scanout *scanout = &pvg->scanout;
	struct pvg_efb *efb = scanout->backend;

	if (pvg_efb_mode_bytes(mode) > efb->bytes)
		return -EINVAL;

	memset(page_address(efb->pages), 0, pvg_efb_mode_bytes(mode));

	/* The picture appears with the last of these writes. */
	pvg_write(pvg, RUXNPVG_REG_EFB_ON, 1);
	pvg_write(pvg, RUXNPVG_REG_EFB_PAGE, pvg_page_number(efb->pages));
	pvg_write(pvg, RUXNPVG_REG_EFB_BYTES, pvg_efb_mode_bytes(mode));
	pvg_write(pvg, RUXNPVG_REG_EFB_DEPTH, RUXNPVG_EFB_DEPTH);
	pvg_write(pvg, RUXNPVG_REG_EFB_MODE, (u32)mode->height << 16 | mode->width);

	scanout->pixels = page_address(efb->pages);
	scanout->pitch = mode->width * 4;
	return 0;
}

static void pvg_efb_disable(struct pvg_device *pvg)
{
	pvg_write(pvg, RUXNPVG_REG_EFB_ON, 0);
	pvg->scanout.pixels = NULL;
}

/* The device reads the block for every frame it shows, so nothing to do. */
static int pvg_efb_flush(struct pvg_device *pvg, struct pvg_surface *surface)
{
	return 0;
}

const struct pvg_scanout_ops pvg_efb_ops = {
	.name = "early framebuffer",
	.key = "efb",
	.init = pvg_efb_init,
	.fini = pvg_efb_fini,
	.enable = pvg_efb_enable,
	.disable = pvg_efb_disable,
	.flush = pvg_efb_flush,
};
