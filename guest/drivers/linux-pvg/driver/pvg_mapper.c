// SPDX-License-Identifier: GPL-2.0
/*
 * Scanout through the IOSurface mapper.
 *
 * The picture is a surface of device pages that need not lie together, so
 * its size is not bound by what the page allocator gives in one piece. The
 * mapper's device gives the surface a mapping id, and a display command
 * swaps the display to that id. There is no command that sets a mode: the
 * display takes the size of the surface it is swapped to.
 *
 * A frame is either a buffer's own surface, shown as it is, or the path's
 * own surface, into which the DRM side has copied. The path's own is made
 * when a picture is first copied in a mode and given up when a buffer's is
 * shown, so that it costs no memory while it is of no use.
 *
 * Observed, and what the order of things below follows from: the device
 * does not unmap the mapping a display shows, and no command blanks a
 * display.
 */

#include <linux/slab.h>

#include "pvg.h"

struct pvg_mapper_scanout {
	struct pvg_mode mode;		/* what the display was last turned to */
	struct pvg_surface *copy;	/* the path's own surface, if there is one */
	struct pvg_surface *old;	/* one of the path's that is still on show */
	u32 shown;			/* the mapping id the display was last swapped to */
};

static int pvg_mapper_scanout_init(struct pvg_device *pvg)
{
	struct pvg_mapper_scanout *backend;
	int ret;

	backend = kzalloc(sizeof(*backend), GFP_KERNEL);
	if (!backend)
		return -ENOMEM;

	ret = pvg_mapper_init(pvg);
	if (ret)
		goto err;
	ret = pvg_display_init(pvg);
	if (ret)
		goto err_mapper;
	pvg->scanout.backend = backend;
	return 0;

err_mapper:
	pvg_mapper_fini(pvg);
err:
	kfree(backend);
	return ret;
}

static void pvg_mapper_scanout_fini(struct pvg_device *pvg)
{
	struct pvg_mapper_scanout *backend = pvg->scanout.backend;

	/* Letting the display go is what lets every surface be unmapped. */
	pvg_display_fini(pvg);
	pvg_surface_destroy(pvg, backend->copy);
	pvg_surface_destroy(pvg, backend->old);
	pvg_mapper_fini(pvg);
	kfree(backend);
	pvg->scanout.backend = NULL;
	pvg->scanout.pixels = NULL;
}

/* Gives up the path's own surface, now or once the display has left it. */
static void pvg_mapper_scanout_drop_copy(struct pvg_device *pvg)
{
	struct pvg_mapper_scanout *backend = pvg->scanout.backend;
	struct pvg_surface *copy = backend->copy;

	if (!copy)
		return;
	backend->copy = NULL;
	pvg->scanout.pixels = NULL;
	if (copy->id == backend->shown) {
		/* Only one surface is on show, so an older one is not. */
		pvg_surface_destroy(pvg, backend->old);
		backend->old = copy;
	} else {
		pvg_surface_destroy(pvg, copy);
	}
}

/* A swap to a surface of another size is the mode set, so nothing is sent. */
static int pvg_mapper_scanout_enable(struct pvg_device *pvg,
				     const struct pvg_mode *mode)
{
	struct pvg_mapper_scanout *backend = pvg->scanout.backend;

	backend->mode = *mode;
	return 0;
}

static int pvg_mapper_scanout_pixels(struct pvg_device *pvg)
{
	struct pvg_scanout *scanout = &pvg->scanout;
	struct pvg_mapper_scanout *backend = scanout->backend;
	struct pvg_surface *surface = backend->copy;
	int ret;

	if (surface && surface->width == backend->mode.width &&
	    surface->height == backend->mode.height)
		return 0;
	pvg_mapper_scanout_drop_copy(pvg);

	surface = pvg_surface_create(pvg, backend->mode.width, backend->mode.height);
	if (IS_ERR(surface))
		return PTR_ERR(surface);
	ret = pvg_surface_map(pvg, surface);
	if (ret) {
		pvg_surface_destroy(pvg, surface);
		return ret;
	}
	backend->copy = surface;
	scanout->pixels = surface->vaddr;
	scanout->pitch = surface->pitch;
	scanout->pixels_fresh = true;
	return 0;
}

static int pvg_mapper_scanout_flush(struct pvg_device *pvg,
				    struct pvg_surface *surface)
{
	struct pvg_mapper_scanout *backend = pvg->scanout.backend;
	struct pvg_surface *target = surface ?: backend->copy;
	int ret;

	if (!target || !target->id)
		return -ENODEV;
	ret = pvg_display_swap(pvg, target->id);
	if (ret)
		return ret;
	backend->shown = target->id;

	/* What the display has left can be unmapped now. */
	if (backend->old && backend->old->id != backend->shown) {
		pvg_surface_destroy(pvg, backend->old);
		backend->old = NULL;
	}
	if (surface)
		pvg_mapper_scanout_drop_copy(pvg);
	pvg_surface_reap(pvg);
	return 0;
}

/*
 * The nearest thing to a display turned off: a black picture of the path's
 * own. It also takes the display off a buffer's surface, which the buffer's
 * owner may be about to free.
 */
static void pvg_mapper_scanout_disable(struct pvg_device *pvg)
{
	struct pvg_mapper_scanout *backend = pvg->scanout.backend;

	if (pvg_mapper_scanout_pixels(pvg)) {
		dev_warn(&pvg->pdev->dev, "no memory for a black picture\n");
		return;
	}
	memset(backend->copy->vaddr, 0, backend->copy->bytes);
	pvg->scanout.pixels_fresh = true;
	pvg_mapper_scanout_flush(pvg, NULL);
}

const struct pvg_scanout_ops pvg_mapper_ops = {
	.name = "IOSurface mapper",
	.key = "mapper",
	.any_size = true,
	.reports_frames = true,
	.surfaces = true,
	.cursor = true,
	.init = pvg_mapper_scanout_init,
	.fini = pvg_mapper_scanout_fini,
	.enable = pvg_mapper_scanout_enable,
	.disable = pvg_mapper_scanout_disable,
	.pixels = pvg_mapper_scanout_pixels,
	.flush = pvg_mapper_scanout_flush,
};
