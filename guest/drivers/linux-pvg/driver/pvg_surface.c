// SPDX-License-Identifier: GPL-2.0
/*
 * The IOSurface mapper's device, and the surfaces it is told of.
 *
 * The mapper is a second device behind the same PCI function: its registers
 * are BAR 2 and its interrupt is MSI vector 1. It gives a surface, which is
 * a list of device pages anywhere in guest memory, a mapping id that display
 * commands then name. Its ring is its own: a command is 16 bytes, the
 * command, a mapping id and a word the device writes the outcome to.
 *
 * All of this is observed.
 */

#include <linux/bitmap.h>
#include <linux/interrupt.h>
#include <linux/io-64-nonatomic-lo-hi.h>
#include <linux/jiffies.h>
#include <linux/mm.h>
#include <linux/slab.h>
#include <linux/vmalloc.h>

#include "pvg.h"

/*
 * The ring is one device page, RUXNPVG_MAP_SLOTS commands. The length the
 * device is told must be a multiple of 1024 commands: the host's process
 * ends on any other value. So this is a constant, and nothing else is ever
 * written to that register.
 *
 * Mapping ids run from 1 to 8191: a bitmap of 8192 with 0 never handed out.
 * A page of descriptions serves RUXNPVG_MAP_IDS_PER_PAGE ids.
 */
#define PVG_MAP_IDS		(RUXNPVG_MAP_LAST_ID + 1)
#define PVG_MAP_DESCRIPTOR_PAGES	(PVG_MAP_IDS / RUXNPVG_MAP_IDS_PER_PAGE)

#define PVG_MAP_TIMEOUT		msecs_to_jiffies(RUXNPVG_MAP_TIMEOUT_MS)

struct pvg_mapper {
	void __iomem *regs;
	int irq;

	struct mutex lock;		/* one command at a time, the ids, the lists */
	struct page *ring_page;
	struct page *table_page;
	struct page *descriptor_pages[PVG_MAP_DESCRIPTOR_PAGES];
	__le32 *ring;
	__le32 *table;
	/*
	 * How many commands the guest has written. A counter that runs on;
	 * the slot in the ring is the counter modulo RUXNPVG_MAP_SLOTS.
	 */
	u64 written;
	wait_queue_head_t wait;
	DECLARE_BITMAP(ids, PVG_MAP_IDS);
	struct list_head mapped;	/* surfaces that have a mapping id */
	/* Surfaces given up while a display showed them, which keeps them mapped. */
	struct list_head held;

	u64 irqs;
	u64 commands;
	u64 polled;			/* commands found done before their interrupt */
};

/*
 * How the mapper names a device page wherever it wants one: the page's
 * number shifted left by two, with bit 0 for valid.
 */
static u32 pvg_map_entry(struct page *page)
{
	return pvg_page_number(page) << 2 | 1;
}

static irqreturn_t pvg_mapper_irq(int irq, void *data)
{
	struct pvg_mapper *mapper = data;

	/* One interrupt for each command done. Nothing to read or clear. */
	mapper->irqs++;
	wake_up(&mapper->wait);
	return IRQ_HANDLED;
}

static bool pvg_mapper_done(struct pvg_mapper *mapper)
{
	return (s64)(readq(mapper->regs + RUXNPVG_MAP_REG_DONE) - mapper->written) >= 0;
}

/*
 * Puts one command into the mapper's ring and sleeps until the device has
 * done it. The outcome is 0 or an error of the host's.
 */
static int pvg_mapper_command(struct pvg_device *pvg, u32 command, u32 id,
			      u32 *outcome)
{
	struct pvg_mapper *mapper = pvg->mapper;
	__le32 *slot = mapper->ring +
		       (mapper->written % RUXNPVG_MAP_SLOTS) * (RUXNPVG_MAP_COMMAND_BYTES / 4);
	unsigned long deadline = jiffies + PVG_MAP_TIMEOUT;

	lockdep_assert_held(&mapper->lock);

	slot[0] = cpu_to_le32(command);
	slot[1] = cpu_to_le32(id);
	slot[2] = cpu_to_le32(U32_MAX);
	slot[3] = 0;
	/* The count only ever goes up by one from what the device was told. */
	mapper->written++;
	mapper->commands++;
	writeq(mapper->written, mapper->regs + RUXNPVG_MAP_REG_WRITTEN);

	/*
	 * The interrupt wakes this up. It can be late, and without a second
	 * MSI vector there is none, so the count is looked at every tick too.
	 */
	if (pvg_mapper_done(mapper))
		mapper->polled++;
	while (!wait_event_timeout(mapper->wait, pvg_mapper_done(mapper), 1)) {
		if (time_after(jiffies, deadline)) {
			dev_err(&pvg->pdev->dev,
				"mapper: command %u for id %u not done: written %llu, done %llu\n",
				command, id, mapper->written,
				readq(mapper->regs + RUXNPVG_MAP_REG_DONE));
			return -ETIMEDOUT;
		}
	}

	*outcome = le32_to_cpu(READ_ONCE(slot[2]));
	return 0;
}

