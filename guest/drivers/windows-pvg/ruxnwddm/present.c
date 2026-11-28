// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// The desktop's pixels into the scanout. In the display-only model Windows
// composes the desktop in software into a picture in system memory and
// calls PresentDisplayOnly with the rectangles that changed; the driver
// copies those rectangles into the framebuffer the device shows.
//
// STEPPING STONE: this is a CPU copy of every changed pixel, which the
// owner's rule allows only as a marked step. It goes away with milestone 2,
// where the compositor draws on the GPU into surfaces the device flips to.
//
// Also here: the bugcheck screen (system display), which writes into the
// same framebuffer at high IRQL with nothing else running.

#include "ruxnwddm.h"

// Copies one rectangle of the picture to the same place in the scanout,
// clipped to both.
static ULONG64 copy_rect(RUXNWDDM_ADAPTER *a, const UCHAR *source, ULONG source_pitch,
			 const RECT *rect)
{
	RUXNWDDM_SCANOUT *s = &a->scanout;
	LONG left = max(rect->left, 0), top = max(rect->top, 0);
	LONG right = min(rect->right, (LONG)s->width), bottom = min(rect->bottom, (LONG)s->height);
	SIZE_T row_bytes;
	LONG y;

	if (right <= left || bottom <= top)
		return 0;
	row_bytes = (SIZE_T)(right - left) * RUXNWDDM_BYTES_PER_PIXEL;
	for (y = top; y < bottom; y++) {
		const UCHAR *from = source + (SIZE_T)y * source_pitch +
				    (SIZE_T)left * RUXNWDDM_BYTES_PER_PIXEL;
		UCHAR *to = s->va + (SIZE_T)y * s->pitch + (SIZE_T)left * RUXNWDDM_BYTES_PER_PIXEL;

		RtlCopyMemory(to, from, row_bytes);
	}
	return (ULONG64)row_bytes * (ULONG64)(bottom - top);
}

NTSTATUS ruxnwddm_present(RUXNWDDM_ADAPTER *a, const DXGKARG_PRESENT_DISPLAYONLY *args)
{
	const UCHAR *source = args->pSource;
	ULONG i;

	if (args->VidPnSourceId != RUXNWDDM_SOURCE_ID ||
	    args->BytesPerPixel != RUXNWDDM_BYTES_PER_PIXEL)
		return STATUS_INVALID_PARAMETER;
	a->present.presents++;
	if (!a->scanout.va || !a->source_visible || a->monitor_power != PowerDeviceD0) {
		a->present.skipped++;
		return STATUS_SUCCESS;
	}

	// pSource holds the whole new picture, so a move's destination is
	// already right in it: copying it from there is the same as moving the
	// old pixels, and needs no read of write-combined memory. Moves come
	// first, as Windows asks.
	for (i = 0; i < args->NumMoves; i++)
		a->present.bytes += copy_rect(a, source, args->Pitch, &args->pMoves[i].DestRect);
	for (i = 0; i < args->NumDirtyRects; i++)
		a->present.bytes += copy_rect(a, source, args->Pitch, &args->pDirtyRect[i]);
	a->present.moves += args->NumMoves;
	a->present.dirty_rects += args->NumDirtyRects;
	// Write-combined stores reach memory before the device next reads it.
	KeMemoryBarrier();
	return STATUS_SUCCESS;
}

void ruxnwddm_blank(RUXNWDDM_ADAPTER *a)
{
	RUXNWDDM_SCANOUT *s = &a->scanout;
	ULONG y;

	if (!s->va)
		return;
	for (y = 0; y < s->height; y++)
		RtlZeroMemory(s->va + (SIZE_T)y * s->pitch, (SIZE_T)s->width * RUXNWDDM_BYTES_PER_PIXEL);
}

// The bugcheck screen. The framebuffer is already mapped and shown, so
// there is nothing to set up but to clear it.
NTSTATUS ruxnwddm_system_display_enable(RUXNWDDM_ADAPTER *a, UINT *width, UINT *height,
					D3DDDIFORMAT *format)
{
	if (!a->scanout.va)
		return STATUS_UNSUCCESSFUL;
	*width = a->scanout.width;
	*height = a->scanout.height;
	*format = D3DDDIFMT_X8R8G8B8;
	ruxnwddm_blank(a);
	return STATUS_SUCCESS;
}

void ruxnwddm_system_display_write(RUXNWDDM_ADAPTER *a, const VOID *source, UINT width,
				   UINT height, UINT stride, UINT x, UINT y)
{
	RECT rect;
	const UCHAR *origin;

	if (!a->scanout.va || x >= a->scanout.width || y >= a->scanout.height)
		return;
	// copy_rect reads the source at the destination's coordinates, so the
	// block's origin is moved back by (x, y).
	origin = (const UCHAR *)source - (SSIZE_T)y * stride - (SSIZE_T)x * RUXNWDDM_BYTES_PER_PIXEL;
	rect.left = (LONG)x;
	rect.top = (LONG)y;
	rect.right = (LONG)(x + width);
	rect.bottom = (LONG)(y + height);
	copy_rect(a, origin, stride, &rect);
}
