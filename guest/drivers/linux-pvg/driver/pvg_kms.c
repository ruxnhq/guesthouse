// SPDX-License-Identifier: GPL-2.0
/*
 * The DRM side: one plane, one CRTC, one connector.
 *
 * A frame reaches the device in one of two ways. A buffer that is a surface
 * (pvg_gem.c) and fills the screen is shown from its own memory. Any other,
 * the kernel's console among them, is a GEM buffer of ordinary pages, and
 * what changed in it is copied into memory the scanout path owns. This file
 * knows nothing of how either reaches the device: that is the path behind
 * struct pvg_scanout_ops.
 */

#include <linux/dma-resv.h>
#include <linux/moduleparam.h>

#include <drm/clients/drm_client_setup.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_state_helper.h>
#include <drm/drm_connector.h>
#include <drm/drm_crtc_helper.h>
#include <drm/drm_damage_helper.h>
#include <drm/drm_drv.h>
#include <drm/drm_encoder.h>
#include <drm/drm_fbdev_shmem.h>
#include <drm/drm_format_helper.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_gem_atomic_helper.h>
#include <drm/drm_gem_framebuffer_helper.h>
#include <drm/drm_gem_shmem_helper.h>
#include <drm/drm_managed.h>
#include <drm/drm_modes.h>
#include <drm/drm_modeset_helper_vtables.h>
#include <drm/drm_print.h>
#include <drm/drm_probe_helper.h>
#include <drm/drm_property.h>
#include <drm/drm_vblank.h>
#include <drm/drm_vblank_helper.h>

#include "pvg.h"

/*
 * The device shows four bytes a pixel: blue, green, red, and a fourth it
 * calls alpha. Buffers are taken with alpha or without, and the fourth byte
 * is not shown, since nothing lies behind the plane. A picture that is
 * copied gets the fourth byte set on the way; a buffer shown from its own
 * memory has there what its owner wrote, which was not seen to change the
 * picture.
 */
#define PVG_SCANOUT_FORMAT	DRM_FORMAT_ARGB8888

/*
 * What ends a page flip where the device reports its frames. The device has
 * no vblank: it shows a frame when it is told of one and says so at once.
 * Taken alone that word lets user space flip as fast as the device answers,
 * so the default holds the flip's event until the vblank timer's next tick,
 * which is the pace a program that flips in a loop expects.
 */
enum {
	PVG_FLIP_TIMER,		/* the vblank timer, as if the device said nothing */
	PVG_FLIP_SHOWN,		/* the device's word that it has shown the frame */
	PVG_FLIP_SHOWN_PACED,	/* that word, and then the timer's next tick */
};

static unsigned int flip = PVG_FLIP_SHOWN_PACED;
module_param(flip, uint, 0644);
MODULE_PARM_DESC(flip,
		 "What ends a page flip: 0 the vblank timer, 1 the device's word that the frame is shown, 2 that word and then the timer's next tick (default)");

/* How long the device's word is waited for before the flip ends without it. */
#define PVG_FLIP_TIMEOUT	msecs_to_jiffies(100)

static const u32 pvg_plane_formats[] = {
	DRM_FORMAT_XRGB8888,
	/* Version 3: what a compositor's clients draw. */
	DRM_FORMAT_ARGB8888,
};

static int pvg_plane_atomic_check(struct drm_plane *plane,
				  struct drm_atomic_state *state)
{
	struct drm_plane_state *plane_state =
		drm_atomic_get_new_plane_state(state, plane);
	struct drm_crtc_state *crtc_state = NULL;

	if (plane_state->crtc)
		crtc_state = drm_atomic_get_new_crtc_state(state, plane_state->crtc);

	return drm_atomic_helper_check_plane_state(plane_state, crtc_state,
						   DRM_PLANE_NO_SCALING,
						   DRM_PLANE_NO_SCALING,
						   false, false);
}

/*
 * The buffer's surface, if the frame can be shown from it as it is: the
 * display takes a surface's size and shows all of it, so the framebuffer
 * has to be the whole surface and the plane the whole framebuffer.
 */
static struct pvg_surface *pvg_plane_surface(struct pvg_device *pvg,
					     const struct drm_plane_state *plane_state)
{
	const struct drm_framebuffer *fb = plane_state->fb;
	struct pvg_surface *surface;

	if (!fb || !pvg->scanout.ops->surfaces)
		return NULL;
	surface = pvg_gem_surface(fb->obj[0]);
	if (!surface)
		return NULL;
	if (fb->offsets[0] || fb->pitches[0] != surface->pitch ||
	    fb->width != surface->width || fb->height != surface->height ||
	    plane_state->src_x || plane_state->src_y ||
	    plane_state->src_w >> 16 != fb->width ||
	    plane_state->src_h >> 16 != fb->height)
		return NULL;
	return surface;
}

