// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Pages pinned for OS descriptor allocations. The host's device answers each with a pin id, and says
 * when nothing on the host reaches the pages any more: only then do they go back to the guest. A
 * device that gives no id leaves the pages pinned until the guest reboots.
 */
#include <linux/list.h>
#include <linux/mm.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include "nvfwd_guest.h"

struct nvf_pin {
	struct list_head link;
	u32 pin;
	struct page **pages;
	unsigned long npages;
};

static LIST_HEAD(pinned);
static DEFINE_MUTEX(pinned_lock);
static u64 let_go_seen;

/* 1 while the pin is held, 0 once it isn't, negative from a device that doesn't say. */
static long pin_held(u64 pin, u64 *let_go)
{
	int slot = slot_get();
	struct nvfwd_req *r = slot_req(slot);
	long rc;

	r->op = NVFWD_OP_PIN_HELD;
	r->fdid = 0;
	r->a0 = pin;
	r->a3 = 0;
	kick(slot);
	rc = r->ret;
	*let_go = r->a3;
	slot_put(slot);
	return rc;
}

void pins_keep(u64 pin, struct page **pages, unsigned long npages)
{
	struct nvf_pin *p = pin ? kmalloc(sizeof(*p), GFP_KERNEL) : NULL;

	if (!p) { /* no word will come for these: they stay pinned */
		kvfree(pages);
		return;
	}
	p->pin = pin;
	p->pages = pages;
	p->npages = npages;
	mutex_lock(&pinned_lock);
	list_add_tail(&p->link, &pinned);
	mutex_unlock(&pinned_lock);
}

/* After a free or a close: gives back the pages of every pin the host has let go. */
void pins_check(void)
{
	struct nvf_pin *p, *next;
	u64 let_go, ignored;

	mutex_lock(&pinned_lock);
	if (list_empty(&pinned) || pin_held(0, &let_go) < 0 || let_go == let_go_seen)
		goto out;
	let_go_seen = let_go;
	list_for_each_entry_safe(p, next, &pinned, link) {
		if (pin_held(p->pin, &ignored) != 0)
			continue;
		list_del(&p->link);
		unpin_user_pages(p->pages, p->npages);
		kvfree(p->pages);
		kfree(p);
	}
out:
	mutex_unlock(&pinned_lock);
}
