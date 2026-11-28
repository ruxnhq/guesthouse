// SPDX-License-Identifier: MIT
//
// Shared declarations of the pvgfb driver: the device's registers, the
// per-device state, and the functions the files call across each other.

#ifndef PVGFB_H
#define PVGFB_H

#include "efi.h"

// The device this driver binds to.
#define PVG_VENDOR_ID 0x106b
#define PVG_DEVICE_ID 0xeeee

// BAR 0 holds the device's registers, all 32 bits wide.
#define PVG_REGISTER_BAR 0

// The early framebuffer registers. Writing PVG_FB_ENABLE_ON turns the early
// framebuffer on; until then the others read 0.
#define PVG_REG_FB_ENABLE 0x1200
#define PVG_REG_MODE_COUNT 0x1204
#define PVG_REG_MODE_SELECT 0x1208
#define PVG_REG_MODE_SIZE 0x120c
#define PVG_REG_FB_PAGE 0x1210
#define PVG_REG_FB_LENGTH 0x1214
#define PVG_REG_FB_DEPTH 0x1218
#define PVG_REG_FB_MODE 0x121c

#define PVG_FB_ENABLE_OFF 0
#define PVG_FB_ENABLE_ON 1

// PVG_REG_MODE_SIZE reads this for a mode index past the last one.
#define PVG_MODE_SIZE_NONE 0xffffffffU

// A mode size packs the height above the width.
#define PVG_MODE_HEIGHT(size) ((size) >> 16)
#define PVG_MODE_WIDTH(size) ((size) & 0xffff)

#define PVG_BITS_PER_PIXEL 32
#define PVG_BYTES_PER_PIXEL 4

// The device takes the framebuffer as a number of 16 KiB pages, and its
// length as a whole number of them.
#define PVG_FB_PAGE_SHIFT 14
#define PVG_FB_PAGE_SIZE (1ULL << PVG_FB_PAGE_SHIFT)

// No more modes than this are offered through the GOP.
#define PVGFB_MAX_MODES 16

typedef struct {
	UINT32 Size; // as the device gives it, height << 16 | width
	EFI_GRAPHICS_OUTPUT_MODE_INFORMATION Info;
} PVGFB_MODE;

typedef struct PVGFB_DEVICE {
	struct PVGFB_DEVICE *Next;

	EFI_HANDLE Controller;
	EFI_HANDLE Child;
	EFI_PCI_IO_PROTOCOL *PciIo;
	UINT64 OriginalAttributes;
	EFI_DEVICE_PATH_PROTOCOL *ChildDevicePath;

	// The whole allocation, and the 16 KiB-aligned framebuffer inside it.
	EFI_PHYSICAL_ADDRESS AllocationBase;
	UINTN AllocationPages;
	EFI_PHYSICAL_ADDRESS FrameBufferBase;
	UINTN FrameBufferCapacity;

	// Set once the framebuffer has been put on the display.
	BOOLEAN Showing;

	UINT32 ModeCount;
	PVGFB_MODE Modes[PVGFB_MAX_MODES];

	EFI_GRAPHICS_OUTPUT_PROTOCOL Gop;
	EFI_GRAPHICS_OUTPUT_PROTOCOL_MODE GopMode;
	EFI_GRAPHICS_OUTPUT_MODE_INFORMATION GopModeInfo;
} PVGFB_DEVICE;

#define PVGFB_FROM_GOP(gop) \
	((PVGFB_DEVICE *)((UINT8 *)(gop) - __builtin_offsetof(PVGFB_DEVICE, Gop)))

extern EFI_BOOT_SERVICES *gBS;

// driver.c: the started device with this controller and, when Child is not
// NULL, this child handle.
PVGFB_DEVICE *PvgfbFindDevice(EFI_HANDLE Controller, EFI_HANDLE Child);

// device.c
EFI_STATUS PvgReadModes(PVGFB_DEVICE *Dev);
EFI_STATUS PvgShowMode(PVGFB_DEVICE *Dev, UINT32 Index, BOOLEAN SetBase);
VOID PvgTurnOff(PVGFB_DEVICE *Dev);

// gop.c
VOID GopInit(PVGFB_DEVICE *Dev);
EFI_STATUS GopSetMode(EFI_GRAPHICS_OUTPUT_PROTOCOL *This, UINT32 ModeNumber);

// lib.c
VOID *memcpy(VOID *Dest, const VOID *Src, UINTN Size);
VOID *memmove(VOID *Dest, const VOID *Src, UINTN Size);
VOID *memset(VOID *Dest, int Value, UINTN Size);

#endif