/* For the debugfs instrument: how many surfaces have a mapping id, and how
 * many of them were given up and are still held by the device.
 */
void pvg_mapper_counts(struct pvg_device *pvg, unsigned int *mapped,
		       unsigned int *held)
{
	struct pvg_mapper *mapper = pvg->mapper;

	*mapped = 0;
	*held = 0;
	if (!mapper)
		return;
	mutex_lock(&mapper->lock);
	*mapped = list_count_nodes(&mapper->mapped);
	*held = list_count_nodes(&mapper->held);
	mutex_unlock(&mapper->lock);
}

/* For the debugfs instrument: the mapper's registers, if it is there. */
void __iomem *pvg_mapper_regs(struct pvg_device *pvg)
{
	return pvg->mapper ? pvg->mapper->regs : NULL;
}

int pvg_mapper_init(struct pvg_device *pvg)
{
	struct pci_dev *pdev = pvg->pdev;
	struct pvg_mapper *mapper;
	u64 written, done;
	int ret;

	BUILD_BUG_ON(PAGE_SHIFT > RUXNPVG_PAGE_SHIFT);

	if (!(pci_resource_flags(pdev, RUXNPVG_BAR_MAPPER) & IORESOURCE_MEM) ||
	    pci_resource_len(pdev, RUXNPVG_BAR_MAPPER) <= RUXNPVG_MAP_REG_LAST)
		return -ENODEV;

	mapper = kzalloc(sizeof(*mapper), GFP_KERNEL);
	if (!mapper)
		return -ENOMEM;
	mutex_init(&mapper->lock);
	init_waitqueue_head(&mapper->wait);
	INIT_LIST_HEAD(&mapper->mapped);
	INIT_LIST_HEAD(&mapper->held);
	mapper->irq = -1;

	mapper->regs = pci_iomap(pdev, RUXNPVG_BAR_MAPPER, 0);
	mapper->ring_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	mapper->table_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	if (!mapper->regs || !mapper->ring_page || !mapper->table_page) {
		ret = -ENOMEM;
		goto err;
	}
	mapper->ring = page_address(mapper->ring_page);
	mapper->table = page_address(mapper->table_page);

	if (pvg->vectors > 1) {
		ret = request_irq(pci_irq_vector(pdev, 1), pvg_mapper_irq, 0,
				  KBUILD_MODNAME "-mapper", mapper);
		if (ret)
			goto err;
		mapper->irq = pci_irq_vector(pdev, 1);
	} else {
		dev_info(&pdev->dev, "mapper: no interrupt of its own, polling\n");
	}

	/*
	 * The device's two counts outlive a driver, and a count written that
	 * is smaller than the one before must never reach a device of this
	 * kind. So this driver goes on from where the last one stopped.
	 */
	written = readq(mapper->regs + RUXNPVG_MAP_REG_WRITTEN);
	done = readq(mapper->regs + RUXNPVG_MAP_REG_DONE);
	mapper->written = max(written, done);

	writeq(page_to_phys(mapper->ring_page), mapper->regs + RUXNPVG_MAP_REG_RING);
	writeq(RUXNPVG_MAP_SLOTS, mapper->regs + RUXNPVG_MAP_REG_SLOTS);
	writeq(page_to_phys(mapper->table_page), mapper->regs + RUXNPVG_MAP_REG_TABLE);

	dev_info(&pdev->dev,
		 "mapper: ring of %lu commands, written %llu and done %llu before, %s\n",
		 (unsigned long)RUXNPVG_MAP_SLOTS, written, done,
		 mapper->irq < 0 ? "polled" : "with its interrupt");

	pvg->mapper = mapper;
	return 0;

err:
	if (mapper->regs)
		pci_iounmap(pdev, mapper->regs);
	pvg_free_pages(mapper->ring_page, PVG_PAGE_SIZE);
	pvg_free_pages(mapper->table_page, PVG_PAGE_SIZE);
	kfree(mapper);
	return ret;
}

