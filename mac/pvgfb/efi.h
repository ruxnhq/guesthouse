// SPDX-License-Identifier: MIT
//
// The small part of the UEFI interface this driver uses, written from the
// UEFI specification (2.10). Only the types, GUIDs and protocol layouts that
// pvgfb needs are here. Table entries the driver never calls are kept as
// plain pointers so that the offsets of the ones it does call stay right.

#ifndef PVGFB_EFI_H
#define PVGFB_EFI_H

typedef unsigned char UINT8;
typedef unsigned short UINT16;
typedef unsigned int UINT32;
typedef unsigned long long UINT64;
typedef long long INT64;
typedef UINT64 UINTN;
typedef UINT8 BOOLEAN;
typedef unsigned short CHAR16;
typedef char CHAR8;
typedef void VOID;

typedef UINTN EFI_STATUS;
typedef VOID *EFI_HANDLE;
typedef VOID *EFI_EVENT;
typedef UINT64 EFI_PHYSICAL_ADDRESS;
typedef UINTN EFI_TPL;

#define NULL ((VOID *)0)
#define TRUE 1
#define FALSE 0

// The calling convention is the platform's own on AArch64.
#define EFIAPI

#define EFI_ERROR_BIT (1ULL << 63)
#define EFI_ERROR(status) (((INT64)(status)) < 0)

#define EFI_SUCCESS 0
#define EFI_INVALID_PARAMETER (EFI_ERROR_BIT | 2)
#define EFI_UNSUPPORTED (EFI_ERROR_BIT | 3)
#define EFI_BUFFER_TOO_SMALL (EFI_ERROR_BIT | 5)
#define EFI_DEVICE_ERROR (EFI_ERROR_BIT | 7)
#define EFI_OUT_OF_RESOURCES (EFI_ERROR_BIT | 9)
#define EFI_NOT_FOUND (EFI_ERROR_BIT | 14)

typedef struct {
	UINT32 Data1;
	UINT16 Data2;
	UINT16 Data3;
	UINT8 Data4[8];
} EFI_GUID;

typedef struct {
	UINT64 Signature;
	UINT32 Revision;
	UINT32 HeaderSize;
	UINT32 CRC32;
	UINT32 Reserved;
} EFI_TABLE_HEADER;

// Memory

typedef enum {
	AllocateAnyPages,
	AllocateMaxAddress,
	AllocateAddress,
	MaxAllocateType
} EFI_ALLOCATE_TYPE;

typedef enum {
	EfiReservedMemoryType,
	EfiLoaderCode,
	EfiLoaderData,
	EfiBootServicesCode,
	EfiBootServicesData,
	EfiRuntimeServicesCode,
	EfiRuntimeServicesData,
	EfiConventionalMemory,
} EFI_MEMORY_TYPE;

#define EFI_PAGE_SIZE 4096
#define EFI_PAGE_SHIFT 12

typedef enum {
	EFI_NATIVE_INTERFACE
} EFI_INTERFACE_TYPE;

// Device paths

typedef struct {
	UINT8 Type;
	UINT8 SubType;
	UINT8 Length[2];
} EFI_DEVICE_PATH_PROTOCOL;

#define ACPI_DEVICE_PATH 0x02
#define ACPI_ADR_DP 0x03
#define END_DEVICE_PATH_TYPE 0x7f
#define END_ENTIRE_DEVICE_PATH_SUBTYPE 0xff

typedef struct {
	EFI_DEVICE_PATH_PROTOCOL Header;
	UINT32 ADR;
} ACPI_ADR_DEVICE_PATH;

// Bits of an ACPI _ADR value for a display output, from the ACPI
// specification's table of video output device attributes.
#define ACPI_ADR_DEVICE_ID_SCHEME (1U << 31)
#define ACPI_ADR_BIOS_CAN_DETECT (1U << 16)
#define ACPI_ADR_DISPLAY_TYPE_SHIFT 8
#define ACPI_ADR_DISPLAY_TYPE_VGA 1