/*
 * A buffer's surface gets its mapping id when it is first to be shown, and
 * keeps it until the buffer is freed.
 */
static int pvg_plane_prepare_fb(struct drm_plane *plane,
				struct drm_plane_state *plane_state)
{
	struct pvg_device *pvg = to_pvg(plane->dev);
	struct pvg_surface *surface;
	int idx, ret;

	ret = drm_gem_plane_helper_prepare_fb(plane, plane_state);
	if (ret)
		return ret;
	surface = pvg_plane_surface(pvg, plane_state);
	if (!surface)
		return 0;
	if (!drm_dev_enter(plane->dev, &idx))
		return -ENODEV;
	ret = pvg_surface_map(pvg, surface);
	drm_dev_exit(idx);
	return ret;
}

static void pvg_plane_atomic_update(struct drm_plane *plane,
				    struct drm_atomic_state *state)
{
	struct pvg_device *pvg = to_pvg(plane->dev);
	struct pvg_scanout *scanout = &pvg->scanout;
	struct drm_plane_state *plane_state =
		drm_atomic_get_new_plane_state(state, plane);
	struct drm_plane_state *old_plane_state =
		drm_atomic_get_old_plane_state(state, plane);
	struct drm_shadow_plane_state *shadow =
		to_drm_shadow_plane_state(plane_state);
	struct drm_framebuffer *fb = plane_state->fb;
	struct pvg_surface *surface;
	struct drm_atomic_helper_damage_iter iter;
	struct drm_rect damage, shown;
	int idx;

	if (!fb || !plane_state->visible)
		return;
	if (!drm_dev_enter(plane->dev, &idx))
		return;

	/* The part of the framebuffer that is the picture. */
	drm_rect_fp_to_int(&shown, &plane_state->src);
	surface = pvg_plane_surface(pvg, plane_state);
	if (surface && !surface->id)
		surface = NULL;
	if (!surface && scanout->ops->pixels && scanout->ops->pixels(pvg))
		drm_err_ratelimited(plane->dev, "no memory to copy the picture into\n");

	drm_atomic_helper_damage_iter_init(&iter, old_plane_state, plane_state);
	drm_atomic_for_each_plane_damage(&iter, &damage) {
		struct iosys_map dst = IOSYS_MAP_INIT_VADDR(scanout->pixels);
		struct drm_rect at;

		pvg->frame_dirty = true;
		if (surface || !scanout->pixels)
			continue;
		/* Memory that holds nothing of the picture yet takes all of it. */
		if (scanout->pixels_fresh)
			damage = shown;
		at = damage;
		drm_rect_translate(&at, -shown.x1, -shown.y1);
		iosys_map_incr(&dst,
			       drm_fb_clip_offset(scanout->pitch,
						  drm_format_info(PVG_SCANOUT_FORMAT),
						  &at));
		/*
		 * Both formats of the plane are copied as XRGB: the fourth byte
		 * set. That is what drm_fb_blit() did for XRGB, which kernels
		 * from 7.0 on no longer have.
		 */
		drm_fb_xrgb8888_to_argb8888(&dst, &scanout->pitch, shadow->data, fb,
					    &damage, &shadow->fmtcnv_state);
		if (scanout->pixels_fresh) {
			scanout->pixels_fresh = false;
			break;
		}
	}
	/*
	 * The commit waited for the fences of the buffer's memory before it
	 * came here, so a queued draw into it is done. Count what would go out
	 * undrawn if it had not.
	 */
	if (surface && !dma_resv_test_signaled(fb->obj[0]->resv, DMA_RESV_USAGE_WRITE))
		pvg->frames.undrawn++;
	pvg->frame_surface = surface;
	drm_dev_exit(idx);
}

static const struct drm_plane_helper_funcs pvg_plane_helper_funcs = {
	DRM_GEM_SHADOW_PLANE_HELPER_FUNCS,
	.prepare_fb = pvg_plane_prepare_fb,
	.atomic_check = pvg_plane_atomic_check,
	.atomic_update = pvg_plane_atomic_update,
};

static const struct drm_plane_funcs pvg_plane_funcs = {
	.update_plane = drm_atomic_helper_update_plane,
	.disable_plane = drm_atomic_helper_disable_plane,
	.destroy = drm_plane_cleanup,
	DRM_GEM_SHADOW_PLANE_FUNCS,
};

/*
 * The cursor plane. The device is handed the cursor's picture and is told
 * where the cursor is and whether it shows; it is the host that draws it.
 */
static const u32 pvg_cursor_formats[] = {
	DRM_FORMAT_ARGB8888,
};

static int pvg_cursor_atomic_check(struct drm_plane *plane,
				   struct drm_atomic_state *state)
{
	struct drm_plane_state *plane_state =
		drm_atomic_get_new_plane_state(state, plane);
	struct drm_crtc_state *crtc_state = NULL;
	int ret;

