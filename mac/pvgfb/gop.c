// SPDX-License-Identifier: MIT
//
// The Graphics Output Protocol on top of the device's framebuffer. The
// framebuffer is ordinary memory, so Blt is plain copying in software.

#include "pvgfb.h"

static EFI_STATUS EFIAPI GopQueryMode(EFI_GRAPHICS_OUTPUT_PROTOCOL *This,
	UINT32 ModeNumber, UINTN *SizeOfInfo,
	EFI_GRAPHICS_OUTPUT_MODE_INFORMATION **Info)
{
	PVGFB_DEVICE *dev;
	EFI_GRAPHICS_OUTPUT_MODE_INFORMATION *info;
	EFI_STATUS status;

	if (This == NULL || SizeOfInfo == NULL || Info == NULL)
		return EFI_INVALID_PARAMETER;
	dev = PVGFB_FROM_GOP(This);
	if (ModeNumber >= dev->ModeCount)
		return EFI_INVALID_PARAMETER;

	// The caller frees what it is given, so it comes from the pool.
	status = gBS->AllocatePool(EfiBootServicesData, sizeof(*info),
		(VOID **)&info);
	if (EFI_ERROR(status))
		return status;
	*info = dev->Modes[ModeNumber].Info;
	*SizeOfInfo = sizeof(*info);
	*Info = info;
	return EFI_SUCCESS;
}

static UINT32 *Pixel(PVGFB_DEVICE *Dev, UINTN X, UINTN Y)
{
	UINT32 *fb = (UINT32 *)(UINTN)Dev->FrameBufferBase;

	return fb + Y * Dev->GopModeInfo.PixelsPerScanLine + X;
}

EFI_STATUS EFIAPI GopSetMode(EFI_GRAPHICS_OUTPUT_PROTOCOL *This,
	UINT32 ModeNumber)
{
	PVGFB_DEVICE *dev;
	EFI_GRAPHICS_OUTPUT_MODE_INFORMATION *info;
	EFI_STATUS status;

	if (This == NULL)
		return EFI_INVALID_PARAMETER;
	dev = PVGFB_FROM_GOP(This);
	if (ModeNumber >= dev->ModeCount)
		return EFI_UNSUPPORTED;

	info = &dev->Modes[ModeNumber].Info;

	// Clear first, so the new size never shows what the old one left.
	memset((VOID *)(UINTN)dev->FrameBufferBase, 0,
		(UINTN)info->PixelsPerScanLine * info->VerticalResolution *
		PVG_BYTES_PER_PIXEL);

	if (!dev->Showing || ModeNumber != dev->GopMode.Mode) {
		status = PvgShowMode(dev, ModeNumber, !dev->Showing);
		if (EFI_ERROR(status))
			return EFI_DEVICE_ERROR;
		dev->Showing = TRUE;
	}

	dev->GopModeInfo = *info;
	dev->GopMode.Mode = ModeNumber;
	dev->GopMode.FrameBufferSize = (UINTN)info->PixelsPerScanLine *
		info->VerticalResolution * PVG_BYTES_PER_PIXEL;
	return EFI_SUCCESS;
}

// True when a Width by Height rectangle at X, Y fits on the screen.
static BOOLEAN FitsScreen(PVGFB_DEVICE *Dev, UINTN X, UINTN Y, UINTN Width,
	UINTN Height)
{
	UINTN screenWidth = Dev->GopModeInfo.HorizontalResolution;
	UINTN screenHeight = Dev->GopModeInfo.VerticalResolution;

	return X <= screenWidth && Width <= screenWidth - X &&
		Y <= screenHeight && Height <= screenHeight - Y;
}

static VOID VideoFill(PVGFB_DEVICE *Dev, UINT32 Color, UINTN X, UINTN Y,
	UINTN Width, UINTN Height)
{
	UINTN row, column;

	for (row = 0; row < Height; row++) {
		UINT32 *line = Pixel(Dev, X, Y + row);

		for (column = 0; column < Width; column++)
			line[column] = Color;
	}
}

static VOID VideoToBuffer(PVGFB_DEVICE *Dev, UINT8 *Buffer, UINTN Delta,
	UINTN SourceX, UINTN SourceY, UINTN DestX, UINTN DestY, UINTN Width,
	UINTN Height)
{
	UINTN row;

	for (row = 0; row < Height; row++)
		memcpy(Buffer + (DestY + row) * Delta + DestX * PVG_BYTES_PER_PIXEL,
			Pixel(Dev, SourceX, SourceY + row),
			Width * PVG_BYTES_PER_PIXEL);
}

