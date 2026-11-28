// SPDX-License-Identifier: GPL-2.0
/*
 * Buffer objects: memory a program maps and the host reads and writes.
 *
 * The memory is device pages, each a block of the page allocator split into
 * the guest's pages, as a surface's is. It is pinned from the start, since
 * the host maps it by its physical address. A handle to a buffer object puts
 * it into the task of the handle's file, and the handle's end takes it out.
 */

#include <linux/dma-buf.h>
#include <linux/iosys-map.h>
#include <linux/mm.h>
#include <linux/slab.h>
#include <linux/vmalloc.h>

#include <drm/drm_drv.h>
#include <drm/drm_file.h>
#include <drm/drm_gem.h>
#include <drm/drm_gem_shmem_helper.h>
#include <drm/drm_prime.h>
#include <drm/drm_vma_manager.h>

#include "pvg.h"

/*
 * The largest buffer object. Up to version 2 it was what one page of a
 * task's table names, 64 MiB; a place in the task may lie across several
 * pages of the table, as a 64 MiB one that does not start at a table's start
 * did already. Since version 3 a quarter of the task.
 */
#define PVG_BO_MAX_BYTES	(256UL << 20)

static const struct drm_gem_object_funcs pvg_bo_funcs;

/* The buffer object behind a GEM object, or NULL if it is none of these. */
struct pvg_bo *to_pvg_bo(struct drm_gem_object *obj)
{
	if (!obj || obj->funcs != &pvg_bo_funcs)
		return NULL;
	return container_of(obj, struct pvg_bo, base);
}

static void pvg_bo_free_pages(struct pvg_bo *bo)
{
	unsigned int i;

	for (i = 0; i < bo->page_count; i++) {
		if (bo->pages[i])
			__free_page(bo->pages[i]);
	}
	kvfree(bo->pages);
}

static void pvg_bo_free(struct drm_gem_object *obj)
{
	struct pvg_bo *bo = to_pvg_bo(obj);

	/* A placement holds the object, so none is left. */
	WARN_ON(!list_empty(&bo->placements));
	drm_gem_object_release(obj);
	pvg_bo_free_pages(bo);
	kfree(bo);
}

/* A handle for a file: the memory comes to lie in the file's task. */
static int pvg_bo_open(struct drm_gem_object *obj, struct drm_file *file)
{
	struct pvg_task *task = pvg_file_task(file);

	if (IS_ERR(task))
		return PTR_ERR(task);
	return pvg_task_place(task, to_pvg_bo(obj));
}

static void pvg_bo_close(struct drm_gem_object *obj, struct drm_file *file)
{
	struct pvg_file *priv = file->driver_priv;

	if (priv->task)
		pvg_task_unplace(priv->task, to_pvg_bo(obj));
}

static const struct vm_operations_struct pvg_bo_vm_ops = {
	.open = drm_gem_vm_open,
	.close = drm_gem_vm_close,
};

static int pvg_bo_mmap(struct drm_gem_object *obj, struct vm_area_struct *vma)
{
	struct pvg_bo *bo = to_pvg_bo(obj);

	/* The offset handed in is the object's place among all mappings. */
	vma->vm_pgoff -= drm_vma_node_start(&obj->vma_node);
	vma->vm_ops = &pvg_bo_vm_ops;
	vm_flags_set(vma, VM_DONTEXPAND | VM_DONTDUMP);
	return vm_map_pages(vma, bo->pages, bo->page_count);
}

/* For a dma-buf of it that another device maps. */
static struct sg_table *pvg_bo_get_sg_table(struct drm_gem_object *obj)
{
	struct pvg_bo *bo = to_pvg_bo(obj);

	return drm_prime_pages_to_sg(obj->dev, bo->pages, bo->page_count);
}

/* For the kernel: a framebuffer over it is copied from here. */
static int pvg_bo_vmap(struct drm_gem_object *obj, struct iosys_map *map)
{
	struct pvg_bo *bo = to_pvg_bo(obj);
	void *vaddr = vmap(bo->pages, bo->page_count, VM_MAP, PAGE_KERNEL);

	if (!vaddr)
		return -ENOMEM;
	iosys_map_set_vaddr(map, vaddr);
	return 0;
}

static void pvg_bo_vunmap(struct drm_gem_object *obj, struct iosys_map *map)
{
	vunmap(map->vaddr);
}

static const struct drm_gem_object_funcs pvg_bo_funcs = {
	.free = pvg_bo_free,
	.open = pvg_bo_open,
	.close = pvg_bo_close,
	.mmap = pvg_bo_mmap,
	.get_sg_table = pvg_bo_get_sg_table,
	.vmap = pvg_bo_vmap,
	.vunmap = pvg_bo_vunmap,
};

/*
 * A dma-buf taken in. One of this device's own is the same GEM object, and
 * a handle to it puts a buffer object into the importer's task. One of
 * another device's is taken as before, as memory the display copies from.
 */
struct drm_gem_object *pvg_prime_import(struct drm_device *drm,
					struct dma_buf *dma_buf)
{
	struct drm_gem_object *obj;

	if (drm_gem_is_prime_exported_dma_buf(drm, dma_buf)) {
		obj = dma_buf->priv;
		drm_gem_object_get(obj);
		return obj;
	}
	return drm_gem_shmem_prime_import_no_map(drm, dma_buf);
}