	if (plane_state->crtc)
		crtc_state = drm_atomic_get_new_crtc_state(state, plane_state->crtc);
	ret = drm_atomic_helper_check_plane_state(plane_state, crtc_state,
						  DRM_PLANE_NO_SCALING,
						  DRM_PLANE_NO_SCALING,
						  true, true);
	if (ret)
		return ret;
	if (plane_state->fb && (plane_state->fb->width > RUXNPVG_CURSOR_MAX_SIDE ||
				plane_state->fb->height > RUXNPVG_CURSOR_MAX_SIDE))
		return -EINVAL;
	return 0;
}

static void pvg_cursor_hide(struct pvg_device *pvg)
{
	if (pvg->cursor_shown) {
		pvg_display_cursor_show(pvg, false);
		pvg->cursor_shown = false;
	}
}

static void pvg_cursor_atomic_update(struct drm_plane *plane,
				     struct drm_atomic_state *state)
{
	struct pvg_device *pvg = to_pvg(plane->dev);
	struct drm_plane_state *plane_state =
		drm_atomic_get_new_plane_state(state, plane);
	struct drm_plane_state *old_plane_state =
		drm_atomic_get_old_plane_state(state, plane);
	struct drm_shadow_plane_state *shadow =
		to_drm_shadow_plane_state(plane_state);
	struct drm_framebuffer *fb = plane_state->fb;
	bool changed;
	int idx;

	if (!drm_dev_enter(plane->dev, &idx))
		return;
	if (!fb || !plane_state->visible) {
		pvg_cursor_hide(pvg);
		drm_dev_exit(idx);
		return;
	}

	/* A move alone leaves the picture as the device has it. */
	changed = fb != old_plane_state->fb ||
		  drm_plane_get_damage_clips_count(plane_state) ||
		  plane_state->hotspot_x != old_plane_state->hotspot_x ||
		  plane_state->hotspot_y != old_plane_state->hotspot_y;
	if (changed &&
	    pvg_display_cursor_glyph(pvg, shadow->data[0].vaddr + fb->offsets[0],
				     fb->pitches[0], fb->width, fb->height,
				     max(plane_state->hotspot_x, 0),
				     max(plane_state->hotspot_y, 0)))
		drm_err_ratelimited(plane->dev, "the cursor's picture was not taken\n");

	/* The cursor's place is where its hot spot is on the screen. */
	pvg_display_cursor_move(pvg,
				max(plane_state->crtc_x + max(plane_state->hotspot_x, 0), 0),
				max(plane_state->crtc_y + max(plane_state->hotspot_y, 0), 0));
	if (!pvg->cursor_shown) {
		pvg_display_cursor_show(pvg, true);
		pvg->cursor_shown = true;
	}
	drm_dev_exit(idx);
}

static void pvg_cursor_atomic_disable(struct drm_plane *plane,
				      struct drm_atomic_state *state)
{
	struct pvg_device *pvg = to_pvg(plane->dev);
	int idx;

	if (!drm_dev_enter(plane->dev, &idx))
		return;
	pvg_cursor_hide(pvg);
	drm_dev_exit(idx);
}

static const struct drm_plane_helper_funcs pvg_cursor_helper_funcs = {
	DRM_GEM_SHADOW_PLANE_HELPER_FUNCS,
	.atomic_check = pvg_cursor_atomic_check,
	.atomic_update = pvg_cursor_atomic_update,
	.atomic_disable = pvg_cursor_atomic_disable,
};

/*
 * Ends the page flip that is waiting, if one is. With hold the event is left
 * to the vblank timer's next tick, else it goes out now. The caller holds
 * the event lock.
 */
static void pvg_flip_end(struct pvg_device *pvg, bool hold)
{
	struct drm_pending_vblank_event *event = pvg->flip_event;

	lockdep_assert_held(&pvg->drm.event_lock);
	if (!event)
		return;
	pvg->flip_event = NULL;
	pvg->flip_kicked = false;
	/* The vblank reference was taken when the flip was set to wait. */
	if (hold) {
		drm_crtc_arm_vblank_event(&pvg->crtc, event);
	} else {
		drm_crtc_send_vblank_event(&pvg->crtc, event);
		drm_crtc_vblank_put(&pvg->crtc);
	}
}

/*
 * Called by the scanout path right before it tells the device of a frame,
 * after it has cleared away what the device said of earlier frames.
 */
void pvg_kms_frame_kicked(struct pvg_device *pvg)
{
	unsigned long flags;

	spin_lock_irqsave(&pvg->drm.event_lock, flags);
	if (pvg->flip_event) {
		pvg->flip_kicked = true;
		mod_timer(&pvg->flip_timer, jiffies + PVG_FLIP_TIMEOUT);
	}
	spin_unlock_irqrestore(&pvg->drm.event_lock, flags);
}

