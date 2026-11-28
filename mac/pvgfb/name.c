// SPDX-License-Identifier: MIT
//
// The driver's names for the UEFI shell's "drivers" and "dh" listings.
// English only.

#include "pvgfb.h"

static CHAR8 gLanguages[] = "en";

static CHAR16 gDriverName[] = u"PV graphics framebuffer driver";
static CHAR16 gControllerName[] = u"PV graphics display";

// Accepts "en" and any "en-..." variant, as RFC 4646 tags are written.
static BOOLEAN IsEnglish(const CHAR8 *Language)
{
	return Language != NULL && Language[0] == 'e' && Language[1] == 'n' &&
		(Language[2] == '\0' || Language[2] == '-');
}

static EFI_STATUS EFIAPI GetDriverName(EFI_COMPONENT_NAME2_PROTOCOL *This,
	CHAR8 *Language, CHAR16 **DriverName)
{
	(void)This;
	if (DriverName == NULL || Language == NULL)
		return EFI_INVALID_PARAMETER;
	if (!IsEnglish(Language))
		return EFI_UNSUPPORTED;
	*DriverName = gDriverName;
	return EFI_SUCCESS;
}

static EFI_STATUS EFIAPI GetControllerName(EFI_COMPONENT_NAME2_PROTOCOL *This,
	EFI_HANDLE Controller, EFI_HANDLE Child, CHAR8 *Language,
	CHAR16 **ControllerName)
{
	(void)This;
	if (Controller == NULL || ControllerName == NULL || Language == NULL)
		return EFI_INVALID_PARAMETER;
	if (!IsEnglish(Language))
		return EFI_UNSUPPORTED;
	if (PvgfbFindDevice(Controller, Child) == NULL)
		return EFI_UNSUPPORTED;
	*ControllerName = gControllerName;
	return EFI_SUCCESS;
}

EFI_COMPONENT_NAME2_PROTOCOL gPvgfbComponentName2 = {
	.GetDriverName = GetDriverName,
	.GetControllerName = GetControllerName,
	.SupportedLanguages = gLanguages,
};
