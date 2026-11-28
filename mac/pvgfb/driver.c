// SPDX-License-Identifier: MIT
//
// The driver model side: the entry point, and the Supported, Start and Stop
// functions of the driver binding protocol. Start puts a framebuffer on the
// device and hangs a child handle off it that carries the GOP, the way the
// video drivers in edk2 do.

#include "pvgfb.h"

#define PVGFB_DRIVER_VERSION 0x10

EFI_BOOT_SERVICES *gBS;

static EFI_GUID gPciIoGuid = EFI_PCI_IO_PROTOCOL_GUID;
static EFI_GUID gDevicePathGuid = EFI_DEVICE_PATH_PROTOCOL_GUID;
static EFI_GUID gGopGuid = EFI_GRAPHICS_OUTPUT_PROTOCOL_GUID;
static EFI_GUID gDriverBindingGuid = EFI_DRIVER_BINDING_PROTOCOL_GUID;
static EFI_GUID gComponentName2Guid = EFI_COMPONENT_NAME2_PROTOCOL_GUID;

// Every device this driver has started, so Stop can find its state.
static PVGFB_DEVICE *gDevices;

extern EFI_COMPONENT_NAME2_PROTOCOL gPvgfbComponentName2;

PVGFB_DEVICE *PvgfbFindDevice(EFI_HANDLE Controller, EFI_HANDLE Child)
{
	PVGFB_DEVICE *dev;

	for (dev = gDevices; dev != NULL; dev = dev->Next) {
		if (Controller != NULL && dev->Controller != Controller)
			continue;
		if (Child != NULL && dev->Child != Child)
			continue;
		return dev;
	}
	return NULL;
}

static VOID ForgetDevice(PVGFB_DEVICE *Dev)
{
	PVGFB_DEVICE **link;

	for (link = &gDevices; *link != NULL; link = &(*link)->Next) {
		if (*link == Dev) {
			*link = Dev->Next;
			return;
		}
	}
}

static UINTN NodeLength(const EFI_DEVICE_PATH_PROTOCOL *Node)
{
	return Node->Length[0] | (Node->Length[1] << 8);
}

static BOOLEAN IsEndNode(const EFI_DEVICE_PATH_PROTOCOL *Node)
{
	return Node->Type == END_DEVICE_PATH_TYPE &&
		Node->SubType == END_ENTIRE_DEVICE_PATH_SUBTYPE;
}

static VOID SetNode(EFI_DEVICE_PATH_PROTOCOL *Node, UINT8 Type, UINT8 SubType,
	UINTN Length)
{
	Node->Type = Type;
	Node->SubType = SubType;
	Node->Length[0] = (UINT8)Length;
	Node->Length[1] = (UINT8)(Length >> 8);
}

// The child's device path: the PCI device's own path with an ACPI _ADR node
// for the display output on the end.
static EFI_STATUS MakeChildDevicePath(EFI_DEVICE_PATH_PROTOCOL *Parent,
	EFI_DEVICE_PATH_PROTOCOL **Result)
{
	const EFI_DEVICE_PATH_PROTOCOL *node = Parent;
	ACPI_ADR_DEVICE_PATH *adr;
	EFI_DEVICE_PATH_PROTOCOL *end;
	UINTN parentLength = 0;
	UINT8 *path;
	EFI_STATUS status;

	while (!IsEndNode(node)) {
		parentLength += NodeLength(node);
		node = (const EFI_DEVICE_PATH_PROTOCOL *)
			((const UINT8 *)node + NodeLength(node));
	}

	status = gBS->AllocatePool(EfiBootServicesData,
		parentLength + sizeof(*adr) + sizeof(*end), (VOID **)&path);
	if (EFI_ERROR(status))
		return status;

	memcpy(path, Parent, parentLength);
	adr = (ACPI_ADR_DEVICE_PATH *)(path + parentLength);
	SetNode(&adr->Header, ACPI_DEVICE_PATH, ACPI_ADR_DP, sizeof(*adr));
	adr->ADR = ACPI_ADR_DEVICE_ID_SCHEME | ACPI_ADR_BIOS_CAN_DETECT |
		(ACPI_ADR_DISPLAY_TYPE_VGA << ACPI_ADR_DISPLAY_TYPE_SHIFT);
	end = (EFI_DEVICE_PATH_PROTOCOL *)(adr + 1);
	SetNode(end, END_DEVICE_PATH_TYPE, END_ENTIRE_DEVICE_PATH_SUBTYPE,
		sizeof(*end));

	*Result = (EFI_DEVICE_PATH_PROTOCOL *)path;
	return EFI_SUCCESS;
}

