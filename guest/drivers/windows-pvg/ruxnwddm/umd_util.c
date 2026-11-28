// SPDX-License-Identifier: MIT
//
// The few helpers ruxnumd.dll needs, since it is built without a C runtime
// (as pvgtest is): memory from the process heap, zeroing and copying.

#include "umd.h"

void *umd_alloc(SIZE_T bytes)
{
	return HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, bytes);
}

void umd_free(void *p)
{
	if (p)
		HeapFree(GetProcessHeap(), 0, p);
}

void umd_zero(void *p, SIZE_T bytes)
{
	volatile UCHAR *b = p;

	while (bytes--)
		*b++ = 0;
}

void umd_copy(void *to, const void *from, SIZE_T bytes)
{
	UCHAR *t = to;
	const UCHAR *f = from;

	while (bytes--)
		*t++ = *f++;
}

// The compiler may turn loops into calls to these, which the C runtime
// would provide.
void *memset(void *p, int c, size_t n)
{
	volatile UCHAR *b = p;

	while (n--)
		*b++ = (UCHAR)c;
	return p;
}

void *memcpy(void *to, const void *from, size_t n)
{
	umd_copy(to, from, n);
	return to;
}

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved)
{
	UNREFERENCED_PARAMETER(instance);
	UNREFERENCED_PARAMETER(reason);
	UNREFERENCED_PARAMETER(reserved);
	return TRUE;
}