static int pvg_surface_unmap_locked(struct pvg_device *pvg,
				    struct pvg_surface *surface);

/*
 * Called once no display shows anything. Buffers can outlive the mapper, so
 * what is still mapped is unmapped here and stays its owner's to free.
 */
void pvg_mapper_fini(struct pvg_device *pvg)
{
	struct pvg_mapper *mapper = pvg->mapper;
	struct pvg_surface *surface, *next;
	unsigned int i;

	if (!mapper)
		return;

	pvg_surface_reap(pvg);
	mutex_lock(&mapper->lock);
	list_for_each_entry_safe(surface, next, &mapper->mapped, link) {
		if (pvg_surface_unmap_locked(pvg, surface)) {
			dev_warn(&pvg->pdev->dev,
				 "mapping %u is still held by the device\n", surface->id);
			list_del_init(&surface->link);
			surface->id = 0;
		}
	}
	list_for_each_entry_safe(surface, next, &mapper->held, link) {
		dev_warn(&pvg->pdev->dev,
			 "mapping %u is still held by the device, %zu bytes are left to it\n",
			 surface->id, surface->bytes);
		list_del(&surface->link);
		vunmap(surface->vaddr);
		kvfree(surface->pages);
		kfree(surface);
	}
	mutex_unlock(&mapper->lock);
	pvg->mapper = NULL;

	if (mapper->irq >= 0)
		free_irq(mapper->irq, mapper);
	dev_info(&pvg->pdev->dev,
		 "mapper: %llu commands, %llu interrupts, %llu done before the sender slept\n",
		 mapper->commands, mapper->irqs, mapper->polled);
	/*
	 * The device keeps the ring's and the table's addresses. It reads
	 * neither before the next kick, and the next driver names new ones.
	 */
	pci_iounmap(pvg->pdev, mapper->regs);
	for (i = 0; i < PVG_MAP_DESCRIPTOR_PAGES; i++)
		pvg_free_pages(mapper->descriptor_pages[i], PVG_PAGE_SIZE);
	pvg_free_pages(mapper->ring_page, PVG_PAGE_SIZE);
	pvg_free_pages(mapper->table_page, PVG_PAGE_SIZE);
	kfree(mapper);
}

/* Where an id's descriptor is; the page is made when its first id is used. */
static __le32 *pvg_mapper_descriptor(struct pvg_device *pvg, u32 id)
{
	struct pvg_mapper *mapper = pvg->mapper;
	unsigned int index = id / RUXNPVG_MAP_IDS_PER_PAGE;
	struct page *page = mapper->descriptor_pages[index];

	if (!page) {
		page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
		if (!page)
			return NULL;
		mapper->descriptor_pages[index] = page;
		mapper->table[index] = cpu_to_le32(pvg_map_entry(page));
	}
	return page_address(page) + (id % RUXNPVG_MAP_IDS_PER_PAGE) * RUXNPVG_MAP_DESCRIPTION_BYTES;
}

static void pvg_surface_free_pages(struct pvg_surface *surface)
{
	unsigned int i;

	for (i = 0; i < surface->page_count; i++) {
		if (surface->pages[i])
			__free_page(surface->pages[i]);
	}
	kvfree(surface->pages);
}

/*
 * Makes a surface of zeroed memory. Each device page is a block of the page
 * allocator, which aligns a block to its own size, split into the guest's
 * pages so that each can be mapped by itself.
 */