// Allocates one framebuffer big enough for the largest mode. It is reserved
// memory so that it survives ExitBootServices without the OS taking it back,
// and it is over-allocated so that a 16 KiB-aligned start fits inside.
static EFI_STATUS AllocateFrameBuffer(PVGFB_DEVICE *Dev)
{
	UINT64 largest = 0;
	UINT32 index;
	EFI_STATUS status;

	for (index = 0; index < Dev->ModeCount; index++) {
		const EFI_GRAPHICS_OUTPUT_MODE_INFORMATION *info =
			&Dev->Modes[index].Info;
		UINT64 bytes = (UINT64)info->PixelsPerScanLine *
			info->VerticalResolution * PVG_BYTES_PER_PIXEL;

		if (bytes > largest)
			largest = bytes;
	}
	largest = (largest + PVG_FB_PAGE_SIZE - 1) & ~(PVG_FB_PAGE_SIZE - 1);

	Dev->AllocationPages = (largest + PVG_FB_PAGE_SIZE - EFI_PAGE_SIZE) >>
		EFI_PAGE_SHIFT;
	status = gBS->AllocatePages(AllocateAnyPages, EfiReservedMemoryType,
		Dev->AllocationPages, &Dev->AllocationBase);
	if (EFI_ERROR(status))
		return status;

	Dev->FrameBufferBase = (Dev->AllocationBase + PVG_FB_PAGE_SIZE - 1) &
		~(PVG_FB_PAGE_SIZE - 1);
	Dev->FrameBufferCapacity = largest;
	return EFI_SUCCESS;
}

static EFI_STATUS EFIAPI PvgfbSupported(EFI_DRIVER_BINDING_PROTOCOL *This,
	EFI_HANDLE Controller, EFI_DEVICE_PATH_PROTOCOL *RemainingDevicePath)
{
	EFI_PCI_IO_PROTOCOL *pciIo;
	UINT32 ids;
	EFI_STATUS status;

	// Only the one child this driver makes can be asked for.
	if (RemainingDevicePath != NULL && !IsEndNode(RemainingDevicePath) &&
	    (RemainingDevicePath->Type != ACPI_DEVICE_PATH ||
	     RemainingDevicePath->SubType != ACPI_ADR_DP ||
	     NodeLength(RemainingDevicePath) != sizeof(ACPI_ADR_DEVICE_PATH)))
		return EFI_UNSUPPORTED;

	status = gBS->OpenProtocol(Controller, &gPciIoGuid, (VOID **)&pciIo,
		This->DriverBindingHandle, Controller,
		EFI_OPEN_PROTOCOL_BY_DRIVER);
	if (EFI_ERROR(status))
		return status;

	status = pciIo->Pci.Read(pciIo, EfiPciIoWidthUint32,
		PCI_VENDOR_ID_OFFSET, 1, &ids);
	if (!EFI_ERROR(status) &&
	    ((ids & 0xffff) != PVG_VENDOR_ID || (ids >> 16) != PVG_DEVICE_ID))
		status = EFI_UNSUPPORTED;

	gBS->CloseProtocol(Controller, &gPciIoGuid, This->DriverBindingHandle,
		Controller);
	return status;
}