static struct pvg_bo *pvg_bo_create(struct pvg_device *pvg, size_t bytes)
{
	unsigned int device_pages = bytes >> RUXNPVG_PAGE_SHIFT;
	struct pvg_bo *bo;
	unsigned int i, j;

	bo = kzalloc(sizeof(*bo), GFP_KERNEL);
	if (!bo)
		return ERR_PTR(-ENOMEM);
	mutex_init(&bo->lock);
	INIT_LIST_HEAD(&bo->placements);
	bo->page_count = device_pages * PVG_PAGE_PAGES;
	bo->pages = kvcalloc(bo->page_count, sizeof(*bo->pages), GFP_KERNEL);
	if (!bo->pages) {
		kfree(bo);
		return ERR_PTR(-ENOMEM);
	}

	/*
	 * A task has room for more than a guest may have of memory. A program
	 * that asks for more than there is gets ENOMEM, and the OOM killer is
	 * not called for it.
	 */
	for (i = 0; i < device_pages; i++) {
		struct page *block = alloc_pages(GFP_KERNEL | __GFP_ZERO | __GFP_NOWARN |
						 __GFP_RETRY_MAYFAIL, PVG_PAGE_ORDER);
		phys_addr_t phys;

		if (!block)
			goto err;
		split_page(block, PVG_PAGE_ORDER);
		for (j = 0; j < PVG_PAGE_PAGES; j++)
			bo->pages[i * PVG_PAGE_PAGES + j] = block + j;

		/* A task's table has 31 bits for a page's number. */
		phys = page_to_phys(block);
		if ((phys & (PVG_PAGE_SIZE - 1)) ||
		    phys >> RUXNPVG_PAGE_SHIFT >= BIT(RUXNPVG_TASK_PAGE_NUMBER_BITS)) {
			dev_err(&pvg->pdev->dev, "memory at %pa is of no use to a task\n",
				&phys);
			goto err;
		}
	}

	bo->base.funcs = &pvg_bo_funcs;
	drm_gem_private_object_init(&pvg->drm, &bo->base, bytes);
	return bo;

err:
	pvg_bo_free_pages(bo);
	kfree(bo);
	return ERR_PTR(-ENOMEM);
}

int pvg_bo_create_ioctl(struct drm_device *drm, void *data, struct drm_file *file)
{
	struct pvg_device *pvg = to_pvg(drm);
	struct drm_pvg_bo_create *args = data;
	struct pvg_task *task;
	struct pvg_bo *bo;
	size_t bytes;
	int ret;

	if (args->pad || (args->flags & ~PVG_BO_SURFACE))
		return -EINVAL;

	if (args->flags & PVG_BO_SURFACE) {
		args->address = 0;
		return pvg_gem_surface_create(file, drm, args->width, args->height,
					      &args->handle, &args->pitch,
					      &args->size, &args->offset);
	}

	if (args->width || args->height || !args->size ||
	    args->size > PVG_BO_MAX_BYTES)
		return -EINVAL;
	bytes = ALIGN(args->size, PVG_PAGE_SIZE);

	/* The handle's making puts the memory into the task. */
	task = pvg_file_task(file);
	if (IS_ERR(task))
		return PTR_ERR(task);

	bo = pvg_bo_create(pvg, bytes);
	if (IS_ERR(bo))
		return PTR_ERR(bo);
	ret = drm_gem_create_mmap_offset(&bo->base);
	if (!ret)
		ret = drm_gem_handle_create(file, &bo->base, &args->handle);
	if (!ret)
		ret = pvg_task_address(task, bo, &args->address);
	if (!ret) {
		args->size = bytes;
		args->pitch = 0;
		args->offset = drm_vma_node_offset_addr(&bo->base.vma_node);
	}
	/* The handle keeps the buffer object from here on. */
	drm_gem_object_put(&bo->base);
	return ret;
}

/*
 * Version 3: what a handle of the file is, for a program that took it in
 * through PRIME. It sends nothing.
 */
int pvg_bo_info_ioctl(struct drm_device *drm, void *data, struct drm_file *file)
{
	struct drm_pvg_bo_info *args = data;
	struct drm_gem_object *obj;
	struct pvg_surface *surface;
	struct pvg_task *task;
	struct pvg_bo *bo;
	u32 handle = args->handle;
	int ret = 0;

	if (args->pad)
		return -EINVAL;
	obj = drm_gem_object_lookup(file, handle);
	if (!obj)
		return -ENOENT;
	memset(args, 0, sizeof(*args));
	args->handle = handle;
	args->size = obj->size;

	surface = pvg_gem_surface(obj);
	bo = to_pvg_bo(obj);
	if (surface) {
		args->flags = PVG_BO_SURFACE;
		args->width = surface->width;
		args->height = surface->height;
		args->pitch = surface->pitch;
	} else if (!bo) {
		/*
		 * A dma-buf of another device, or a dumb buffer of ordinary
		 * pages: memory the display copies from and no task can name.
		 */
		args->flags = PVG_BO_INFO_OTHER_DEVICE;
		goto out;
	}
	ret = drm_gem_create_mmap_offset(obj);
	if (ret)
		goto out;
	args->offset = drm_vma_node_offset_addr(&obj->vma_node);
	if (bo) {
		/* The handle put the buffer object into the file's task. */
		task = pvg_file_task(file);
		ret = IS_ERR(task) ? PTR_ERR(task) : pvg_task_address(task, bo, &args->address);
	}
out:
	drm_gem_object_put(obj);
	return ret;
}