struct pvg_surface *pvg_surface_create(struct pvg_device *pvg, u32 width,
				       u32 height)
{
	struct pvg_surface *surface;
	unsigned int device_pages, i, j;
	__le32 *list;

	if (!width || !height || width > U16_MAX || height > U16_MAX)
		return ERR_PTR(-EINVAL);

	surface = kzalloc(sizeof(*surface), GFP_KERNEL);
	if (!surface)
		return ERR_PTR(-ENOMEM);
	surface->width = width;
	surface->height = height;
	surface->pitch = pvg_surface_pitch(width);
	surface->bytes = ALIGN((size_t)surface->pitch * height, PVG_PAGE_SIZE);
	device_pages = surface->bytes >> RUXNPVG_PAGE_SHIFT;
	if (device_pages > RUXNPVG_SURFACE_MAX_PAGES) {
		kfree(surface);
		return ERR_PTR(-EINVAL);
	}
	surface->page_count = device_pages * PVG_PAGE_PAGES;

	surface->pages = kvcalloc(surface->page_count, sizeof(*surface->pages),
				  GFP_KERNEL);
	surface->list_page = pvg_alloc_pages(pvg, PVG_PAGE_SIZE);
	if (!surface->pages || !surface->list_page)
		goto err;
	list = page_address(surface->list_page);

	for (i = 0; i < device_pages; i++) {
		struct page *block = alloc_pages(GFP_KERNEL | __GFP_ZERO,
						 PVG_PAGE_ORDER);
		phys_addr_t phys;

		if (!block)
			goto err;
		split_page(block, PVG_PAGE_ORDER);
		for (j = 0; j < PVG_PAGE_PAGES; j++)
			surface->pages[i * PVG_PAGE_PAGES + j] = block + j;

		/* The mapper's word for a page has 30 bits for its number. */
		phys = page_to_phys(block);
		if ((phys & (PVG_PAGE_SIZE - 1)) ||
		    phys >> RUXNPVG_PAGE_SHIFT >= BIT(30)) {
			dev_err(&pvg->pdev->dev,
				"memory at %pa is of no use to the mapper\n", &phys);
			goto err;
		}
		list[i] = cpu_to_le32(pvg_map_entry(block));
	}

	surface->vaddr = vmap(surface->pages, surface->page_count, VM_MAP,
			      PAGE_KERNEL);
	if (!surface->vaddr)
		goto err;
	INIT_LIST_HEAD(&surface->link);
	return surface;

err:
	if (surface->pages)
		pvg_surface_free_pages(surface);
	pvg_free_pages(surface->list_page, PVG_PAGE_SIZE);
	kfree(surface);
	return ERR_PTR(-ENOMEM);
}

static void pvg_surface_free(struct pvg_surface *surface)
{
	vunmap(surface->vaddr);
	pvg_surface_free_pages(surface);
	pvg_free_pages(surface->list_page, PVG_PAGE_SIZE);
	kfree(surface);
}

/*
 * Gives the surface's memory back. The device will not unmap a surface a
 * display shows, and the host goes on reading its pages. Such a surface is
 * kept until pvg_surface_reap() finds the display on another.
 */
void pvg_surface_destroy(struct pvg_device *pvg, struct pvg_surface *surface)
{
	struct pvg_mapper *mapper = pvg->mapper;
	int ret;

	if (!surface)
		return;
	/* Without the mapper nothing is mapped: its end unmapped what was. */
	if (surface->id && mapper) {
		mutex_lock(&mapper->lock);
		ret = pvg_surface_unmap_locked(pvg, surface);
		if (ret)
			list_move(&surface->link, &mapper->held);
		mutex_unlock(&mapper->lock);
		if (ret)
			return;
	}
	pvg_surface_free(surface);
}

/* Frees the surfaces that were given up while on show and no longer are. */
void pvg_surface_reap(struct pvg_device *pvg)
{
	struct pvg_mapper *mapper = pvg->mapper;
	struct pvg_surface *surface, *next;

	if (!mapper || list_empty_careful(&mapper->held))
		return;
	mutex_lock(&mapper->lock);
	list_for_each_entry_safe(surface, next, &mapper->held, link) {
		if (!pvg_surface_unmap_locked(pvg, surface))
			pvg_surface_free(surface);
	}
	mutex_unlock(&mapper->lock);
}

/*
 * Gives the surface a mapping id. Mapping over an id that is mapped is no
 * error for the device and changes nothing, so the id is unmapped first. An
 * id the device will not unmap is one a driver before this one left on a
 * display; it is passed over.
 *
 * A surface shared by PRIME can be mapped by two files at once, so whether
 * it has an id is asked under the mapper's lock.
 */