/* Called when the device has said that it has shown a frame. */
void pvg_kms_frame_shown(struct pvg_device *pvg, bool from_irq)
{
	unsigned long flags;

	spin_lock_irqsave(&pvg->drm.event_lock, flags);
	if (pvg->flip_event && pvg->flip_kicked) {
		if (from_irq)
			pvg->flips.by_irq++;
		else
			pvg->flips.by_look++;
		pvg_flip_end(pvg, flip == PVG_FLIP_SHOWN_PACED);
	}
	spin_unlock_irqrestore(&pvg->drm.event_lock, flags);
}

static void pvg_flip_timeout(struct timer_list *timer)
{
	struct pvg_device *pvg = timer_container_of(pvg, timer, flip_timer);
	unsigned long flags;

	spin_lock_irqsave(&pvg->drm.event_lock, flags);
	if (pvg->flip_event) {
		pvg->flips.timed_out++;
		pvg_flip_end(pvg, false);
	}
	spin_unlock_irqrestore(&pvg->drm.event_lock, flags);
}

static enum drm_mode_status
pvg_crtc_mode_valid(struct drm_crtc *crtc, const struct drm_display_mode *mode)
{
	struct pvg_device *pvg = to_pvg(crtc->dev);
	struct pvg_scanout *scanout = &pvg->scanout;
	enum drm_mode_status status = MODE_BAD;
	unsigned int i;

	if (scanout->ops->any_size)
		return (u64)pvg_surface_pitch(mode->hdisplay) * mode->vdisplay > PVG_SURFACE_MAX_BYTES ?
		       MODE_MEM : MODE_OK;

	mutex_lock(&scanout->modes_lock);
	for (i = 0; i < scanout->mode_count; i++) {
		if (scanout->modes[i].width == mode->hdisplay &&
		    scanout->modes[i].height == mode->vdisplay)
			status = MODE_OK;
	}
	mutex_unlock(&scanout->modes_lock);
	return status;
}

static void pvg_crtc_atomic_enable(struct drm_crtc *crtc,
				   struct drm_atomic_state *state)
{
	struct pvg_device *pvg = to_pvg(crtc->dev);
	struct drm_crtc_state *crtc_state =
		drm_atomic_get_new_crtc_state(state, crtc);
	const struct pvg_mode mode = {
		.width = crtc_state->mode.hdisplay,
		.height = crtc_state->mode.vdisplay,
	};
	int idx, ret;

	if (!drm_dev_enter(crtc->dev, &idx))
		return;
	ret = pvg->scanout.ops->enable(pvg, &mode);
	if (ret)
		drm_err(crtc->dev, "mode %ux%u not set: %d\n", mode.width,
			mode.height, ret);
	else
		drm_info(crtc->dev, "showing %ux%u through the %s\n", mode.width,
			 mode.height, pvg->scanout.ops->name);
	drm_dev_exit(idx);

	drm_crtc_vblank_atomic_enable(crtc, state);
}

/*
 * Has the device show the commit's frame, and sees to the commit's event.
 * Where the device says when it has shown the frame the event waits for
 * that word; otherwise, and for a commit that changed no pixel, it is left
 * to the vblank timer.
 */
static void pvg_crtc_atomic_flush(struct drm_crtc *crtc,
				  struct drm_atomic_state *state)
{
	struct pvg_device *pvg = to_pvg(crtc->dev);
	struct pvg_scanout *scanout = &pvg->scanout;
	struct pvg_surface *surface = pvg->frame_surface;
	bool dirty = pvg->frame_dirty && (surface || scanout->pixels);
	int idx;

	pvg->frame_dirty = false;
	pvg->frame_surface = NULL;
	if (dirty && drm_dev_enter(crtc->dev, &idx)) {
		if (scanout->ops->reports_frames && flip != PVG_FLIP_TIMER) {
			spin_lock_irq(&crtc->dev->event_lock);
			if (crtc->state->event && !pvg->flip_event &&
			    !drm_crtc_vblank_get(crtc)) {
				pvg->flip_event = crtc->state->event;
				pvg->flip_kicked = false;
				crtc->state->event = NULL;
			}
			spin_unlock_irq(&crtc->dev->event_lock);
		}
		if (scanout->ops->flush(pvg, surface)) {
			/* No frame went out, so no word will come for it. */
			spin_lock_irq(&crtc->dev->event_lock);
			pvg_flip_end(pvg, false);
			spin_unlock_irq(&crtc->dev->event_lock);
		} else if (surface) {
			pvg->frames.direct++;
		} else {
			pvg->frames.copied++;
		}
		drm_dev_exit(idx);
	}
	if (crtc->state->event)
		pvg->flips.by_timer++;
	drm_crtc_vblank_atomic_flush(crtc, state);
}

