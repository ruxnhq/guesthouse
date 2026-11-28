// SPDX-License-Identifier: GPL-2.0
/*
 * Guest driver for Apple's paravirtualized graphics device: the PCI device,
 * its interrupt, and the memory it is handed.
 */

#include <linux/aperture.h>
#include <linux/gfp.h>
#include <linux/interrupt.h>
#include <linux/module.h>
#include <linux/pci.h>

#include <drm/drm_drv.h>

#include "pvg.h"

static bool irq_log;
module_param(irq_log, bool, 0444);
MODULE_PARM_DESC(irq_log, "Log every interrupt from the start");

static bool modeset = true;
module_param(modeset, bool, 0444);
MODULE_PARM_DESC(modeset, "Register a display (default on)");

static bool firmware = true;
module_param(firmware, bool, 0444);
MODULE_PARM_DESC(firmware,
		 "Take the screen from the firmware: remove its framebuffer and turn the early framebuffer off (default on)");

static char *scanout = "auto";
module_param(scanout, charp, 0444);
MODULE_PARM_DESC(scanout,
		 "How the picture reaches the device: mapper, efb, or auto for the first that works (default)");

/*
 * The device reads guest memory by its physical address: the host maps the
 * guest's RAM as a whole and knows nothing of an IOMMU the guest may have.
 * So memory for it comes from the page allocator, not from the DMA interface,
 * whose handles are the bus's addresses. A block of the page allocator is
 * aligned to its own size, which makes any block of 16 KiB or more a run of
 * whole device pages.
 */
struct page *pvg_alloc_pages(struct pvg_device *pvg, size_t bytes)
{
	unsigned int order = get_order(max(bytes, PVG_PAGE_SIZE));
	struct page *pages;
	phys_addr_t phys;

	pages = alloc_pages(GFP_KERNEL | __GFP_ZERO | __GFP_NOWARN, order);
	if (!pages)
		return NULL;

	phys = page_to_phys(pages);
	if ((phys & (PVG_PAGE_SIZE - 1)) ||
	    (phys + bytes - 1) >> RUXNPVG_PAGE_SHIFT > U32_MAX) {
		dev_err(&pvg->pdev->dev, "memory at %pa is of no use to the device\n",
			&phys);
		__free_pages(pages, order);
		return NULL;
	}
	return pages;
}

void pvg_free_pages(struct page *pages, size_t bytes)
{
	if (pages)
		__free_pages(pages, get_order(max(bytes, PVG_PAGE_SIZE)));
}

u32 pvg_page_number(struct page *pages)
{
	return page_to_phys(pages) >> RUXNPVG_PAGE_SHIFT;
}

/*
 * The device has one interrupt for everything, and three registers say what
 * it was for. Observed: the done bits and the display events clear when
 * read, so each is read once here; the fault stays until the ring is
 * resumed. Nothing has to be acknowledged for the next interrupt to come,
 * and several causes can share one interrupt, so a run that finds nothing is
 * no error. irq_skip is the debugfs instrument's: it leaves registers unread.
 */
static irqreturn_t pvg_irq(int irq, void *data)
{
	struct pvg_device *pvg = data;
	u32 done = 0, events = 0, fault = 0;

	if (!(pvg->irq_skip & BIT(0)))
		done = pvg_read(pvg, RUXNPVG_REG_DONE);
	if (!(pvg->irq_skip & BIT(1)))
		events = pvg_read(pvg, RUXNPVG_REG_DISPLAY_EVENTS);
	if (!(pvg->irq_skip & BIT(2)))
		fault = pvg_read(pvg, RUXNPVG_REG_FAULT);

	spin_lock(&pvg->counts_lock);
	pvg->counts.taken++;
	if (done)
		pvg->counts.done++;
	if (events)
		pvg->counts.events++;
	if (fault)
		pvg->counts.faults++;
	if (!done && !events && !fault)
		pvg->counts.empty++;
	spin_unlock(&pvg->counts_lock);

	if (pvg->irq_log)
		dev_info(&pvg->pdev->dev,
			 "irq: done %#x, events %#x, fault %#x, stamp cell %#x\n",
			 done, events, fault, le32_to_cpu(READ_ONCE(pvg->ring.mem[0])));

	pvg_ring_irq(pvg, fault);
	if ((done & ~1U) || fault)
		pvg_child_irq(pvg);
	if (events && READ_ONCE(pvg->display_live))
		pvg_display_irq(pvg);
	return IRQ_HANDLED;
}

/*
 * The firmware may have left the device showing a picture: an option ROM such
 * as Guesthouse's pvgfb turns the early framebuffer on and hands the firmware a
 * framebuffer in guest memory, which Linux then shows its console on through
 * simpledrm. That framebuffer lies in no BAR of this device, so the aperture
 * helper finds it by the firmware's screen_info, which names no PCI device:
 * sysfb's device is removed with simpledrm on it. The early framebuffer is
 * then turned off, so that from here on the device shows what this driver
 * gives it. Its mode count reads 0 while it is off, so the count says
 * whether the firmware had it on. Done before the ring is set up.
 */