// Open protocol attributes

#define EFI_OPEN_PROTOCOL_BY_HANDLE_PROTOCOL 0x01
#define EFI_OPEN_PROTOCOL_GET_PROTOCOL 0x02
#define EFI_OPEN_PROTOCOL_TEST_PROTOCOL 0x04
#define EFI_OPEN_PROTOCOL_BY_CHILD_CONTROLLER 0x08
#define EFI_OPEN_PROTOCOL_BY_DRIVER 0x10
#define EFI_OPEN_PROTOCOL_EXCLUSIVE 0x20

// Boot services

typedef EFI_STATUS(EFIAPI *EFI_ALLOCATE_PAGES)(EFI_ALLOCATE_TYPE Type,
	EFI_MEMORY_TYPE MemoryType, UINTN Pages, EFI_PHYSICAL_ADDRESS *Memory);
typedef EFI_STATUS(EFIAPI *EFI_FREE_PAGES)(EFI_PHYSICAL_ADDRESS Memory,
	UINTN Pages);
typedef EFI_STATUS(EFIAPI *EFI_ALLOCATE_POOL)(EFI_MEMORY_TYPE PoolType,
	UINTN Size, VOID **Buffer);
typedef EFI_STATUS(EFIAPI *EFI_FREE_POOL)(VOID *Buffer);
typedef EFI_STATUS(EFIAPI *EFI_INSTALL_PROTOCOL_INTERFACE)(EFI_HANDLE *Handle,
	EFI_GUID *Protocol, EFI_INTERFACE_TYPE InterfaceType, VOID *Interface);
typedef EFI_STATUS(EFIAPI *EFI_UNINSTALL_PROTOCOL_INTERFACE)(EFI_HANDLE Handle,
	EFI_GUID *Protocol, VOID *Interface);
typedef EFI_STATUS(EFIAPI *EFI_OPEN_PROTOCOL)(EFI_HANDLE Handle,
	EFI_GUID *Protocol, VOID **Interface, EFI_HANDLE AgentHandle,
	EFI_HANDLE ControllerHandle, UINT32 Attributes);
typedef EFI_STATUS(EFIAPI *EFI_CLOSE_PROTOCOL)(EFI_HANDLE Handle,
	EFI_GUID *Protocol, EFI_HANDLE AgentHandle, EFI_HANDLE ControllerHandle);

typedef struct {
	EFI_TABLE_HEADER Hdr;

	VOID *RaiseTPL;
	VOID *RestoreTPL;

	EFI_ALLOCATE_PAGES AllocatePages;
	EFI_FREE_PAGES FreePages;
	VOID *GetMemoryMap;
	EFI_ALLOCATE_POOL AllocatePool;
	EFI_FREE_POOL FreePool;

	VOID *CreateEvent;
	VOID *SetTimer;
	VOID *WaitForEvent;
	VOID *SignalEvent;
	VOID *CloseEvent;
	VOID *CheckEvent;

	EFI_INSTALL_PROTOCOL_INTERFACE InstallProtocolInterface;
	VOID *ReinstallProtocolInterface;
	EFI_UNINSTALL_PROTOCOL_INTERFACE UninstallProtocolInterface;
	VOID *HandleProtocol;
	VOID *Reserved;
	VOID *RegisterProtocolNotify;
	VOID *LocateHandle;
	VOID *LocateDevicePath;
	VOID *InstallConfigurationTable;

	VOID *LoadImage;
	VOID *StartImage;
	VOID *Exit;
	VOID *UnloadImage;
	VOID *ExitBootServices;

	VOID *GetNextMonotonicCount;
	VOID *Stall;
	VOID *SetWatchdogTimer;

	VOID *ConnectController;
	VOID *DisconnectController;

	EFI_OPEN_PROTOCOL OpenProtocol;
	EFI_CLOSE_PROTOCOL CloseProtocol;
	VOID *OpenProtocolInformation;

	VOID *ProtocolsPerHandle;
	VOID *LocateHandleBuffer;
	VOID *LocateProtocol;
	VOID *InstallMultipleProtocolInterfaces;
	VOID *UninstallMultipleProtocolInterfaces;

	VOID *CalculateCrc32;

	VOID *CopyMem;
	VOID *SetMem;
	VOID *CreateEventEx;
} EFI_BOOT_SERVICES;