static void pvg_crtc_atomic_disable(struct drm_crtc *crtc,
				    struct drm_atomic_state *state)
{
	struct pvg_device *pvg = to_pvg(crtc->dev);
	int idx;

	drm_crtc_vblank_atomic_disable(crtc, state);

	/*
	 * With the CRTC off there is no flush that would send the commit's
	 * event, so it goes out here.
	 */
	spin_lock_irq(&crtc->dev->event_lock);
	if (crtc->state->event && !crtc->state->active) {
		drm_crtc_send_vblank_event(crtc, crtc->state->event);
		crtc->state->event = NULL;
	}
	spin_unlock_irq(&crtc->dev->event_lock);

	if (!drm_dev_enter(crtc->dev, &idx))
		return;
	pvg->scanout.ops->disable(pvg);
	drm_dev_exit(idx);
}

static const struct drm_crtc_helper_funcs pvg_crtc_helper_funcs = {
	.mode_valid = pvg_crtc_mode_valid,
	.atomic_check = drm_crtc_helper_atomic_check,
	.atomic_flush = pvg_crtc_atomic_flush,
	.atomic_enable = pvg_crtc_atomic_enable,
	.atomic_disable = pvg_crtc_atomic_disable,
};

static const struct drm_crtc_funcs pvg_crtc_funcs = {
	.reset = drm_atomic_helper_crtc_reset,
	.destroy = drm_crtc_cleanup,
	.set_config = drm_atomic_helper_set_config,
	.page_flip = drm_atomic_helper_page_flip,
	.atomic_duplicate_state = drm_atomic_helper_crtc_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_crtc_destroy_state,
	/*
	 * The device has no vblank. A timer at the mode's rate stands in for
	 * one: it gives the counts and times, and ends the page flips that
	 * the device's own word does not end.
	 */
	DRM_CRTC_VBLANK_TIMER_FUNCS,
};

static const struct drm_encoder_funcs pvg_encoder_funcs = {
	.destroy = drm_encoder_cleanup,
};

static int pvg_connector_get_modes(struct drm_connector *connector)
{
	struct pvg_device *pvg = to_pvg(connector->dev);
	struct pvg_scanout *scanout = &pvg->scanout;
	struct pvg_mode modes[PVG_MAX_MODES];
	unsigned int i, mode_count, count = 0;

	mutex_lock(&scanout->modes_lock);
	mode_count = scanout->mode_count;
	memcpy(modes, scanout->modes, sizeof(modes));
	mutex_unlock(&scanout->modes_lock);

	for (i = 0; i < mode_count; i++) {
		/* The device has sizes and a rate; the timings are made up. */
		const struct drm_display_mode timing = {
			DRM_MODE_INIT(modes[i].refresh ?: 60, modes[i].width,
				      modes[i].height, 0, 0)
		};
		struct drm_display_mode *mode;

		mode = drm_mode_duplicate(connector->dev, &timing);
		if (!mode)
			continue;
		/* The host lists the size of its window first. */
		if (i == 0)
			mode->type |= DRM_MODE_TYPE_PREFERRED;
		drm_mode_set_name(mode);
		drm_mode_probed_add(connector, mode);
		count++;
	}
	return count;
}

/*
 * A compositor reads the connector again at a hotplug event, but weston 14
 * keeps the mode it has for as long as the connector stays connected, even
 * when the preferred mode has changed. One that goes away and comes back is
 * made again with the preferred mode. So when a settled list has another
 * first mode while a compositor holds the device, the connector is shown as
 * gone: the next probe finds it disconnected, and a moment after that probe a
 * second hotplug event finds it back with the new modes. If nobody probes, it
 * comes back after a second anyway. pvg.replug=0 keeps it connected
 * throughout.
 */
static bool replug = true;
module_param(replug, bool, 0644);
MODULE_PARM_DESC(replug, "Show the connector gone for a moment when the host's preferred mode changes (default: on)");

/* After a probe has seen the connector gone, and if nobody probes. */
#define PVG_BACK_AFTER_PROBE_MS	100
#define PVG_BACK_AT_LATEST_MS	1000

static enum drm_connector_status pvg_connector_detect(struct drm_connector *connector, bool force)
{
	struct pvg_device *pvg = to_pvg(connector->dev);
	struct pvg_scanout *scanout = &pvg->scanout;
	bool gone;

	mutex_lock(&scanout->modes_lock);
	gone = pvg->gone;
	if (gone && !pvg->gone_seen) {
		pvg->gone_seen = true;
		mod_delayed_work(system_wq, &pvg->back_work,
				 msecs_to_jiffies(PVG_BACK_AFTER_PROBE_MS));
	}
	mutex_unlock(&scanout->modes_lock);
	return gone ? connector_status_disconnected : connector_status_connected;
}

