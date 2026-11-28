// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// Writes what the driver saw under its service key's Parameters
// (HKLM\SYSTEM\CurrentControlSet\Services\ruxnwddm\Parameters), so it can
// be read with reg.exe and no debugger, as ruxnpvg's record.c does. Best
// effort: a value that cannot be written never fails the device. Call at
// PASSIVE_LEVEL only.

#include "ruxnwddm.h"

#define POOL_TAG	'rwxR'

static UNICODE_STRING parameters;	// the absolute path of the key
static const WCHAR parameters_suffix[] = L"\\Parameters";

NTSTATUS ruxnwddm_record_init(PUNICODE_STRING registry_path)
{
	USHORT bytes = registry_path->Length + sizeof(parameters_suffix);

	parameters.Buffer = ExAllocatePool2(POOL_FLAG_PAGED, bytes, POOL_TAG);
	if (!parameters.Buffer)
		return STATUS_NO_MEMORY;
	parameters.Length = 0;
	parameters.MaximumLength = bytes;
	RtlCopyUnicodeString(&parameters, registry_path);
	RtlAppendUnicodeToString(&parameters, parameters_suffix);
	RtlCreateRegistryKey(RTL_REGISTRY_ABSOLUTE, parameters.Buffer);
	return STATUS_SUCCESS;
}

void ruxnwddm_record_free(void)
{
	if (parameters.Buffer)
		ExFreePoolWithTag(parameters.Buffer, POOL_TAG);
	RtlZeroMemory(&parameters, sizeof(parameters));
}

void ruxnwddm_record_ulong(PCWSTR name, ULONG value)
{
	if (parameters.Buffer)
		RtlWriteRegistryValue(RTL_REGISTRY_ABSOLUTE, parameters.Buffer, name, REG_DWORD,
				      &value, sizeof(value));
}

void ruxnwddm_record_ulong64(PCWSTR name, ULONG64 value)
{
	if (parameters.Buffer)
		RtlWriteRegistryValue(RTL_REGISTRY_ABSOLUTE, parameters.Buffer, name, REG_QWORD,
				      &value, sizeof(value));
}

// Reads a number under Parameters; 0 if it is missing.
ULONG ruxnwddm_record_read(PCWSTR name)
{
	RTL_QUERY_REGISTRY_TABLE query[2];
	ULONG value = 0;
	ULONG none = 0;

	if (!parameters.Buffer)
		return 0;
	RtlZeroMemory(query, sizeof(query));
	query[0].Flags = RTL_QUERY_REGISTRY_DIRECT | RTL_QUERY_REGISTRY_TYPECHECK;
	query[0].Name = (PWSTR)name;
	query[0].EntryContext = &value;
	query[0].DefaultType = (REG_DWORD << RTL_QUERY_REGISTRY_TYPECHECK_SHIFT) | REG_DWORD;
	query[0].DefaultData = &none;
	query[0].DefaultLength = sizeof(none);
	if (!NT_SUCCESS(RtlQueryRegistryValues(RTL_REGISTRY_ABSOLUTE, parameters.Buffer, query,
					       NULL, NULL)))
		return 0;
	return value;
}

// Adds one to a counter kept across starts and boots and returns it. A
// missing value counts as zero.
ULONG ruxnwddm_record_count(PCWSTR name)
{
	ULONG count = ruxnwddm_record_read(name) + 1;

	ruxnwddm_record_ulong(name, count);
	return count;
}

// Records a DDI's failure under its name, and passes the status on. The
// graphics kernel's own log gives only the last status of a failed start;
// this says which call gave it. "Not supported" is an answer, not a fault.
NTSTATUS ruxnwddm_note(PCWSTR name, NTSTATUS status)
{
	if (!NT_SUCCESS(status) && status != STATUS_NOT_SUPPORTED) {
		ruxnwddm_record_ulong(name, (ULONG)status);
		ruxnwddm_record_count(L"Failures");
	}
	return status;
}