typedef struct {
	EFI_TABLE_HEADER Hdr;
	CHAR16 *FirmwareVendor;
	UINT32 FirmwareRevision;
	EFI_HANDLE ConsoleInHandle;
	VOID *ConIn;
	EFI_HANDLE ConsoleOutHandle;
	VOID *ConOut;
	EFI_HANDLE StandardErrorHandle;
	VOID *StdErr;
	VOID *RuntimeServices;
	EFI_BOOT_SERVICES *BootServices;
	UINTN NumberOfTableEntries;
	VOID *ConfigurationTable;
} EFI_SYSTEM_TABLE;

// Driver binding protocol

#define EFI_DRIVER_BINDING_PROTOCOL_GUID \
	{ 0x18a031ab, 0xb443, 0x4d1a, \
	  { 0xa5, 0xc0, 0x0c, 0x09, 0x26, 0x1e, 0x9f, 0x71 } }

typedef struct EFI_DRIVER_BINDING_PROTOCOL EFI_DRIVER_BINDING_PROTOCOL;

typedef EFI_STATUS(EFIAPI *EFI_DRIVER_BINDING_SUPPORTED)(
	EFI_DRIVER_BINDING_PROTOCOL *This, EFI_HANDLE ControllerHandle,
	EFI_DEVICE_PATH_PROTOCOL *RemainingDevicePath);
typedef EFI_STATUS(EFIAPI *EFI_DRIVER_BINDING_START)(
	EFI_DRIVER_BINDING_PROTOCOL *This, EFI_HANDLE ControllerHandle,
	EFI_DEVICE_PATH_PROTOCOL *RemainingDevicePath);
typedef EFI_STATUS(EFIAPI *EFI_DRIVER_BINDING_STOP)(
	EFI_DRIVER_BINDING_PROTOCOL *This, EFI_HANDLE ControllerHandle,
	UINTN NumberOfChildren, EFI_HANDLE *ChildHandleBuffer);

struct EFI_DRIVER_BINDING_PROTOCOL {
	EFI_DRIVER_BINDING_SUPPORTED Supported;
	EFI_DRIVER_BINDING_START Start;
	EFI_DRIVER_BINDING_STOP Stop;
	UINT32 Version;
	EFI_HANDLE ImageHandle;
	EFI_HANDLE DriverBindingHandle;
};

// Component name 2 protocol, which gives the driver a name in the shell's
// "drivers" and "dh" listings.

#define EFI_COMPONENT_NAME2_PROTOCOL_GUID \
	{ 0x6a7a5cff, 0xe8d9, 0x4f70, \
	  { 0xba, 0xda, 0x75, 0xab, 0x30, 0x25, 0xce, 0x14 } }

typedef struct EFI_COMPONENT_NAME2_PROTOCOL EFI_COMPONENT_NAME2_PROTOCOL;

typedef EFI_STATUS(EFIAPI *EFI_COMPONENT_NAME_GET_DRIVER_NAME)(
	EFI_COMPONENT_NAME2_PROTOCOL *This, CHAR8 *Language,
	CHAR16 **DriverName);
typedef EFI_STATUS(EFIAPI *EFI_COMPONENT_NAME_GET_CONTROLLER_NAME)(
	EFI_COMPONENT_NAME2_PROTOCOL *This, EFI_HANDLE ControllerHandle,
	EFI_HANDLE ChildHandle, CHAR8 *Language, CHAR16 **ControllerName);

