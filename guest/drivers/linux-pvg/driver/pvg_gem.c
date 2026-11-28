// SPDX-License-Identifier: GPL-2.0
/*
 * Buffers the device can show from their own memory.
 *
 * A dumb buffer of user space's is made as a surface: device pages the
 * mapper's device can be told of. A framebuffer over such a buffer is shown
 * by a swap to the buffer's mapping id, with nothing copied. The memory is
 * pinned from the start, since the host maps it by its physical address.
 *
 * The kernel's own console keeps buffers of the shmem helper, which its
 * fbdev emulation is written for, and so do buffers the device has no
 * format for. Those are copied, see pvg_kms.c.
 */

#include <linux/mm.h>
#include <linux/module.h>
#include <linux/slab.h>

#include <drm/drm_client.h>
#include <drm/drm_device.h>
#include <drm/drm_fb_helper.h>
#include <drm/drm_file.h>
#include <drm/drm_gem.h>
#include <drm/drm_gem_shmem_helper.h>
#include <drm/drm_prime.h>
#include <drm/drm_vma_manager.h>

#include "pvg.h"

static bool direct = true;
module_param(direct, bool, 0444);
MODULE_PARM_DESC(direct,
		 "Show user space's buffers from their own memory where the device can, in place of a copy (default on)");

struct pvg_gem_object {
	struct drm_gem_object base;
	struct pvg_surface *surface;
};

static const struct drm_gem_object_funcs pvg_gem_funcs;

static struct pvg_gem_object *to_pvg_gem(struct drm_gem_object *obj)
{
	return container_of(obj, struct pvg_gem_object, base);
}

/* The buffer's surface, or NULL for a buffer that is none of these. */
struct pvg_surface *pvg_gem_surface(struct drm_gem_object *obj)
{
	if (!obj || obj->funcs != &pvg_gem_funcs)
		return NULL;
	return to_pvg_gem(obj)->surface;
}

static void pvg_gem_free(struct drm_gem_object *obj)
{
	struct pvg_gem_object *bo = to_pvg_gem(obj);

	pvg_surface_destroy(to_pvg(obj->dev), bo->surface);
	drm_gem_object_release(obj);
	kfree(bo);
}

static int pvg_gem_vmap(struct drm_gem_object *obj, struct iosys_map *map)
{
	iosys_map_set_vaddr(map, to_pvg_gem(obj)->surface->vaddr);
	return 0;
}

static const struct vm_operations_struct pvg_gem_vm_ops = {
	.open = drm_gem_vm_open,
	.close = drm_gem_vm_close,
};

static int pvg_gem_mmap(struct drm_gem_object *obj, struct vm_area_struct *vma)
{
	struct pvg_surface *surface = to_pvg_gem(obj)->surface;

	/* The offset handed in is the object's place among all mappings. */
	vma->vm_pgoff -= drm_vma_node_start(&obj->vma_node);
	vma->vm_ops = &pvg_gem_vm_ops;
	vm_flags_set(vma, VM_DONTEXPAND | VM_DONTDUMP);
	return vm_map_pages(vma, surface->pages, surface->page_count);
}

/* For a dma-buf of it that another device maps. */
static struct sg_table *pvg_gem_get_sg_table(struct drm_gem_object *obj)
{
	struct pvg_surface *surface = to_pvg_gem(obj)->surface;

	return drm_prime_pages_to_sg(obj->dev, surface->pages, surface->page_count);
}

static const struct drm_gem_object_funcs pvg_gem_funcs = {
	.free = pvg_gem_free,
	.vmap = pvg_gem_vmap,
	.mmap = pvg_gem_mmap,
	.get_sg_table = pvg_gem_get_sg_table,
};

static bool pvg_gem_wanted(struct pvg_device *pvg, struct drm_file *file,
			   const struct drm_mode_create_dumb *args)
{
	struct drm_fb_helper *fb_helper = pvg->drm.fb_helper;

	if (!direct || !pvg->scanout.ops || !pvg->scanout.ops->surfaces)
		return false;
	if (fb_helper && file == fb_helper->client.file)
		return false;
	return args->bpp == 32 && args->width && args->height &&
	       args->width <= RUXNPVG_MAX_SIDE && args->height <= RUXNPVG_MAX_SIDE &&
	       (u64)pvg_surface_pitch(args->width) * args->height <= PVG_SURFACE_MAX_BYTES;
}

/*
 * Makes a buffer whose memory is a surface, with a handle for the file and
 * the offset its memory is mapped at.
 */
int pvg_gem_surface_create(struct drm_file *file, struct drm_device *drm,
			   u32 width, u32 height, u32 *handle, u32 *pitch,
			   u64 *size, u64 *offset)
{
	struct pvg_device *pvg = to_pvg(drm);
	struct pvg_gem_object *bo;
	struct pvg_surface *surface;
	int ret;

	if (!pvg->scanout.ops || !pvg->scanout.ops->surfaces)
		return -ENODEV;
	if (!width || !height || width > RUXNPVG_MAX_SIDE || height > RUXNPVG_MAX_SIDE ||
	    (u64)pvg_surface_pitch(width) * height > PVG_SURFACE_MAX_BYTES)
		return -EINVAL;

	bo = kzalloc(sizeof(*bo), GFP_KERNEL);
	if (!bo)
		return -ENOMEM;
	surface = pvg_surface_create(pvg, width, height);
	if (IS_ERR(surface)) {
		kfree(bo);
		return PTR_ERR(surface);
	}
	bo->surface = surface;
	bo->base.funcs = &pvg_gem_funcs;
	drm_gem_private_object_init(drm, &bo->base, surface->bytes);

	ret = drm_gem_create_mmap_offset(&bo->base);
	if (!ret)
		ret = drm_gem_handle_create(file, &bo->base, handle);
	if (!ret) {
		*pitch = surface->pitch;
		*size = surface->bytes;
		*offset = drm_vma_node_offset_addr(&bo->base.vma_node);
	}
	/* The handle keeps the buffer from here on. */
	drm_gem_object_put(&bo->base);
	return ret;
}

int pvg_gem_dumb_create(struct drm_file *file, struct drm_device *drm,
			struct drm_mode_create_dumb *args)
{
	struct pvg_device *pvg = to_pvg(drm);
	u64 offset;

	if (!pvg_gem_wanted(pvg, file, args))
		return drm_gem_shmem_dumb_create(file, drm, args);
	return pvg_gem_surface_create(file, drm, args->width, args->height,
				      &args->handle, &args->pitch, &args->size,
				      &offset);
}