static void pvg_connector_back(struct work_struct *work)
{
	struct pvg_device *pvg = container_of(to_delayed_work(work), struct pvg_device, back_work);
	struct pvg_scanout *scanout = &pvg->scanout;
	struct pvg_mode mode;
	bool was_gone;

	mutex_lock(&scanout->modes_lock);
	was_gone = pvg->gone;
	pvg->gone = false;
	mode = pvg->preferred;
	mutex_unlock(&scanout->modes_lock);
	if (!was_gone || !pvg->kms)
		return;
	drm_info(&pvg->drm, "the connector is back, %ux%u preferred\n", mode.width, mode.height);
	drm_kms_helper_hotplug_event(&pvg->drm);
}

/*
 * A mode list is taken once it has stayed the same for pvg.settle_ms: the
 * host's window may pass through sizes, and a compositor that made its output
 * again for each would lose what it was doing for nothing. QEMU's Cocoa
 * window once told 5120x2880 for about a second and then its old size again,
 * so the wait is longer than that. A list that ends as the one taken before
 * is not told at all.
 */
static unsigned int settle_ms = 1500;
module_param(settle_ms, uint, 0644);
MODULE_PARM_DESC(settle_ms, "How long a new mode list from the host has to stay before it is taken, in ms (default: 1500)");

/* The device has handed over a new mode list. */
void pvg_kms_modes_changed(struct pvg_device *pvg)
{
	if (!pvg->kms)
		return;
	mod_delayed_work(system_wq, &pvg->settle_work, msecs_to_jiffies(READ_ONCE(settle_ms)));
}

/* The host's mode list has stayed the same for pvg.settle_ms. */
static void pvg_kms_settled(struct work_struct *work)
{
	struct pvg_device *pvg = container_of(to_delayed_work(work), struct pvg_device, settle_work);
	struct pvg_scanout *scanout = &pvg->scanout;
	struct pvg_mode first = {};
	bool same, gone = false;

	if (!pvg->kms)
		return;
	mutex_lock(&scanout->modes_lock);
	same = scanout->mode_count == pvg->settled_count &&
	       !memcmp(scanout->modes, pvg->settled, scanout->mode_count * sizeof(scanout->modes[0]));
	if (scanout->mode_count)
		first = scanout->modes[0];
	/*
	 * The master is read without its lock: whether a compositor is there
	 * only decides whether the console's client sees the connector go.
	 */
	if (!same && scanout->mode_count && replug && READ_ONCE(pvg->drm.master) &&
	    (first.width != pvg->preferred.width || first.height != pvg->preferred.height)) {
		pvg->gone = true;
		pvg->gone_seen = false;
		gone = true;
	}
	if (scanout->mode_count)
		pvg->preferred = first;
	memcpy(pvg->settled, scanout->modes, sizeof(pvg->settled));
	pvg->settled_count = scanout->mode_count;
	mutex_unlock(&scanout->modes_lock);

	if (same) {
		drm_info(&pvg->drm, "the device's mode list is the one taken before: nothing has changed\n");
		return;
	}
	if (gone) {
		drm_info(&pvg->drm, "the device's mode list has changed, %ux%u preferred: the connector is gone until it is probed\n",
			 first.width, first.height);
		mod_delayed_work(system_wq, &pvg->back_work, msecs_to_jiffies(PVG_BACK_AT_LATEST_MS));
	} else {
		drm_info(&pvg->drm, "the device's mode list has changed\n");
	}
	drm_kms_helper_hotplug_event(&pvg->drm);
}

static const struct drm_connector_helper_funcs pvg_connector_helper_funcs = {
	.get_modes = pvg_connector_get_modes,
};

static const struct drm_connector_funcs pvg_connector_funcs = {
	.reset = drm_atomic_helper_connector_reset,
	.detect = pvg_connector_detect,
	.fill_modes = drm_helper_probe_single_connector_modes,
	.destroy = drm_connector_cleanup,
	.atomic_duplicate_state = drm_atomic_helper_connector_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_connector_destroy_state,
};

/*
 * What a compositor reads of a virtual display: that the modes change with
 * the host's window, so it reads them again at a hotplug event, and where
 * the output lies, which is at 0, 0. As qxl and vmwgfx have them.
 */
static int pvg_connector_properties(struct pvg_device *pvg)
{
	struct drm_device *drm = &pvg->drm;
	struct drm_property *hotplug;
	int ret;

	ret = drm_mode_create_suggested_offset_properties(drm);
	if (ret)
		return ret;
	hotplug = drm_property_create_range(drm, DRM_MODE_PROP_IMMUTABLE,
					    "hotplug_mode_update", 0, 1);
	if (!hotplug)
		return -ENOMEM;
	drm_object_attach_property(&pvg->connector.base, hotplug, 1);
	drm_object_attach_property(&pvg->connector.base,
				   drm->mode_config.suggested_x_property, 0);
	drm_object_attach_property(&pvg->connector.base,
				   drm->mode_config.suggested_y_property, 0);
	return 0;
}

