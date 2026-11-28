// SPDX-License-Identifier: GPL-2.0-or-later
/* The object tree and its charges. */
#include "objects.h"

#include <assert.h>
#include <stdio.h>

#define MIB (1ull << 20)

static int gone_calls;
static uint64_t gone_bytes;

static void gone(void *opaque, void *range, uint64_t bytes)
{
    assert(opaque == &gone_calls && range);
    gone_calls++;
    gone_bytes += bytes;
}

static void add(Objects *o, uint32_t client, uint32_t parent, uint32_t object, MemoryKind kind,
                uint64_t bytes, uint32_t file)
{
    ObjectClaim claim = { kind, bytes };
    ObjectInfo info = { .client = client, .parent = parent, .object = object, .file = file };

    assert(objects_claim(o, &claim));
    objects_add(o, &claim, bytes, &info);
}

static void test_objects(void)
{
    ObjectLimits limits = { .max_objects = 8, .max_bytes = { 0, 100 * MIB, 10 * MIB } };
    Objects *o = objects_new(&limits, gone, &gone_calls);
    ObjectClaim big = { MEMORY_VIDEO, 60 * MIB }, huge = { MEMORY_VIDEO, UINT64_MAX };

    add(o, 1, 1, 1, MEMORY_NONE, 0, 7); /* client */
    add(o, 1, 1, 2, MEMORY_NONE, 0, 7); /* device */
    add(o, 1, 2, 3, MEMORY_VIDEO, 40 * MIB, 7);
    assert(objects_has_client(o, 1) && !objects_has_client(o, 2) && !objects_has_client(o, 3));
    assert(objects_bytes(o, MEMORY_VIDEO) == 40 * MIB);

    /* Two claims in flight cannot both fit, and a size that would wrap fits nothing. */
    assert(objects_claim(o, &big));
    assert(!objects_claim(o, &big));
    assert(!objects_claim(o, &huge));
    objects_drop_claim(o, &big);
    assert(objects_bytes(o, MEMORY_VIDEO) == 40 * MIB);

    /* The driver's size wins over the size asked for. */
    {
        ObjectClaim c = { MEMORY_VIDEO, MIB };
        ObjectInfo info = { .client = 1, .parent = 2, .object = 4, .file = 7 };

        assert(objects_claim(o, &c));
        objects_add(o, &c, 2 * MIB, &info);
        assert(objects_bytes(o, MEMORY_VIDEO) == 42 * MIB);
    }

    /* Freeing the device takes the memory under it, with no free for each. */
    objects_remove(o, 1, 2);
    assert(objects_bytes(o, MEMORY_VIDEO) == 0 && objects_count(o) == 1);

    /* Host ranges go with their object, by whichever road. */
    {
        ObjectClaim c = { MEMORY_NONE, 0 };
        ObjectInfo info = { .client = 1, .parent = 1, .object = 5, .file = 7,
                            .host_range = &limits, .host_bytes = 4096 };

        assert(objects_claim(o, &c));
        objects_add(o, &c, 0, &info);
    }
    objects_file_closed(o, 9); /* another file: nothing */
    assert(objects_count(o) == 2 && gone_calls == 0);
    objects_file_closed(o, 7);
    assert(objects_count(o) == 0 && gone_calls == 1 && gone_bytes == 4096);
    assert(!objects_has_client(o, 1));

    /* The count is a hard stop, and space comes back. */
    add(o, 2, 2, 2, MEMORY_NONE, 0, 1);
    for (uint32_t i = 10; i < 17; i++) {
        add(o, 2, 2, i, MEMORY_NONE, 0, 1);
    }
    {
        ObjectClaim c = { MEMORY_NONE, 0 };

        assert(!objects_claim(o, &c));
        objects_remove(o, 2, 10);
        assert(objects_claim(o, &c));
        objects_drop_claim(o, &c);
    }

    /* A client gone takes all, an object of an unknown parent included. */
    add(o, 2, 99, 50, MEMORY_SYSTEM, MIB, 1);
    objects_remove(o, 2, 2);
    assert(objects_count(o) == 0 && objects_bytes(o, MEMORY_SYSTEM) == 0);

    /* Local objects are told apart, per client. */
    {
        ObjectInfo info = { .client = 3, .parent = 3, .object = 6 };

        add(o, 3, 3, 3, MEMORY_NONE, 0, 1);
        assert(objects_add_local(o, &info) == LOCAL_ADDED);
        assert(objects_is_local(o, 3, 6) && !objects_is_local(o, 4, 6) && !objects_is_local(o, 3, 3));
    }

    /* A handle made again replaces the old object and its charge. */
    add(o, 5, 5, 5, MEMORY_NONE, 0, 1);
    add(o, 5, 5, 6, MEMORY_VIDEO, MIB, 1);
    add(o, 5, 5, 6, MEMORY_VIDEO, 2 * MIB, 1);
    assert(objects_bytes(o, MEMORY_VIDEO) == 2 * MIB);
}