int pvg_surface_map(struct pvg_device *pvg, struct pvg_surface *surface)
{
	struct pvg_mapper *mapper = pvg->mapper;
	__le32 *descriptor;
	u32 id, outcome;
	int ret = 0;

	if (READ_ONCE(surface->id))
		return 0;
	if (!mapper)
		return -ENODEV;

	mutex_lock(&mapper->lock);
	if (surface->id)
		goto out;
	for (;;) {
		id = find_next_zero_bit(mapper->ids, PVG_MAP_IDS, 1);
		if (id >= PVG_MAP_IDS) {
			ret = -ENOSPC;
			goto out;
		}
		descriptor = pvg_mapper_descriptor(pvg, id);
		if (!descriptor) {
			ret = -ENOMEM;
			goto out;
		}
		ret = pvg_mapper_command(pvg, RUXNPVG_MAP_CMD_UNMAP, id, &outcome);
		if (ret)
			goto out;
		if (!outcome)
			break;
		dev_info(&pvg->pdev->dev,
			 "mapper: id %u is taken and stays so (%#x), passing over it\n",
			 id, outcome);
		__set_bit(id, mapper->ids);
	}

	memset(descriptor, 0, RUXNPVG_MAP_DESCRIPTION_BYTES);
	descriptor[RUXNPVG_DESC_PAGE_LIST] = cpu_to_le32(pvg_map_entry(surface->list_page));
	descriptor[RUXNPVG_DESC_FORMAT] = cpu_to_le32(RUXNPVG_FORMAT_BGRA);
	descriptor[RUXNPVG_DESC_BYTES] = cpu_to_le32(surface->bytes);
	/* An element is one pixel wide and high. */
	descriptor[RUXNPVG_DESC_WIDTH] = cpu_to_le32(surface->width << 8 | 1);
	descriptor[RUXNPVG_DESC_HEIGHT] = cpu_to_le32(surface->height << 8 | 1);
	descriptor[RUXNPVG_DESC_PITCH] = cpu_to_le32(surface->pitch);
	descriptor[RUXNPVG_DESC_PIXEL_BYTES] = cpu_to_le32(4);

	ret = pvg_mapper_command(pvg, RUXNPVG_MAP_CMD_MAP, id, &outcome);
	if (!ret && outcome) {
		dev_err(&pvg->pdev->dev, "mapper: %ux%u not mapped as id %u: %#x\n",
			surface->width, surface->height, id, outcome);
		ret = -EIO;
	}
	if (!ret) {
		__set_bit(id, mapper->ids);
		WRITE_ONCE(surface->id, id);
		list_add(&surface->link, &mapper->mapped);
	}
out:
	mutex_unlock(&mapper->lock);
	return ret;
}

/*
 * Takes the surface's mapping away. -EBUSY says a display still shows it:
 * it stays mapped until the display has been swapped to another.
 */
static int pvg_surface_unmap_locked(struct pvg_device *pvg,
				    struct pvg_surface *surface)
{
	struct pvg_mapper *mapper = pvg->mapper;
	u32 outcome;
	int ret;

	lockdep_assert_held(&mapper->lock);
	if (!surface->id)
		return 0;

	ret = pvg_mapper_command(pvg, RUXNPVG_MAP_CMD_UNMAP, surface->id, &outcome);
	if (!ret && outcome) {
		if (outcome != RUXNPVG_MAP_OUTCOME_BUSY)
			dev_err(&pvg->pdev->dev, "mapper: id %u not unmapped: %#x\n",
				surface->id, outcome);
		ret = outcome == RUXNPVG_MAP_OUTCOME_BUSY ? -EBUSY : -EIO;
	}
	if (!ret) {
		__clear_bit(surface->id, mapper->ids);
		surface->id = 0;
		list_del_init(&surface->link);
	}
	return ret;
}

int pvg_surface_unmap(struct pvg_device *pvg, struct pvg_surface *surface)
{
	struct pvg_mapper *mapper = pvg->mapper;
	int ret;

	if (!surface->id)
		return 0;
	if (!mapper)
		return -ENODEV;
	mutex_lock(&mapper->lock);
	ret = pvg_surface_unmap_locked(pvg, surface);
	mutex_unlock(&mapper->lock);
	return ret;
}
