// SPDX-License-Identifier: MIT
//
// The device side: the early framebuffer registers in BAR 0, written in the
// order the device expects. Nothing else in the BAR is touched.

#include "pvgfb.h"

static EFI_STATUS RegRead(PVGFB_DEVICE *Dev, UINT32 Offset, UINT32 *Value)
{
	return Dev->PciIo->Mem.Read(Dev->PciIo, EfiPciIoWidthUint32,
		PVG_REGISTER_BAR, Offset, 1, Value);
}

static EFI_STATUS RegWrite(PVGFB_DEVICE *Dev, UINT32 Offset, UINT32 Value)
{
	return Dev->PciIo->Mem.Write(Dev->PciIo, EfiPciIoWidthUint32,
		PVG_REGISTER_BAR, Offset, 1, &Value);
}

static UINT64 FrameBufferLength(const PVGFB_MODE *Mode)
{
	UINT64 bytes = (UINT64)Mode->Info.HorizontalResolution *
		Mode->Info.VerticalResolution * PVG_BYTES_PER_PIXEL;

	return (bytes + PVG_FB_PAGE_SIZE - 1) & ~(PVG_FB_PAGE_SIZE - 1);
}

// Turns the early framebuffer on and reads the list of modes the device
// offers, keeping at most PVGFB_MAX_MODES of them.
EFI_STATUS PvgReadModes(PVGFB_DEVICE *Dev)
{
	EFI_STATUS status;
	UINT32 count, index;

	status = RegWrite(Dev, PVG_REG_FB_ENABLE, PVG_FB_ENABLE_ON);
	if (EFI_ERROR(status))
		return status;
	status = RegRead(Dev, PVG_REG_MODE_COUNT, &count);
	if (EFI_ERROR(status))
		return status;
	if (count > PVGFB_MAX_MODES)
		count = PVGFB_MAX_MODES;

	Dev->ModeCount = 0;
	for (index = 0; index < count; index++) {
		PVGFB_MODE *mode = &Dev->Modes[Dev->ModeCount];
		UINT32 size;

		status = RegWrite(Dev, PVG_REG_MODE_SELECT, index);
		if (EFI_ERROR(status))
			return status;
		status = RegRead(Dev, PVG_REG_MODE_SIZE, &size);
		if (EFI_ERROR(status))
			return status;
		if (size == PVG_MODE_SIZE_NONE)
			break;
		if (PVG_MODE_WIDTH(size) == 0 || PVG_MODE_HEIGHT(size) == 0)
			continue;

		memset(mode, 0, sizeof(*mode));
		mode->Size = size;
		mode->Info.Version = 0;
		mode->Info.HorizontalResolution = PVG_MODE_WIDTH(size);
		mode->Info.VerticalResolution = PVG_MODE_HEIGHT(size);
		mode->Info.PixelFormat = PixelBlueGreenRedReserved8BitPerColor;
		mode->Info.PixelsPerScanLine = PVG_MODE_WIDTH(size);
		Dev->ModeCount++;
	}

	return Dev->ModeCount ? EFI_SUCCESS : EFI_UNSUPPORTED;
}

// Puts the framebuffer on the display in the given mode. The start address
// only needs writing the first time, as it never moves.
EFI_STATUS PvgShowMode(PVGFB_DEVICE *Dev, UINT32 Index, BOOLEAN SetBase)
{
	const PVGFB_MODE *mode = &Dev->Modes[Index];
	UINT64 length = FrameBufferLength(mode);
	EFI_STATUS status;

	if (length > Dev->FrameBufferCapacity)
		return EFI_UNSUPPORTED;

	if (SetBase) {
		status = RegWrite(Dev, PVG_REG_FB_PAGE,
			(UINT32)(Dev->FrameBufferBase >> PVG_FB_PAGE_SHIFT));
		if (EFI_ERROR(status))
			return status;
	}
	status = RegWrite(Dev, PVG_REG_FB_LENGTH, (UINT32)length);
	if (EFI_ERROR(status))
		return status;
	status = RegWrite(Dev, PVG_REG_FB_DEPTH, PVG_BITS_PER_PIXEL);
	if (EFI_ERROR(status))
		return status;
	// The picture appears with this write.
	return RegWrite(Dev, PVG_REG_FB_MODE, mode->Size);
}

VOID PvgTurnOff(PVGFB_DEVICE *Dev)
{
	RegWrite(Dev, PVG_REG_FB_ENABLE, PVG_FB_ENABLE_OFF);
}