/* A local object goes under a client the guest has, and never in the place of anything already there. */
static void test_local_objects(void)
{
    ObjectLimits limits = { .max_objects = 4, .max_bytes = { 0, 100 * MIB, 10 * MIB } };
    Objects *o = objects_new(&limits, gone, &gone_calls);
    ObjectInfo on_client = { .client = 1, .parent = 1, .object = 1 };
    ObjectInfo on_memory = { .client = 1, .parent = 2, .object = 3 };
    ObjectInfo stranger = { .client = 7, .parent = 7, .object = 7 };
    ObjectInfo fresh = { .client = 1, .parent = 2, .object = 4 };

    add(o, 1, 1, 1, MEMORY_NONE, 0, 1);
    add(o, 1, 1, 2, MEMORY_NONE, 0, 1);
    add(o, 1, 2, 3, MEMORY_VIDEO, 40 * MIB, 1);

    /* The client's own handle and a handle in use stay as they are, with their charges. */
    assert(objects_add_local(o, &on_client) == LOCAL_HANDLE_TAKEN);
    assert(objects_add_local(o, &on_memory) == LOCAL_HANDLE_TAKEN);
    assert(objects_bytes(o, MEMORY_VIDEO) == 40 * MIB && objects_count(o) == 3);
    assert(!objects_is_local(o, 1, 1) && !objects_is_local(o, 1, 3));

    /* A client nobody made is not made by naming it. */
    assert(objects_add_local(o, &stranger) == LOCAL_NO_CLIENT);
    assert(!objects_has_client(o, 7) && objects_count(o) == 3);

    /* A new handle under a known client is answered, counted, and the count still stops. */
    assert(objects_add_local(o, &fresh) == LOCAL_ADDED);
    assert(objects_is_local(o, 1, 4) && objects_count(o) == 4);
    fresh.object = 5;
    assert(objects_add_local(o, &fresh) == LOCAL_FULL);

    /* It goes with its client. */
    objects_remove(o, 1, 1);
    assert(objects_count(o) == 0 && objects_bytes(o, MEMORY_VIDEO) == 0);
}

/* Guest pages mapped for the driver stay inside the guest's memory and a count of mappings, and come back with their object. */
static void test_page_budget(void)
{
    ObjectLimits limits = { .max_objects = 8, .max_bytes = { 0, 100 * MIB, 10 * MIB },
                            .max_page_bytes = 100 * MIB, .max_page_runs = 10 };
    Objects *o = objects_new(&limits, gone, &gone_calls);
    int calls = gone_calls;

    add(o, 1, 1, 1, MEMORY_NONE, 0, 1);
    assert(objects_take_pages(o, 6, 40 * MIB));
    assert(!objects_take_pages(o, 5, MIB));       /* past the count of mappings */
    assert(!objects_take_pages(o, 1, 61 * MIB));  /* past the guest's memory */
    assert(!objects_take_pages(o, UINT32_MAX, 0) && !objects_take_pages(o, 0, UINT64_MAX));
    {
        ObjectClaim c = { MEMORY_NONE, 0 };
        ObjectInfo info = { .client = 1, .parent = 1, .object = 2, .file = 1,
                            .host_range = &limits, .host_bytes = 40 * MIB, .host_runs = 6 };

        assert(objects_claim(o, &c));
        objects_add(o, &c, 0, &info);
    }
    /* The same pages again, over and over, are charged each time. */
    assert(objects_take_pages(o, 4, 40 * MIB));
    assert(!objects_take_pages(o, 1, 40 * MIB));
    objects_give_pages(o, 4, 40 * MIB);

    /* Freeing the object gives its room back. */
    objects_remove(o, 1, 2);
    assert(gone_calls == calls + 1);
    assert(objects_take_pages(o, 10, 100 * MIB));
    objects_give_pages(o, 10, 100 * MIB);
}

