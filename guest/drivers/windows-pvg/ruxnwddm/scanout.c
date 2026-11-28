// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The framebuffer the desktop is copied into for milestone 1, and nothing
// else: the early framebuffer that the firmware's pvgfb set up in reserved
// guest memory and that the device shows on its own, about ten times a
// second. Windows' basic display driver writes the same pages today.
//
// Two ways to find it. Windows may hand it over (the POST display) when
// this driver takes the device; failing that, the early framebuffer's
// bank names its first page and its mode. Either way the device is told
// nothing: it keeps showing those pages as it did since boot.
//
// This is the stepping stone the design allows: the display-only model
// gives the driver a picture in system memory, so every present is a CPU
// copy. Milestone 2 replaces it.

#include "ruxnwddm.h"

#define EFB_WIDTH(mode)		((mode) & 0xffff)
#define EFB_HEIGHT(mode)	((mode) >> 16)

static BOOLEAN post_usable(const RUXNWDDM_ADAPTER *a)
{
	const DXGK_DISPLAY_INFORMATION *p = &a->post;

	return NT_SUCCESS(a->post_status) && p->Width && p->Height &&
	       p->Pitch >= p->Width * RUXNWDDM_BYTES_PER_PIXEL && p->PhysicAddress.QuadPart &&
	       (p->ColorFormat == D3DDDIFMT_X8R8G8B8 || p->ColorFormat == D3DDDIFMT_A8R8G8B8);
}

static BOOLEAN efb_usable(const RUXNWDDM_ADAPTER *a)
{
	ULONG width = EFB_WIDTH(a->efb_mode), height = EFB_HEIGHT(a->efb_mode);

	return a->efb_page && width && height && a->efb_depth == RUXNWDDM_BITS_PER_PIXEL &&
	       (ULONG64)width * height * RUXNWDDM_BYTES_PER_PIXEL <= a->efb_bytes;
}

static void choose(RUXNWDDM_ADAPTER *a)
{
	RUXNWDDM_SCANOUT *s = &a->scanout;

	if (post_usable(a)) {
		s->kind = RUXNWDDM_SCANOUT_POST;
		s->phys = a->post.PhysicAddress;
		s->width = a->post.Width;
		s->height = a->post.Height;
		s->pitch = a->post.Pitch;
	} else if (efb_usable(a)) {
		s->kind = RUXNWDDM_SCANOUT_EFB;
		s->phys.QuadPart = (LONGLONG)a->efb_page << RUXNWDDM_PAGE_SHIFT;
		s->width = EFB_WIDTH(a->efb_mode);
		s->height = EFB_HEIGHT(a->efb_mode);
		s->pitch = s->width * RUXNWDDM_BYTES_PER_PIXEL;
	} else {
		s->kind = RUXNWDDM_SCANOUT_NONE;
		return;
	}
	// pvgfb's pixels are blue, green, red and a spare byte in memory,
	// which is what Windows calls X8R8G8B8.
	s->format = D3DDDIFMT_X8R8G8B8;
	s->bytes = ROUND_TO_PAGES((SIZE_T)s->pitch * s->height);
}

NTSTATUS ruxnwddm_scanout_open(RUXNWDDM_ADAPTER *a)
{
	RUXNWDDM_SCANOUT *s = &a->scanout;

	ruxnwddm_scanout_close(a);
	choose(a);
	ruxnwddm_record_ulong(L"ScanoutKind", s->kind);
	// Whether what Windows handed over is the early framebuffer itself.
	ruxnwddm_record_ulong(L"PostIsEfb", post_usable(a) && efb_usable(a) &&
			      (ULONG64)a->post.PhysicAddress.QuadPart ==
			      (ULONG64)a->efb_page << RUXNWDDM_PAGE_SHIFT);
	if (s->kind == RUXNWDDM_SCANOUT_NONE)
		return STATUS_GRAPHICS_NO_DISPLAY_MODE_MANAGEMENT_SUPPORT;

	s->va = MmMapIoSpaceEx(s->phys, s->bytes, PAGE_READWRITE | PAGE_WRITECOMBINE);
	if (!s->va) {
		s->kind = RUXNWDDM_SCANOUT_NONE;
		return STATUS_INSUFFICIENT_RESOURCES;
	}
	ruxnwddm_record_ulong64(L"ScanoutAddress", (ULONG64)s->phys.QuadPart);
	ruxnwddm_record_ulong(L"ScanoutWidth", s->width);
	ruxnwddm_record_ulong(L"ScanoutHeight", s->height);
	ruxnwddm_record_ulong(L"ScanoutPitch", s->pitch);
	return STATUS_SUCCESS;
}

void ruxnwddm_scanout_close(RUXNWDDM_ADAPTER *a)
{
	RUXNWDDM_SCANOUT *s = &a->scanout;

	if (s->va)
		MmUnmapIoSpace(s->va, s->bytes);
	s->va = NULL;
}