static const struct drm_mode_config_funcs pvg_mode_config_funcs = {
	.fb_create = drm_gem_fb_create_with_dirty,
	.atomic_check = drm_atomic_helper_check,
	.atomic_commit = drm_atomic_helper_commit,
};

/*
 * The CRTC is enabled before the planes are updated, so that the first
 * picture of a mode is copied into the memory that mode is shown from.
 *
 * A commit then waits for the vblank timer's next tick before it is cleaned
 * up, which is what keeps commits to the timer's pace. Where flips are to
 * end by the device's word alone it waits for that word and no longer.
 */
static void pvg_atomic_commit_tail(struct drm_atomic_state *state)
{
	struct drm_device *drm = state->dev;
	struct pvg_device *pvg = to_pvg(drm);

	drm_atomic_helper_commit_modeset_disables(drm, state);
	drm_atomic_helper_commit_modeset_enables(drm, state);
	drm_atomic_helper_commit_planes(drm, state, DRM_PLANE_COMMIT_ACTIVE_ONLY);
	drm_atomic_helper_fake_vblank(state);
	drm_atomic_helper_commit_hw_done(state);
	if (pvg->scanout.ops->reports_frames && flip == PVG_FLIP_SHOWN)
		drm_atomic_helper_wait_for_flip_done(drm, state);
	else
		drm_atomic_helper_wait_for_vblanks(drm, state);
	drm_atomic_helper_cleanup_planes(drm, state);
}

static const struct drm_mode_config_helper_funcs pvg_mode_config_helper_funcs = {
	.atomic_commit_tail = pvg_atomic_commit_tail,
};

/*
 * A file's task has to be deleted while the file's handles are still there,
 * and DRM closes the handles before it calls the driver. So the driver's
 * part comes first.
 */
static int pvg_drm_release(struct inode *inode, struct file *filp)
{
	pvg_file_release(filp->private_data);
	return drm_release(inode, filp);
}

static const struct file_operations pvg_drm_fops = {
	.owner = THIS_MODULE,
	.open = drm_open,
	.release = pvg_drm_release,
	.unlocked_ioctl = drm_ioctl,
	.compat_ioctl = drm_compat_ioctl,
	.poll = drm_poll,
	.read = drm_read,
	.llseek = noop_llseek,
	.mmap = drm_gem_mmap,
	.fop_flags = FOP_UNSIGNED_OFFSET,
};

const struct drm_driver pvg_drm_driver = {
	/*
	 * The cursor is drawn by the host where the host's pointer is, so a
	 * program that uses the cursor plane has to say where the hot spot is.
	 * Version 3: DRM's sync objects, binary and timeline, for QUEUE.
	 */
	.driver_features = DRIVER_GEM | DRIVER_MODESET | DRIVER_ATOMIC |
			   DRIVER_CURSOR_HOTSPOT | DRIVER_RENDER |
			   DRIVER_SYNCOBJ | DRIVER_SYNCOBJ_TIMELINE,
	.fops = &pvg_drm_fops,
	.open = pvg_file_open,
	.postclose = pvg_file_postclose,
	.ioctls = pvg_ioctls,
	.num_ioctls = PVG_IOCTL_COUNT,
	.gem_prime_import = pvg_prime_import,
	.dumb_create = pvg_gem_dumb_create,
	DRM_FBDEV_SHMEM_DRIVER_OPS,
	.name = KBUILD_MODNAME,
	.desc = "Apple paravirtualized graphics",
	.major = 0,
	.minor = 3,
};

static const struct pvg_scanout_ops *const pvg_scanout_paths[] = {
	&pvg_mapper_ops,
	&pvg_efb_ops,
};

/*
 * Sets the display up on the first scanout path that works here, or on the
 * one path named: "mapper" or "efb".
 */
