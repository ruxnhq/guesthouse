// SPDX-License-Identifier: MIT
//
// The private data passed between ruxnwddm's kernel driver and its
// user-mode driver through the graphics kernel: the adapter's private block
// (read once by the user-mode driver), and the description of each
// allocation (written by the user-mode driver when it allocates, or by the
// kernel driver for the standard allocations Windows makes itself). Both
// halves are built from this header; a change here is a protocol change.

#ifndef RUXNWDDM_UMD_H
#define RUXNWDDM_UMD_H

#define RUXNWDDM_UMD_MAGIC		0x44575852u	// "RXWD", as the escapes
#define RUXNWDDM_UMD_PROTOCOL		2

struct ruxnwddm_umd_adapter_info {
	unsigned int magic;
	unsigned int version;		// the kernel driver's
	unsigned int protocol;		// RUXNWDDM_UMD_PROTOCOL
	unsigned int reserved[5];
};

// What an allocation holds.
#define RUXNWDDM_ALLOC_TEXTURE		1	// a 2D picture, rows of pitch bytes
#define RUXNWDDM_ALLOC_BUFFER		2	// plain bytes

// Flags of an allocation.
#define RUXNWDDM_ALLOC_PRIMARY		0x1	// may be scanned out
#define RUXNWDDM_ALLOC_SHARED		0x2	// opened by other processes
#define RUXNWDDM_ALLOC_CPU_READ		0x4	// read back by the CPU (staging)
// Its pixels are a device surface the user-mode driver made in its own
// task and exported (pvg_surface.c): the kernel driver holds the surface by
// the token while the allocation lives, and a flip to the allocation shows
// the surface with the display's command 8, with no copy.
#define RUXNWDDM_ALLOC_SURFACE		0x8

// A surface is a multiple of 4 pixels wide, so that its rows (4 bytes a
// pixel, which is all the guard takes) are a multiple of 16 bytes apart:
// the device takes a texture over a surface only so (run rn71: "Invalid row
// byte alignment (c18) vs (10)" for a window 774 pixels wide). A texture
// lies over it at its own width (helper scene surface-narrow: 638 pixels
// over a surface of 640), so the padding is never drawn or sampled. A
// texture of one byte a pixel lies over a surface a quarter as wide.
#define RUXNWDDM_SURFACE_WIDTH(width)	(((width) + 3) & ~3u)
#define RUXNWDDM_SURFACE_WIDTH_ONE_BYTE(width)	RUXNWDDM_SURFACE_WIDTH(((width) + 3) / 4)

struct ruxnwddm_alloc_info {
	unsigned int magic;
	unsigned int kind;		// RUXNWDDM_ALLOC_*
	unsigned int flags;
	unsigned int format;		// DXGI_FORMAT for textures, 0 for buffers
	unsigned int width, height, pitch;
	unsigned int reserved;
	unsigned long long size;	// bytes
	unsigned long long surface_token;	// with RUXNWDDM_ALLOC_SURFACE
};

#endif