static void add_pinned(Objects *o, uint32_t client, uint32_t parent, uint32_t object, uint32_t file, uint32_t pin)
{
    ObjectClaim claim = { MEMORY_NONE, 0 };
    ObjectInfo info = { .client = client, .parent = parent, .object = object, .file = file, .pin = pin };

    assert(objects_claim(o, &claim));
    objects_add(o, &claim, 0, &info);
}

/* Pinned guest pages are let go only when nothing on the host reaches them any more. */
static void test_pins(void)
{
    ObjectLimits limits = { .max_objects = 64, .max_bytes = { 0, 100 * MIB, 100 * MIB } };
    Objects *o = objects_new(&limits, gone, &gone_calls);
    uint32_t pin;

    add(o, 1, 1, 1, MEMORY_NONE, 0, 1);
    add(o, 2, 2, 2, MEMORY_NONE, 0, 2);
    assert(objects_pin_held(o, 0) && objects_pin_held(o, 1000)); /* none, and one not made yet */

    /* A duplicate in another client keeps the pages after the allocation goes. */
    pin = objects_new_pin(o);
    add_pinned(o, 1, 1, 10, 1, pin);
    assert(objects_pin_of(o, 1, 10) == pin);
    add_pinned(o, 2, 2, 20, 2, objects_pin_of(o, 1, 10));
    objects_remove(o, 1, 10);
    assert(objects_pin_held(o, pin));
    assert(objects_pins_let_go(o) == 0);
    objects_remove(o, 2, 2); /* the duplicate goes with its client */
    assert(!objects_pin_held(o, pin) && objects_pins_let_go(o) == 1);

    /* A CPU or UVM mapping holds the pages until its file closes. */
    pin = objects_new_pin(o);
    add_pinned(o, 1, 1, 11, 1, pin);
    objects_pin_hold_file(o, pin, 5);
    objects_remove(o, 1, 11);
    assert(objects_pin_held(o, pin));
    objects_file_closed(o, 6);
    assert(objects_pin_held(o, pin));
    objects_file_closed(o, 5);
    assert(!objects_pin_held(o, pin));

    /* An export holds them in its file, and an import from that file is an object of its own. */
    pin = objects_new_pin(o);
    add_pinned(o, 1, 1, 12, 1, pin);
    objects_pin_export(o, pin, 9);
    assert(objects_pin_exported(o, 9) == pin && objects_pin_exported(o, 8) == 0);
    add(o, 3, 3, 3, MEMORY_NONE, 0, 3);
    add_pinned(o, 3, 3, 30, 3, objects_pin_exported(o, 9));
    objects_remove(o, 1, 12);
    objects_file_closed(o, 9);
    assert(objects_pin_exported(o, 9) == 0 && objects_pin_held(o, pin));
    objects_remove(o, 3, 30);
    assert(!objects_pin_held(o, pin));

    /* What the device can't follow keeps them for good, and a pin let go is never taken up again. */
    pin = objects_new_pin(o);
    add_pinned(o, 1, 1, 13, 1, pin);
    objects_pin_stick(o, pin);
    objects_remove(o, 1, 13);
    objects_file_closed(o, 1);
    assert(objects_pin_held(o, pin));
    {
        uint32_t gone_pin = objects_new_pin(o);

        add(o, 4, 4, 4, MEMORY_NONE, 0, 4);
        add_pinned(o, 4, 4, 40, 4, gone_pin);
        objects_remove(o, 4, 40);
        assert(!objects_pin_held(o, gone_pin));
        add_pinned(o, 4, 4, 41, 4, gone_pin);
        assert(objects_pin_of(o, 4, 41) == 0 && !objects_pin_held(o, gone_pin));
    }
}

int main(void)
{
    test_objects();
    test_local_objects();
    test_page_budget();
    test_pins();
    puts("ok");
    return 0;
}