struct EFI_COMPONENT_NAME2_PROTOCOL {
	EFI_COMPONENT_NAME_GET_DRIVER_NAME GetDriverName;
	EFI_COMPONENT_NAME_GET_CONTROLLER_NAME GetControllerName;
	CHAR8 *SupportedLanguages;
};

// Device path protocol

#define EFI_DEVICE_PATH_PROTOCOL_GUID \
	{ 0x09576e91, 0x6d3f, 0x11d2, \
	  { 0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b } }

// PCI I/O protocol

#define EFI_PCI_IO_PROTOCOL_GUID \
	{ 0x4cf5b200, 0x68b8, 0x4ca5, \
	  { 0x9e, 0xec, 0xb2, 0x3e, 0x3f, 0x50, 0x02, 0x9a } }

typedef enum {
	EfiPciIoWidthUint8,
	EfiPciIoWidthUint16,
	EfiPciIoWidthUint32,
	EfiPciIoWidthUint64,
} EFI_PCI_IO_PROTOCOL_WIDTH;

typedef enum {
	EfiPciIoAttributeOperationGet,
	EfiPciIoAttributeOperationSet,
	EfiPciIoAttributeOperationEnable,
	EfiPciIoAttributeOperationDisable,
	EfiPciIoAttributeOperationSupported,
} EFI_PCI_IO_PROTOCOL_ATTRIBUTE_OPERATION;

#define EFI_PCI_IO_ATTRIBUTE_MEMORY 0x0200

typedef struct EFI_PCI_IO_PROTOCOL EFI_PCI_IO_PROTOCOL;

typedef EFI_STATUS(EFIAPI *EFI_PCI_IO_PROTOCOL_MEM)(EFI_PCI_IO_PROTOCOL *This,
	EFI_PCI_IO_PROTOCOL_WIDTH Width, UINT8 BarIndex, UINT64 Offset,
	UINTN Count, VOID *Buffer);
typedef EFI_STATUS(EFIAPI *EFI_PCI_IO_PROTOCOL_CONFIG)(
	EFI_PCI_IO_PROTOCOL *This, EFI_PCI_IO_PROTOCOL_WIDTH Width,
	UINT32 Offset, UINTN Count, VOID *Buffer);
typedef EFI_STATUS(EFIAPI *EFI_PCI_IO_PROTOCOL_ATTRIBUTES)(
	EFI_PCI_IO_PROTOCOL *This,
	EFI_PCI_IO_PROTOCOL_ATTRIBUTE_OPERATION Operation, UINT64 Attributes,
	UINT64 *Result);

typedef struct {
	EFI_PCI_IO_PROTOCOL_MEM Read;
	EFI_PCI_IO_PROTOCOL_MEM Write;
} EFI_PCI_IO_PROTOCOL_ACCESS;

typedef struct {
	EFI_PCI_IO_PROTOCOL_CONFIG Read;
	EFI_PCI_IO_PROTOCOL_CONFIG Write;
} EFI_PCI_IO_PROTOCOL_CONFIG_ACCESS;

struct EFI_PCI_IO_PROTOCOL {
	VOID *PollMem;
	VOID *PollIo;
	EFI_PCI_IO_PROTOCOL_ACCESS Mem;
	EFI_PCI_IO_PROTOCOL_ACCESS Io;
	EFI_PCI_IO_PROTOCOL_CONFIG_ACCESS Pci;
	VOID *CopyMem;
	VOID *Map;
	VOID *Unmap;
	VOID *AllocateBuffer;
	VOID *FreeBuffer;
	VOID *Flush;
	VOID *GetLocation;
	EFI_PCI_IO_PROTOCOL_ATTRIBUTES Attributes;
	VOID *GetBarAttributes;
	VOID *SetBarAttributes;
	UINT64 RomSize;
	VOID *RomImage;
};