static void pvg_take_from_firmware(struct pvg_device *pvg)
{
	struct pci_dev *pdev = pvg->pdev;
	u32 modes;
	int ret;

	ret = aperture_remove_conflicting_pci_devices(pdev, KBUILD_MODNAME);
	if (ret)
		dev_warn(&pdev->dev, "the firmware's framebuffer was not removed: %d\n", ret);

	modes = pvg_read(pvg, RUXNPVG_REG_EFB_MODE_COUNT);
	pvg_write(pvg, RUXNPVG_REG_EFB_ON, 0);
	if (modes)
		dev_info(&pdev->dev,
			 "the firmware's early framebuffer was on (%u modes); turned off\n",
			 modes);
}

static int pvg_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct pvg_device *pvg;
	int ret;

	pvg = devm_drm_dev_alloc(&pdev->dev, &pvg_drm_driver, struct pvg_device, drm);
	if (IS_ERR(pvg))
		return PTR_ERR(pvg);
	pvg->pdev = pdev;
	pvg->irq_log = irq_log;
	spin_lock_init(&pvg->counts_lock);
	ida_init(&pvg->task_ids);
	ida_init(&pvg->child_ids);
	pvg_children_init(pvg);
	pci_set_drvdata(pdev, pvg);

	ret = pcim_enable_device(pdev);
	if (ret)
		return ret;
	pvg->regs = pcim_iomap_region(pdev, 0, KBUILD_MODNAME);
	if (IS_ERR(pvg->regs))
		return PTR_ERR(pvg->regs);
	pvg->firmware = firmware;
	if (firmware)
		pvg_take_from_firmware(pvg);

	/*
	 * The device has no other interrupt than MSI, and QEMU delivers MSI
	 * through the device's bus-master address space. Vector 0 is the
	 * graphics device's. Where the IOSurface mapper is there, vector 1
	 * is its own.
	 */
	pci_set_master(pdev);
	ret = pci_alloc_irq_vectors(pdev, 1, 2, PCI_IRQ_MSI);
	if (ret < 0)
		return dev_err_probe(&pdev->dev, ret, "no MSI\n");
	pvg->vectors = ret;

	ret = pvg_ring_start(pvg);
	if (ret)
		return ret;

	ret = request_irq(pci_irq_vector(pdev, 0), pvg_irq, 0, KBUILD_MODNAME, pvg);
	if (ret) {
		dev_err(&pdev->dev, "no interrupt: %d\n", ret);
		goto err_ring;
	}

	pvg_debugfs_init(pvg);

	/* One command each way round, to see the ring and the interrupt work. */
	ret = pvg_ring_send(pvg, RUXNPVG_CMD_NOP, NULL, 0);
	if (ret) {
		dev_err(&pdev->dev, "the first command got no answer: %d\n", ret);
		goto err_irq;
	}
	dev_info(&pdev->dev, "ring up, protocol version %u\n",
		 pvg_read(pvg, RUXNPVG_REG_VERSION));

	/* The ring is of use without a display, so this may fail. */
	if (modeset) {
		ret = pvg_kms_init(pvg, scanout);
		if (ret)
			dev_warn(&pdev->dev, "no display: %d\n", ret);
	}
	return 0;

err_irq:
	pvg_debugfs_exit(pvg);
	free_irq(pci_irq_vector(pdev, 0), pvg);
	pvg_children_fini(pvg);
err_ring:
	pvg_ring_stop(pvg);
	pvg_ring_free(pvg);
	return ret;
}

static void pvg_remove(struct pci_dev *pdev)
{
	struct pvg_device *pvg = pci_get_drvdata(pdev);

	if (pvg->kms)
		pvg_kms_exit(pvg);
	pvg_debugfs_exit(pvg);
	pvg_ring_stop(pvg);
	free_irq(pci_irq_vector(pdev, 0), pvg);
	pvg_children_fini(pvg);
	pvg_ring_free(pvg);
}

static const struct pci_device_id pvg_ids[] = {
	{ PCI_DEVICE(RUXNPVG_PCI_VENDOR, RUXNPVG_PCI_DEVICE) },
	{ }
};
MODULE_DEVICE_TABLE(pci, pvg_ids);

static struct pci_driver pvg_driver = {
	.name = KBUILD_MODNAME,
	.id_table = pvg_ids,
	.probe = pvg_probe,
	.remove = pvg_remove,
};

static int __init pvg_init(void)
{
	int ret;

	pvg_debugfs_register();
	ret = pci_register_driver(&pvg_driver);
	if (ret)
		pvg_debugfs_unregister();
	return ret;
}
module_init(pvg_init);

static void __exit pvg_exit(void)
{
	pci_unregister_driver(&pvg_driver);
	pvg_debugfs_unregister();
}
module_exit(pvg_exit);

MODULE_DESCRIPTION("Guest driver for Apple's paravirtualized graphics device");
MODULE_LICENSE("GPL");