static EFI_STATUS EFIAPI PvgfbStart(EFI_DRIVER_BINDING_PROTOCOL *This,
	EFI_HANDLE Controller, EFI_DEVICE_PATH_PROTOCOL *RemainingDevicePath)
{
	PVGFB_DEVICE *dev;
	EFI_DEVICE_PATH_PROTOCOL *parentPath;
	EFI_PCI_IO_PROTOCOL *childPciIo;
	UINT64 supported;
	EFI_STATUS status;

	(void)RemainingDevicePath;

	status = gBS->AllocatePool(EfiBootServicesData, sizeof(*dev),
		(VOID **)&dev);
	if (EFI_ERROR(status))
		return status;
	memset(dev, 0, sizeof(*dev));
	dev->Controller = Controller;

	status = gBS->OpenProtocol(Controller, &gPciIoGuid,
		(VOID **)&dev->PciIo, This->DriverBindingHandle, Controller,
		EFI_OPEN_PROTOCOL_BY_DRIVER);
	if (EFI_ERROR(status))
		goto free_dev;

	status = gBS->OpenProtocol(Controller, &gDevicePathGuid,
		(VOID **)&parentPath, This->DriverBindingHandle, Controller,
		EFI_OPEN_PROTOCOL_GET_PROTOCOL);
	if (EFI_ERROR(status))
		goto close_pci;

	// The firmware leaves memory decoding off, and the registers are in
	// memory space.
	status = dev->PciIo->Attributes(dev->PciIo,
		EfiPciIoAttributeOperationGet, 0, &dev->OriginalAttributes);
	if (EFI_ERROR(status))
		goto close_pci;
	status = dev->PciIo->Attributes(dev->PciIo,
		EfiPciIoAttributeOperationSupported, 0, &supported);
	if (EFI_ERROR(status))
		goto close_pci;
	if (!(supported & EFI_PCI_IO_ATTRIBUTE_MEMORY)) {
		status = EFI_UNSUPPORTED;
		goto close_pci;
	}
	status = dev->PciIo->Attributes(dev->PciIo,
		EfiPciIoAttributeOperationEnable, EFI_PCI_IO_ATTRIBUTE_MEMORY,
		NULL);
	if (EFI_ERROR(status))
		goto close_pci;

	status = PvgReadModes(dev);
	if (EFI_ERROR(status))
		goto turn_off;
	status = AllocateFrameBuffer(dev);
	if (EFI_ERROR(status))
		goto turn_off;

	GopInit(dev);
	status = GopSetMode(&dev->Gop, 0);
	if (EFI_ERROR(status))
		goto free_fb;

	status = MakeChildDevicePath(parentPath, &dev->ChildDevicePath);
	if (EFI_ERROR(status))
		goto free_fb;

	status = gBS->InstallProtocolInterface(&dev->Child, &gDevicePathGuid,
		EFI_NATIVE_INTERFACE, dev->ChildDevicePath);
	if (EFI_ERROR(status))
		goto free_path;
	status = gBS->InstallProtocolInterface(&dev->Child, &gGopGuid,
		EFI_NATIVE_INTERFACE, &dev->Gop);
	if (EFI_ERROR(status))
		goto uninstall_path;

	// Ties the child to the controller, so that disconnecting the
	// controller stops the child first.
	status = gBS->OpenProtocol(Controller, &gPciIoGuid,
		(VOID **)&childPciIo, This->DriverBindingHandle, dev->Child,
		EFI_OPEN_PROTOCOL_BY_CHILD_CONTROLLER);
	if (EFI_ERROR(status))
		goto uninstall_gop;

	dev->Next = gDevices;
	gDevices = dev;
	return EFI_SUCCESS;

uninstall_gop:
	gBS->UninstallProtocolInterface(dev->Child, &gGopGuid, &dev->Gop);
uninstall_path:
	gBS->UninstallProtocolInterface(dev->Child, &gDevicePathGuid,
		dev->ChildDevicePath);
free_path:
	gBS->FreePool(dev->ChildDevicePath);
free_fb:
	gBS->FreePages(dev->AllocationBase, dev->AllocationPages);
turn_off:
	PvgTurnOff(dev);
	dev->PciIo->Attributes(dev->PciIo, EfiPciIoAttributeOperationSet,
		dev->OriginalAttributes, NULL);
close_pci:
	gBS->CloseProtocol(Controller, &gPciIoGuid, This->DriverBindingHandle,
		Controller);
free_dev:
	gBS->FreePool(dev);
	return status;
}