// Offsets in PCI configuration space
#define PCI_VENDOR_ID_OFFSET 0x00

// Graphics output protocol

#define EFI_GRAPHICS_OUTPUT_PROTOCOL_GUID \
	{ 0x9042a9de, 0x23dc, 0x4a38, \
	  { 0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a } }

typedef enum {
	PixelRedGreenBlueReserved8BitPerColor,
	PixelBlueGreenRedReserved8BitPerColor,
	PixelBitMask,
	PixelBltOnly,
	PixelFormatMax
} EFI_GRAPHICS_PIXEL_FORMAT;

typedef struct {
	UINT32 RedMask;
	UINT32 GreenMask;
	UINT32 BlueMask;
	UINT32 ReservedMask;
} EFI_PIXEL_BITMASK;

typedef struct {
	UINT32 Version;
	UINT32 HorizontalResolution;
	UINT32 VerticalResolution;
	EFI_GRAPHICS_PIXEL_FORMAT PixelFormat;
	EFI_PIXEL_BITMASK PixelInformation;
	UINT32 PixelsPerScanLine;
} EFI_GRAPHICS_OUTPUT_MODE_INFORMATION;

typedef struct {
	UINT32 MaxMode;
	UINT32 Mode;
	EFI_GRAPHICS_OUTPUT_MODE_INFORMATION *Info;
	UINTN SizeOfInfo;
	EFI_PHYSICAL_ADDRESS FrameBufferBase;
	UINTN FrameBufferSize;
} EFI_GRAPHICS_OUTPUT_PROTOCOL_MODE;

typedef struct {
	UINT8 Blue;
	UINT8 Green;
	UINT8 Red;
	UINT8 Reserved;
} EFI_GRAPHICS_OUTPUT_BLT_PIXEL;

typedef enum {
	EfiBltVideoFill,
	EfiBltVideoToBltBuffer,
	EfiBltBufferToVideo,
	EfiBltVideoToVideo,
	EfiGraphicsOutputBltOperationMax
} EFI_GRAPHICS_OUTPUT_BLT_OPERATION;

typedef struct EFI_GRAPHICS_OUTPUT_PROTOCOL EFI_GRAPHICS_OUTPUT_PROTOCOL;

typedef EFI_STATUS(EFIAPI *EFI_GRAPHICS_OUTPUT_PROTOCOL_QUERY_MODE)(
	EFI_GRAPHICS_OUTPUT_PROTOCOL *This, UINT32 ModeNumber,
	UINTN *SizeOfInfo, EFI_GRAPHICS_OUTPUT_MODE_INFORMATION **Info);
typedef EFI_STATUS(EFIAPI *EFI_GRAPHICS_OUTPUT_PROTOCOL_SET_MODE)(
	EFI_GRAPHICS_OUTPUT_PROTOCOL *This, UINT32 ModeNumber);
typedef EFI_STATUS(EFIAPI *EFI_GRAPHICS_OUTPUT_PROTOCOL_BLT)(
	EFI_GRAPHICS_OUTPUT_PROTOCOL *This,
	EFI_GRAPHICS_OUTPUT_BLT_PIXEL *BltBuffer,
	EFI_GRAPHICS_OUTPUT_BLT_OPERATION BltOperation, UINTN SourceX,
	UINTN SourceY, UINTN DestinationX, UINTN DestinationY, UINTN Width,
	UINTN Height, UINTN Delta);

struct EFI_GRAPHICS_OUTPUT_PROTOCOL {
	EFI_GRAPHICS_OUTPUT_PROTOCOL_QUERY_MODE QueryMode;
	EFI_GRAPHICS_OUTPUT_PROTOCOL_SET_MODE SetMode;
	EFI_GRAPHICS_OUTPUT_PROTOCOL_BLT Blt;
	EFI_GRAPHICS_OUTPUT_PROTOCOL_MODE *Mode;
};

#endif