int pvg_kms_init(struct pvg_device *pvg, const char *path)
{
	struct pvg_scanout *scanout = &pvg->scanout;
	struct drm_device *drm = &pvg->drm;
	const struct pvg_scanout_ops *ops = NULL;
	unsigned int i;
	int ret = -ENODEV;

	mutex_init(&scanout->modes_lock);
	timer_setup(&pvg->flip_timer, pvg_flip_timeout, 0);
	INIT_DELAYED_WORK(&pvg->back_work, pvg_connector_back);
	INIT_DELAYED_WORK(&pvg->settle_work, pvg_kms_settled);

	for (i = 0; i < ARRAY_SIZE(pvg_scanout_paths); i++) {
		ops = pvg_scanout_paths[i];
		if (path && strcmp(path, "auto") && strcmp(path, ops->key))
			continue;
		scanout->ops = ops;
		ret = ops->init(pvg);
		if (!ret)
			break;
		drm_info(drm, "no picture through the %s: %d\n", ops->name, ret);
	}
	if (ret)
		return ret;
	drm_info(drm, "the picture goes through the %s\n", ops->name);

	ret = drmm_mode_config_init(drm);
	if (ret)
		goto err_scanout;

	drm->mode_config.funcs = &pvg_mode_config_funcs;
	drm->mode_config.helper_private = &pvg_mode_config_helper_funcs;
	drm->mode_config.preferred_depth = 24;
	if (ops->any_size) {
		drm->mode_config.min_width = 1;
		drm->mode_config.min_height = 1;
		drm->mode_config.max_width = RUXNPVG_MAX_SIDE;
		drm->mode_config.max_height = RUXNPVG_MAX_SIDE;
	} else {
		drm->mode_config.min_width = U16_MAX;
		drm->mode_config.min_height = U16_MAX;
		for (i = 0; i < scanout->mode_count; i++) {
			const struct pvg_mode *mode = &scanout->modes[i];

			drm->mode_config.min_width = min(drm->mode_config.min_width, mode->width);
			drm->mode_config.max_width = max(drm->mode_config.max_width, mode->width);
			drm->mode_config.min_height = min(drm->mode_config.min_height, mode->height);
			drm->mode_config.max_height = max(drm->mode_config.max_height, mode->height);
		}
	}

	ret = drm_universal_plane_init(drm, &pvg->plane, 1, &pvg_plane_funcs,
				       pvg_plane_formats, ARRAY_SIZE(pvg_plane_formats),
				       NULL, DRM_PLANE_TYPE_PRIMARY, NULL);
	if (ret)
		goto err_scanout;
	drm_plane_helper_add(&pvg->plane, &pvg_plane_helper_funcs);
	drm_plane_enable_fb_damage_clips(&pvg->plane);

	if (ops->cursor) {
		ret = drm_universal_plane_init(drm, &pvg->cursor, 1, &pvg_plane_funcs,
					       pvg_cursor_formats,
					       ARRAY_SIZE(pvg_cursor_formats),
					       NULL, DRM_PLANE_TYPE_CURSOR, NULL);
		if (ret)
			goto err_scanout;
		drm_plane_helper_add(&pvg->cursor, &pvg_cursor_helper_funcs);
		drm->mode_config.cursor_width = 64;
		drm->mode_config.cursor_height = 64;
	}

	ret = drm_crtc_init_with_planes(drm, &pvg->crtc, &pvg->plane,
					ops->cursor ? &pvg->cursor : NULL,
					&pvg_crtc_funcs, NULL);
	if (ret)
		goto err_scanout;
	drm_crtc_helper_add(&pvg->crtc, &pvg_crtc_helper_funcs);

	ret = drm_encoder_init(drm, &pvg->encoder, &pvg_encoder_funcs,
			       DRM_MODE_ENCODER_VIRTUAL, NULL);
	if (ret)
		goto err_scanout;
	pvg->encoder.possible_crtcs = drm_crtc_mask(&pvg->crtc);

	ret = drm_connector_init(drm, &pvg->connector, &pvg_connector_funcs,
				 DRM_MODE_CONNECTOR_VIRTUAL);
	if (ret)
		goto err_scanout;
	drm_connector_helper_add(&pvg->connector, &pvg_connector_helper_funcs);
	ret = drm_connector_attach_encoder(&pvg->connector, &pvg->encoder);
	if (ret)
		goto err_scanout;
	ret = pvg_connector_properties(pvg);
	if (ret)
		goto err_scanout;

	ret = drm_vblank_init(drm, 1);
	if (ret)
		goto err_scanout;

	drm_mode_config_reset(drm);

	ret = drm_dev_register(drm, 0);
	if (ret)
		goto err_scanout;

	if (scanout->mode_count)
		pvg->preferred = scanout->modes[0];
	memcpy(pvg->settled, scanout->modes, sizeof(pvg->settled));
	pvg->settled_count = scanout->mode_count;
	pvg->kms = true;
	drm_client_setup(drm, NULL);
	return 0;

err_scanout:
	ops->fini(pvg);
	return ret;
}

void pvg_kms_exit(struct pvg_device *pvg)
{
	pvg->kms = false;
	drm_dev_unplug(&pvg->drm);
	drm_atomic_helper_shutdown(&pvg->drm);
	/* No probe is left to put the connector back later. */
	cancel_delayed_work_sync(&pvg->settle_work);
	cancel_delayed_work_sync(&pvg->back_work);

	timer_delete_sync(&pvg->flip_timer);
	spin_lock_irq(&pvg->drm.event_lock);
	pvg_flip_end(pvg, false);
	spin_unlock_irq(&pvg->drm.event_lock);

	pvg->scanout.ops->fini(pvg);
}
