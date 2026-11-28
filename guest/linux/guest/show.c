// SPDX-License-Identifier: GPL-2.0-or-later
#include <linux/capability.h>
#include <linux/cred.h>
#include <linux/file.h>
#include <linux/spinlock.h>
#include <linux/uaccess.h>
#include "nvfwd_guest.h"

/*
 * One file shows frames at a time. Another file of the same user, or of an administrator, takes
 * over from it, as a program that starts again does; another user's waits until it closes.
 */
static DEFINE_SPINLOCK(show_lock);
static struct nvf_file *show_owner;
static kuid_t show_owner_uid;

static bool show_claim(struct nvf_file *nf)
{
	kuid_t uid = current_euid();
	bool ok;

	spin_lock(&show_lock);
	ok = !show_owner || show_owner == nf || uid_eq(uid, show_owner_uid) || capable(CAP_SYS_ADMIN);
	if (ok && show_owner != nf) {
		show_owner = nf;
		show_owner_uid = uid;
	}
	spin_unlock(&show_lock);
	return ok;
}

void show_file_closed(struct nvf_file *nf)
{
	spin_lock(&show_lock);
	if (show_owner == nf)
		show_owner = NULL;
	spin_unlock(&show_lock);
}

/* The "show this surface" calls. These go to QEMU's own code, never to the host's driver. */
long show_ioctl(struct nvf_file *nf, unsigned int cmd, unsigned long arg)
{
	struct nvfwd_show_buffer b;
	struct nvfwd_req *r;
	u64 value = 0;
	long rc;
	int slot;

	if (cmd != NVFWD_SHOW_IOC_BUFFER && cmd != NVFWD_SHOW_IOC_NEXT && cmd != NVFWD_SHOW_IOC_FRAME)
		return -ENOTTY;
	if (!show_claim(nf))
		return -EBUSY;
	if (cmd == NVFWD_SHOW_IOC_BUFFER) {
		struct file *f;

		if (copy_from_user(&b, (void __user *)arg, sizeof(b)))
			return -EFAULT;
		f = fget(b.fd);
		if (!f)
			return -EBADF;
		if (f->f_op != nv.fops) {
			fput(f);
			return -EBADF;
		}
		slot = slot_get();
		r = slot_req(slot);
		r->op = NVFWD_OP_SHOW_BUFFER;
		r->fdid = ((struct nvf_file *)f->private_data)->fdid;
		r->a0 = b.slot;
		r->a1 = b.width | (u64)b.height << 32;
		r->a2 = b.pitch | (u64)b.format << 32;
		r->a3 = b.size;
		kick(slot);
		rc = r->ret;
		slot_put(slot);
		fput(f);
		return rc;
	}
	if (cmd == NVFWD_SHOW_IOC_FRAME && copy_from_user(&value, (void __user *)arg, 8))
		return -EFAULT;
	slot = slot_get();
	r = slot_req(slot);
	r->op = cmd == NVFWD_SHOW_IOC_NEXT ? NVFWD_OP_SHOW_NEXT : NVFWD_OP_SHOW_FRAME;
	r->a0 = value;
	kick(slot);
	rc = r->ret;
	slot_put(slot);
	if (cmd == NVFWD_SHOW_IOC_NEXT && rc >= 0) {
		value = rc;
		if (copy_to_user((void __user *)arg, &value, 4))
			return -EFAULT;
		return 0;
	}
	return rc;
}
