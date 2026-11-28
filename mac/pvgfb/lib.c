// SPDX-License-Identifier: MIT
//
// The memory functions the compiler emits calls to. There is no C library
// here, so they are written out. Blt moves whole screens through them, so
// they copy eight bytes at a time when both pointers allow it.

#include "pvgfb.h"

#define WORD_SIZE sizeof(UINT64)

static BOOLEAN WordAligned(const VOID *A, const VOID *B)
{
	return (((UINTN)A | (UINTN)B) & (WORD_SIZE - 1)) == 0;
}

static VOID CopyForward(UINT8 *d, const UINT8 *s, UINTN Size)
{
	if (WordAligned(d, s)) {
		for (; Size >= WORD_SIZE; Size -= WORD_SIZE) {
			*(UINT64 *)d = *(const UINT64 *)s;
			d += WORD_SIZE;
			s += WORD_SIZE;
		}
	}
	while (Size--)
		*d++ = *s++;
}

// Copies from the end towards the start; d and s point just past the end.
static VOID CopyBackward(UINT8 *d, const UINT8 *s, UINTN Size)
{
	if (WordAligned(d, s)) {
		for (; Size >= WORD_SIZE; Size -= WORD_SIZE) {
			d -= WORD_SIZE;
			s -= WORD_SIZE;
			*(UINT64 *)d = *(const UINT64 *)s;
		}
	}
	while (Size--)
		*--d = *--s;
}

VOID *memcpy(VOID *Dest, const VOID *Src, UINTN Size)
{
	CopyForward(Dest, Src, Size);
	return Dest;
}

VOID *memmove(VOID *Dest, const VOID *Src, UINTN Size)
{
	UINT8 *d = Dest;
	const UINT8 *s = Src;

	if (d < s)
		CopyForward(d, s, Size);
	else if (d > s)
		CopyBackward(d + Size, s + Size, Size);
	return Dest;
}

VOID *memset(VOID *Dest, int Value, UINTN Size)
{
	UINT8 *d = Dest;
	UINT64 word = (UINT8)Value * 0x0101010101010101ULL;

	if (WordAligned(d, d)) {
		for (; Size >= WORD_SIZE; Size -= WORD_SIZE) {
			*(UINT64 *)d = word;
			d += WORD_SIZE;
		}
	}
	while (Size--)
		*d++ = (UINT8)Value;
	return Dest;
}