static VOID BufferToVideo(PVGFB_DEVICE *Dev, const UINT8 *Buffer,
	UINTN Delta, UINTN SourceX, UINTN SourceY, UINTN DestX, UINTN DestY,
	UINTN Width, UINTN Height)
{
	UINTN row;

	for (row = 0; row < Height; row++)
		memcpy(Pixel(Dev, DestX, DestY + row),
			Buffer + (SourceY + row) * Delta +
			SourceX * PVG_BYTES_PER_PIXEL,
			Width * PVG_BYTES_PER_PIXEL);
}

// Copies within the screen. When the rectangles overlap and the destination
// is lower down, the rows are copied from the bottom up so no source row is
// overwritten before it is read; memmove takes care of overlap within a row.
static VOID VideoToVideo(PVGFB_DEVICE *Dev, UINTN SourceX, UINTN SourceY,
	UINTN DestX, UINTN DestY, UINTN Width, UINTN Height)
{
	UINTN bytes = Width * PVG_BYTES_PER_PIXEL;
	UINTN row;

	if (DestY > SourceY) {
		for (row = Height; row-- > 0;)
			memmove(Pixel(Dev, DestX, DestY + row),
				Pixel(Dev, SourceX, SourceY + row), bytes);
	} else {
		for (row = 0; row < Height; row++)
			memmove(Pixel(Dev, DestX, DestY + row),
				Pixel(Dev, SourceX, SourceY + row), bytes);
	}
}

static EFI_STATUS EFIAPI GopBlt(EFI_GRAPHICS_OUTPUT_PROTOCOL *This,
	EFI_GRAPHICS_OUTPUT_BLT_PIXEL *BltBuffer,
	EFI_GRAPHICS_OUTPUT_BLT_OPERATION BltOperation, UINTN SourceX,
	UINTN SourceY, UINTN DestinationX, UINTN DestinationY, UINTN Width,
	UINTN Height, UINTN Delta)
{
	PVGFB_DEVICE *dev;
	UINT32 color;

	if (This == NULL || Width == 0 || Height == 0)
		return EFI_INVALID_PARAMETER;
	dev = PVGFB_FROM_GOP(This);
	if (!dev->Showing)
		return EFI_DEVICE_ERROR;

	// A Delta of 0 means the buffer is exactly Width pixels wide. It is
	// not used for a video fill or a copy within the screen.
	if (Delta == 0)
		Delta = Width * sizeof(EFI_GRAPHICS_OUTPUT_BLT_PIXEL);

	switch (BltOperation) {
	case EfiBltVideoFill:
		if (BltBuffer == NULL ||
		    !FitsScreen(dev, DestinationX, DestinationY, Width, Height))
			return EFI_INVALID_PARAMETER;
		memcpy(&color, BltBuffer, sizeof(color));
		VideoFill(dev, color, DestinationX, DestinationY, Width, Height);
		return EFI_SUCCESS;

	case EfiBltVideoToBltBuffer:
		if (BltBuffer == NULL ||
		    !FitsScreen(dev, SourceX, SourceY, Width, Height))
			return EFI_INVALID_PARAMETER;
		VideoToBuffer(dev, (UINT8 *)BltBuffer, Delta, SourceX, SourceY,
			DestinationX, DestinationY, Width, Height);
		return EFI_SUCCESS;

	case EfiBltBufferToVideo:
		if (BltBuffer == NULL ||
		    !FitsScreen(dev, DestinationX, DestinationY, Width, Height))
			return EFI_INVALID_PARAMETER;
		BufferToVideo(dev, (const UINT8 *)BltBuffer, Delta, SourceX,
			SourceY, DestinationX, DestinationY, Width, Height);
		return EFI_SUCCESS;

	case EfiBltVideoToVideo:
		if (!FitsScreen(dev, SourceX, SourceY, Width, Height) ||
		    !FitsScreen(dev, DestinationX, DestinationY, Width, Height))
			return EFI_INVALID_PARAMETER;
		VideoToVideo(dev, SourceX, SourceY, DestinationX, DestinationY,
			Width, Height);
		return EFI_SUCCESS;

	default:
		return EFI_INVALID_PARAMETER;
	}
}

// Fills in the protocol and its mode structure. The mode itself is set
// afterwards, by a first call to GopSetMode.
VOID GopInit(PVGFB_DEVICE *Dev)
{
	Dev->Gop.QueryMode = GopQueryMode;
	Dev->Gop.SetMode = GopSetMode;
	Dev->Gop.Blt = GopBlt;
	Dev->Gop.Mode = &Dev->GopMode;

	Dev->GopMode.MaxMode = Dev->ModeCount;
	Dev->GopMode.Mode = 0;
	Dev->GopMode.Info = &Dev->GopModeInfo;
	Dev->GopMode.SizeOfInfo = sizeof(Dev->GopModeInfo);
	Dev->GopMode.FrameBufferBase = Dev->FrameBufferBase;
	Dev->GopMode.FrameBufferSize = 0;
}
