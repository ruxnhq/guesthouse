// SPDX-License-Identifier: MIT
//
// The "nothing" entries of ruxnumd.dll's tables, one function per slot, so
// that the lab log (umd_log.c) can say which entry the runtime called, the
// first time it calls each: a table slot's number is its place among the
// table's function pointers. Each returns zero (S_OK, FALSE or nothing).

#include "umd.h"

#define MOST_SLOTS	256

static volatile LONG called[MOST_SLOTS];

static ULONG_PTR slot_called(ULONG slot)
{
	if (slot < MOST_SLOTS && !InterlockedExchange(&called[slot], 1))
		umd_log("first call of table slot", slot);
	return 0;
}

#define STUB(n) static ULONG_PTR APIENTRY stub_##n(void) { return slot_called(n); }
#define STUB8(n) STUB(n##0) STUB(n##1) STUB(n##2) STUB(n##3) STUB(n##4) STUB(n##5) STUB(n##6) STUB(n##7)
#define STUB32(n) STUB8(n##0) STUB8(n##1) STUB8(n##2) STUB8(n##3)

// Slots 0000 to 0377 in octal, 256 in all.
STUB32(00) STUB32(01) STUB32(02) STUB32(03) STUB32(04) STUB32(05) STUB32(06) STUB32(07)

#define REF(n) (void *)stub_##n,
#define REF8(n) REF(n##0) REF(n##1) REF(n##2) REF(n##3) REF(n##4) REF(n##5) REF(n##6) REF(n##7)
#define REF32(n) REF8(n##0) REF8(n##1) REF8(n##2) REF8(n##3)

static void *const stubs[MOST_SLOTS] = {
	REF32(00) REF32(01) REF32(02) REF32(03) REF32(04) REF32(05) REF32(06) REF32(07)
};

// Fills a table of function pointers with the stubs; first is the slot
// number the table's first entry gets in the log.
void umd_fill_stubs(void *table, SIZE_T bytes, ULONG first)
{
	void **slot = table;
	SIZE_T i;

	for (i = 0; i < bytes / sizeof(void *); i++)
		slot[i] = stubs[(first + i) % MOST_SLOTS];
}

// The DXGI table's own: its entries share no log number with the device
// table's (item B's probe: which DXGI entries the runtime calls that we
// leave as "nothing"). Logged as the entry's place in the DXGI table.
#define DXGI_SLOTS	32

static volatile LONG dxgi_called[DXGI_SLOTS];

static ULONG_PTR dxgi_slot_called(ULONG slot)
{
	if (slot < DXGI_SLOTS && !InterlockedExchange(&dxgi_called[slot], 1))
		umd_log("first call of DXGI table entry (left as nothing)", slot);
	return 0;
}

#define DSTUB(n) static ULONG_PTR APIENTRY dxgi_stub_##n(void) { return dxgi_slot_called(n); }
#define DSTUB8(n) DSTUB(n##0) DSTUB(n##1) DSTUB(n##2) DSTUB(n##3) DSTUB(n##4) DSTUB(n##5) DSTUB(n##6) DSTUB(n##7)
DSTUB8(0) DSTUB8(1) DSTUB8(2) DSTUB8(3)

#define DREF(n) (void *)dxgi_stub_##n,
#define DREF8(n) DREF(n##0) DREF(n##1) DREF(n##2) DREF(n##3) DREF(n##4) DREF(n##5) DREF(n##6) DREF(n##7)

static void *const dxgi_stubs[DXGI_SLOTS] = { DREF8(0) DREF8(1) DREF8(2) DREF8(3) };

void umd_fill_dxgi_stubs(void *table, SIZE_T bytes)
{
	void **slot = table;
	SIZE_T i;

	for (i = 0; i < bytes / sizeof(void *) && i < DXGI_SLOTS; i++)
		slot[i] = dxgi_stubs[i];
}