static EFI_STATUS StopChild(EFI_DRIVER_BINDING_PROTOCOL *This,
	EFI_HANDLE Controller, EFI_HANDLE Child)
{
	PVGFB_DEVICE *dev = PvgfbFindDevice(Controller, Child);
	EFI_PCI_IO_PROTOCOL *pciIo;
	EFI_STATUS status;

	if (dev == NULL)
		return EFI_DEVICE_ERROR;

	gBS->CloseProtocol(Controller, &gPciIoGuid, This->DriverBindingHandle,
		Child);

	status = gBS->UninstallProtocolInterface(Child, &gGopGuid, &dev->Gop);
	if (EFI_ERROR(status))
		goto reopen;
	status = gBS->UninstallProtocolInterface(Child, &gDevicePathGuid,
		dev->ChildDevicePath);
	if (EFI_ERROR(status)) {
		gBS->InstallProtocolInterface(&Child, &gGopGuid,
			EFI_NATIVE_INTERFACE, &dev->Gop);
		goto reopen;
	}

	PvgTurnOff(dev);
	dev->Showing = FALSE;
	gBS->FreePool(dev->ChildDevicePath);
	dev->ChildDevicePath = NULL;
	dev->Child = NULL;
	return EFI_SUCCESS;

reopen:
	// Something still uses the GOP, so the child stays as it was.
	gBS->OpenProtocol(Controller, &gPciIoGuid, (VOID **)&pciIo,
		This->DriverBindingHandle, Child,
		EFI_OPEN_PROTOCOL_BY_CHILD_CONTROLLER);
	return status;
}

static EFI_STATUS EFIAPI PvgfbStop(EFI_DRIVER_BINDING_PROTOCOL *This,
	EFI_HANDLE Controller, UINTN NumberOfChildren,
	EFI_HANDLE *ChildHandleBuffer)
{
	PVGFB_DEVICE *dev;
	EFI_STATUS result = EFI_SUCCESS;
	UINTN index;

	if (NumberOfChildren > 0) {
		for (index = 0; index < NumberOfChildren; index++) {
			EFI_STATUS status = StopChild(This, Controller,
				ChildHandleBuffer[index]);

			if (EFI_ERROR(status))
				result = status;
		}
		return result;
	}

	dev = PvgfbFindDevice(Controller, NULL);
	if (dev == NULL)
		return EFI_DEVICE_ERROR;

	if (dev->Showing) {
		PvgTurnOff(dev);
		dev->Showing = FALSE;
	}
	gBS->FreePages(dev->AllocationBase, dev->AllocationPages);
	dev->PciIo->Attributes(dev->PciIo, EfiPciIoAttributeOperationSet,
		dev->OriginalAttributes, NULL);
	gBS->CloseProtocol(Controller, &gPciIoGuid, This->DriverBindingHandle,
		Controller);
	ForgetDevice(dev);
	gBS->FreePool(dev);
	return EFI_SUCCESS;
}

static EFI_DRIVER_BINDING_PROTOCOL gPvgfbDriverBinding = {
	.Supported = PvgfbSupported,
	.Start = PvgfbStart,
	.Stop = PvgfbStop,
	.Version = PVGFB_DRIVER_VERSION,
};

EFI_STATUS EFIAPI PvgfbEntry(EFI_HANDLE ImageHandle,
	EFI_SYSTEM_TABLE *SystemTable)
{
	EFI_HANDLE handle = ImageHandle;
	EFI_STATUS status;

	gBS = SystemTable->BootServices;

	gPvgfbDriverBinding.ImageHandle = ImageHandle;
	gPvgfbDriverBinding.DriverBindingHandle = ImageHandle;

	status = gBS->InstallProtocolInterface(&handle, &gDriverBindingGuid,
		EFI_NATIVE_INTERFACE, &gPvgfbDriverBinding);
	if (EFI_ERROR(status))
		return status;

	status = gBS->InstallProtocolInterface(&handle, &gComponentName2Guid,
		EFI_NATIVE_INTERFACE, &gPvgfbComponentName2);
	if (EFI_ERROR(status))
		gBS->UninstallProtocolInterface(handle, &gDriverBindingGuid,
			&gPvgfbDriverBinding);
	return status;
}
