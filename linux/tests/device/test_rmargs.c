// SPDX-License-Identifier: GPL-2.0-or-later
/* The rules for pointer and file fields. */
#include "rmargs.h"

#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define RM_CMD(nr, size) (0xc0000000u | (uint32_t)(size) << 16 | 'F' << 8 | (nr))
#define CTRL_FB_GET_INFO 0x20801301u /* one embedded list: pointer at 8, count at 0, 8 bytes each */
#define CTRL_EXPORT_FD 0x3d05u       /* fd at 16 of its parameters */
#define CTRL_NO_PUBLIC_SOURCE 0x20808163u /* forwarded at exactly 4 bytes and no other size */
#define CTRL_NO_PUBLIC_SOURCE_BYTES 4
#define CLASS_CONTEXT_SHARE 0x9067 /* parameters of 12 bytes in branch 615, 16 in 620 */
#define FUZZ_ROUNDS 3000000

static uint8_t data[4096];
static RmField fields[RMARGS_MAX_FIELDS];
static int matched[RMARGS_MAX_FIELDS];

static void put32(uint32_t at, uint32_t v)
{
    memcpy(data + at, &v, sizeof(v));
}

static int walk(uint32_t cmd, uint32_t len, const struct nvfwd_fix *fix, uint32_t nfix)
{
    RmCall call;

    if (!rmargs_parse(&call, cmd, data, len, fix, nfix)) {
        return -EBADMSG;
    }
    return rmargs_fields(&call, fields, matched);
}

static void test_parse(void)
{
    RmCall c;

    memset(data, 0, sizeof(data));
    assert(rmargs_parse(&c, RM_CMD(ESC_RM_CONTROL, 32), data, 32, NULL, 0));
    assert(c.nr == ESC_RM_CONTROL && c.arg_at == 0 && c.size == 32);

    /* Another ioctl type, and an ioctl longer than its call. */
    assert(!rmargs_parse(&c, 0xc020462a ^ 0x0100, data, 32, NULL, 0));
    assert(!rmargs_parse(&c, 0xc0204b2a, data, 32, NULL, 0));
    assert(!rmargs_parse(&c, RM_CMD(ESC_RM_CONTROL, 64), data, 32, NULL, 0)); /* longer than the call */

    put32(RM_XFER_CMD, ESC_RM_ALLOC);
    put32(RM_XFER_SIZE, 48);
    assert(rmargs_parse(&c, RM_CMD(ESC_XFER, 16), data, 64, NULL, 0));
    assert(c.nr == ESC_RM_ALLOC && c.arg_at == 16 && c.size == 48);
    assert(!rmargs_parse(&c, RM_CMD(ESC_XFER, 16), data, 63, NULL, 0));
    assert(!rmargs_parse(&c, RM_CMD(ESC_XFER, 8), data, 64, NULL, 0));
    put32(RM_XFER_CMD, 0x100 | ESC_RM_ALLOC); /* the driver reads the whole word */
    assert(!rmargs_parse(&c, RM_CMD(ESC_XFER, 16), data, 64, NULL, 0));
}

static int control_of_size_in(uint32_t cmd, uint32_t size, uint32_t len)
{
    struct nvfwd_fix fix = { NVFWD_FIX_PTR, RM_CONTROL_PARAMS, 64, 0 };

    memset(data, 0, sizeof(data));
    put32(RM_CONTROL_CMD, cmd);
    put32(RM_CONTROL_PARAMS_SIZE, size);
    return walk(RM_CMD(ESC_RM_CONTROL, 32), len, &fix, 1);
}

static int control_of_size(uint32_t cmd, uint32_t size)
{
    return control_of_size_in(cmd, size, 192);
}

/* Nobody can read what this control does, so its size is the one thing known about it. */
static void test_a_pinned_control_goes_at_its_size_only(void)
{
    static const uint32_t wrong[] = { 0, 1, 3, 5, 8, 64, 100, UINT32_MAX };

    assert(control_of_size(CTRL_NO_PUBLIC_SOURCE, CTRL_NO_PUBLIC_SOURCE_BYTES) == 1);
    for (size_t i = 0; i < sizeof(wrong) / sizeof(wrong[0]); i++) {
        assert(control_of_size(CTRL_NO_PUBLIC_SOURCE, wrong[i]) == -EINVAL);
    }
    /* Others aren't held to it. */
    assert(control_of_size(0x20800110u, 100) == 1);
}

/* What the CUDA runtime asks before its first call returns. None of it can carry a pointer. */
static const struct { uint32_t cmd, size; } cuda_runtime_controls[] = {
    { 0x20802209u, 4 },   /* NV2080_CTRL_CMD_RC_GET_WATCHDOG_INFO */
    { 0x20809001u, 8 },   /* no public name */
    { 0x2080a026u, 532 }, /* no public name */
};

static void test_the_cuda_runtimes_controls_go_at_their_sizes_only(void)
{
    static const uint32_t wrong[] = { 0, 1, 3, 5, 7, 9, 16, 64, 531, 533, 1024, UINT32_MAX };

    for (size_t i = 0; i < sizeof(cuda_runtime_controls) / sizeof(cuda_runtime_controls[0]); i++) {
        uint32_t cmd = cuda_runtime_controls[i].cmd, size = cuda_runtime_controls[i].size;

        assert(control_of_size_in(cmd, size, 64 + size) == 1);
        for (size_t j = 0; j < sizeof(wrong) / sizeof(wrong[0]); j++) {
            if (wrong[j] != size) {
                assert(control_of_size_in(cmd, wrong[j], sizeof(data)) == -EINVAL);
            }
        }
    }
}

/* The filter's question, asked before any field is read: the device answers a wrong size with the driver's status. */
static void test_a_wrong_size_is_said_before_anything_is_read(void)
{
    static const uint32_t wrong[] = { 0, 1, 3, 4, 5, 7, 8, 9, 16, 64, 531, 533, 536, 1024, UINT32_MAX };

    for (size_t i = 0; i < sizeof(cuda_runtime_controls) / sizeof(cuda_runtime_controls[0]); i++) {
        uint32_t cmd = cuda_runtime_controls[i].cmd, size = cuda_runtime_controls[i].size;

        assert(rmargs_control_size_ok(cmd, size));
        for (size_t j = 0; j < sizeof(wrong) / sizeof(wrong[0]); j++) {
            assert(rmargs_control_size_ok(cmd, wrong[j]) == (wrong[j] == size));
        }
    }
    assert(rmargs_control_size_ok(CTRL_NO_PUBLIC_SOURCE, CTRL_NO_PUBLIC_SOURCE_BYTES));
    assert(!rmargs_control_size_ok(CTRL_NO_PUBLIC_SOURCE, 8));
    assert(rmargs_control_size_ok(0x20800110u, 100)); /* the rest aren't held to a size */
}

/* The host only ever reads the device's own bytes for the two controls nobody can read. */
static void test_an_owned_control_carries_only_the_devices_bytes(void)
{
    static const uint32_t question[][2] = { { 0, 0x400 }, { 16, 2 }, { 20, 1 }, { 36, 0x10 } };
    uint8_t params[532], before[532];

    /* NVIDIA's library leaves the rest of the block as it found it: here, the guest's leftovers. */
    memset(params, 0xab, sizeof(params));
    for (size_t i = 0; i < 4; i++) {
        memcpy(params + question[i][0], &question[i][1], 4);
    }
    assert(rmargs_own_request(0x2080a026u, params, sizeof(params)) == 1);
    for (uint32_t at = 0; at < sizeof(params); at++) {
        uint8_t want = 0;
        for (size_t i = 0; i < 4; i++) {
            if (at >= question[i][0] && at < question[i][0] + 4) {
                want = ((const uint8_t *)&question[i][1])[at - question[i][0]];
            }
        }
        assert(params[at] == want);
    }

    /* Any other question goes nowhere, and the block is left alone. */
    for (size_t i = 0; i < 4; i++) {
        uint32_t other = question[i][1] + 1;

        memcpy(before, params, sizeof(params));
        memcpy(before + question[i][0], &other, 4);
        memcpy(params, before, sizeof(params));
        assert(rmargs_own_request(0x2080a026u, params, sizeof(params)) == -1);
        assert(!memcmp(params, before, sizeof(params)));
        memcpy(params + question[i][0], &question[i][1], 4);
    }
    assert(rmargs_own_request(0x2080a026u, params, 531) == -1);
    assert(rmargs_own_request(0x2080a026u, NULL, sizeof(params)) == -1);

    memset(params, 0xab, 8);
    assert(rmargs_own_request(0x20809001u, params, 8) == 1);
    for (uint32_t at = 0; at < 8; at++) {
        assert(params[at] == 0);
    }
    assert(rmargs_own_request(0x20809001u, params, 4) == -1);

    /* What libcuda 620.73.04 really sent, leftovers and all, recorded on the host. */
    static const uint8_t recorded[] = {
        0x00, 0x04, 0x00, 0x00, 0x00, 0x61, 0x2f, 0x30, 0x00, 0x00, 0x00, 0x00, 0x30, 0x30, 0x30, 0x3a, 0x02, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x34, 0x3a,
        0x30, 0x30, 0x2e, 0x30, 0x2f, 0x76, 0x72, 0x61, 0x6d, 0x20, 0x34, 0x33, 0x30, 0x38, 0x39, 0x39, 0x32, 0x0a,
    };
    memset(params, 0, sizeof(params));
    memcpy(params, recorded, sizeof(recorded));
    assert(rmargs_own_request(0x2080a026u, params, sizeof(params)) == 1);
    for (uint32_t at = 52; at < sizeof(params); at++) {
        assert(params[at] == 0);
    }
    memset(params, 0, sizeof(params));
    memcpy(params, recorded, 52); /* the same call without leftovers past the question */
    memset(params + 4, 0, 12);
    assert(rmargs_own_request(0x2080a026u, params, sizeof(params)) == 1);

    /* Bytes outside the named fields are the guest's to set: the call is answered, and none of them goes on. */
    for (uint32_t at = 0; at < sizeof(params); at++) {
        int named = at < 4 || (at >= 16 && at < 24) || (at >= 36 && at < 40);

        if (named) {
            continue;
        }
        memset(params, 0, sizeof(params));
        for (size_t i = 0; i < 4; i++) {
            memcpy(params + question[i][0], &question[i][1], 4);
        }
        params[at] = 0x5a;
        assert(rmargs_own_request(0x2080a026u, params, sizeof(params)) == 1);
        assert(params[at] == 0);
    }
    for (uint32_t at = 0; at < 8; at++) {
        memset(params, 0, 8);
        params[at] = 0x80;
        assert(rmargs_own_request(0x20809001u, params, 8) == 1 && params[at] == 0);
    }

    /* A control with a public layout keeps the guest's parameters. */
    memset(params, 0xab, sizeof(params));
    assert(rmargs_own_request(0x20802209u, params, 4) == 0);
    assert(rmargs_own_request(0x20800110u, params, 100) == 0);
    assert(params[0] == 0xab && params[99] == 0xab);
}

static void test_xfer_points_at_what_was_read(void)
{
    struct nvfwd_fix good = { NVFWD_FIX_PTR, RM_XFER_POINTER, RM_XFER_BYTES, 0 };
    struct nvfwd_fix elsewhere = { NVFWD_FIX_PTR, RM_XFER_POINTER, 1024, 0 };

    memset(data, 0, sizeof(data));
    put32(RM_XFER_CMD, ESC_RM_FREE);
    put32(RM_XFER_SIZE, 16);
    assert(walk(RM_CMD(ESC_XFER, 16), 2048, &good, 1) == 1);
    assert(walk(RM_CMD(ESC_XFER, 16), 2048, &elsewhere, 1) == -EINVAL);
    assert(walk(RM_CMD(ESC_XFER, 16), 2048, NULL, 0) == -EINVAL);
}

static void test_control(void)
{
    struct nvfwd_fix fix[3] = {
        { NVFWD_FIX_PTR, RM_CONTROL_PARAMS, 64, 0 },
        { NVFWD_FIX_PTR, 64 + 8, 128, 0 },
    };

    memset(data, 0, sizeof(data));
    put32(RM_CONTROL_CMD, CTRL_FB_GET_INFO);
    put32(RM_CONTROL_PARAMS_SIZE, 16);
    put32(64, 4); /* four list entries */

    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 192, fix, 2) == 2);
    assert(matched[0] == 0 && matched[1] == 1);

    /* A field with no fix is still listed: it is cleared, never passed on. */
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 192, fix, 1) == 2);
    assert(matched[1] == -1 && fields[1].kind == NVFWD_FIX_PTR && fields[1].at == 72);
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 192, NULL, 0) == 1 && matched[0] == -1);

    /* The list must fit behind its pointer, and the count times the size must not wrap. */
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 160, fix, 2) == 2);
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 159, fix, 2) == -EINVAL);
    put32(64, 0x20000001); /* times 8 this is 8 in 32 bits */
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 192, fix, 2) == -EINVAL);
    put32(64, 4);

    /* Parameters too short to hold the list's own fields. */
    put32(RM_CONTROL_PARAMS_SIZE, 8);
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 192, fix, 2) == -EINVAL);
    put32(RM_CONTROL_PARAMS_SIZE, 16);

    /* A size past the end of the call. */
    put32(RM_CONTROL_PARAMS_SIZE, 4096);
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 192, fix, 2) == -EINVAL);
    put32(RM_CONTROL_PARAMS_SIZE, 16);

    /* A fix anywhere but a field, and two on one field. */
    fix[1].at = 40;
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 192, fix, 2) == -EINVAL);
    fix[1].at = 72;
    fix[2] = fix[1];
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 192, fix, 3) == -EINVAL);

    /* The wrong kind of fix on a field. */
    fix[1].kind = NVFWD_FIX_FD;
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 192, fix, 2) == -EINVAL);
    fix[1].kind = NVFWD_FIX_PAGES;
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 192, fix, 2) == -EINVAL);

    /* An fd in the parameters. */
    memset(data, 0, sizeof(data));
    put32(RM_CONTROL_CMD, CTRL_EXPORT_FD);
    put32(RM_CONTROL_PARAMS_SIZE, 32);
    fix[0] = (struct nvfwd_fix){ NVFWD_FIX_PTR, RM_CONTROL_PARAMS, 64, 0 };
    fix[1] = (struct nvfwd_fix){ NVFWD_FIX_FD, 64 + 16, 3, 0 };
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 96, fix, 2) == 2 && fields[1].kind == NVFWD_FIX_FD);
    assert(walk(RM_CMD(ESC_RM_CONTROL, 32), 96, fix, 1) == 2 && matched[1] == -1);
}

static void test_alloc_and_memory(void)
{
    struct nvfwd_fix params = { NVFWD_FIX_PTR, RM_ALLOC_PARAMS, 64, 0 };
    struct nvfwd_fix pages = { NVFWD_FIX_PAGES, RM_MEMORY_POINTER, 64, 0 };
    struct nvfwd_fix event[2] = { params, { NVFWD_FIX_FD, 64 + RM_EVENT_DATA, 3, 0 } };

    memset(data, 0, sizeof(data));
    put32(RM_ALLOC_CLASS, 0x80);
    put32(RM_ALLOC_LONG_PARAMS_SIZE, 0); /* size from the class table: 56 for a device */
    assert(walk(RM_CMD(ESC_RM_ALLOC, 48), 120, &params, 1) == 2);
    assert(walk(RM_CMD(ESC_RM_ALLOC, 48), 119, &params, 1) == -EINVAL);
    assert(walk(RM_CMD(ESC_RM_ALLOC, 40), 120, &params, 1) == -EINVAL);

    memset(data, 0, sizeof(data));
    put32(RM_ALLOC_CLASS, CLASS_EVENT);
    put32(RM_ALLOC_LONG_PARAMS_SIZE, 24);
    put32(64 + RM_EVENT_KIND, CLASS_OS_EVENT);
    assert(walk(RM_CMD(ESC_RM_ALLOC, 48), 128, event, 2) == 3);
    put32(64 + RM_EVENT_KIND, 0x78); /* any kind but an OS event */
    assert(walk(RM_CMD(ESC_RM_ALLOC, 48), 128, event, 1) == -EPERM);

    memset(data, 0, sizeof(data));
    put32(RM_MEMORY_CLASS, CLASS_OS_DESCRIPTOR);
    assert(walk(RM_CMD(ESC_RM_ALLOC_MEMORY, 56), 128, &pages, 1) == 2);
    assert(walk(RM_CMD(ESC_RM_ALLOC_MEMORY, 56), 128, NULL, 0) == 2 && matched[0] == -1);
    put32(RM_MEMORY_CLASS, CLASS_MEMORY_SYSTEM); /* guest pages are for OS descriptors only */
    assert(walk(RM_CMD(ESC_RM_ALLOC_MEMORY, 56), 128, &pages, 1) == -EINVAL);

    memset(data, 0, sizeof(data));
    put32(8, 27); /* pins an address, and guest memory has its own way in */
    assert(walk(RM_CMD(ESC_RM_VID_HEAP_CONTROL, 184), 184, NULL, 0) == -EPERM);
    put32(8, 2);
    put32(12, 27); /* a handle, which may be anything */
    assert(walk(RM_CMD(ESC_RM_VID_HEAP_CONTROL, 184), 184, NULL, 0) == 0);
    assert(walk(RM_CMD(ESC_RM_VID_HEAP_CONTROL, 176), 184, NULL, 0) == -EINVAL);

    /* No fields, so no fix has anywhere to land. */
    assert(walk(RM_CMD(ESC_RM_FREE, 16), 128, &params, 1) == -EINVAL);
}

static void test_a_branch_picks_its_class_sizes(void)
{
    struct nvfwd_fix params = { NVFWD_FIX_PTR, RM_ALLOC_PARAMS, 64, 0 };

    memset(data, 0, sizeof(data));
    put32(RM_ALLOC_CLASS, CLASS_CONTEXT_SHARE);
    put32(RM_ALLOC_LONG_PARAMS_SIZE, 0);
    assert(!rmargs_use_driver("610.1.2") && !rmargs_use_driver("6150.1") && !rmargs_use_driver(""));
    assert(rmargs_use_driver("615.78.08"));
    assert(walk(RM_CMD(ESC_RM_ALLOC, 48), 76, &params, 1) == 2);
    assert(walk(RM_CMD(ESC_RM_ALLOC, 48), 80, &params, 1) == 2);
    assert(rmargs_use_driver("620.70.06"));
    assert(walk(RM_CMD(ESC_RM_ALLOC, 48), 76, &params, 1) == -EINVAL);
    assert(walk(RM_CMD(ESC_RM_ALLOC, 48), 80, &params, 1) == 2);
    assert(rmargs_use_driver("615.78.08"));
}

static void test_uvm(void)
{
    struct nvfwd_fix fd = { NVFWD_FIX_FD, 24, 3, 0 };

    assert(rmargs_uvm_fields(37, 40, &fd, 1, fields, matched) == 1); /* UVM_REGISTER_GPU */
    assert(rmargs_uvm_fields(37, 40, NULL, 0, fields, matched) == 1 && matched[0] == -1);
    assert(rmargs_uvm_fields(37, 39, &fd, 1, fields, matched) == -EINVAL);
    assert(rmargs_uvm_fields(39, 8, &fd, 1, fields, matched) == -EINVAL);
    assert(rmargs_uvm_fields(0xdead, 8, NULL, 0, fields, matched) == -EPERM);
}

/* Anything the walk accepts stays inside the call, whatever the call holds. */
static void fuzz(void)
{
    static const uint32_t escapes[] = {
        ESC_RM_CONTROL, ESC_RM_ALLOC, ESC_RM_ALLOC_MEMORY, ESC_RM_MAP_MEMORY, ESC_REGISTER_FD,
        ESC_ALLOC_OS_EVENT, ESC_RM_IDLE_CHANNELS, ESC_RM_VID_HEAP_CONTROL, ESC_XFER, ESC_RM_FREE,
    };
    static const uint32_t controls[] = { CTRL_FB_GET_INFO, CTRL_EXPORT_FD, 0x101, 0x80170d, 0x3d06 };
    uint64_t accepted = 0;

    for (int round = 0; round < FUZZ_ROUNDS; round++) {
        uint32_t len = random() % 512, nfix = random() % 6;
        uint8_t *call = malloc(len ? len : 1); /* exact, so ASan sees any read past it */
        struct nvfwd_fix fix[6];
        uint32_t nr = escapes[random() % (sizeof(escapes) / sizeof(escapes[0]))];
        uint32_t size = random() % 3 ? (random() % 4 ? 32 : 48) : random() % 256;
        RmCall c;
        int n;

        for (uint32_t i = 0; i < len; i++) {
            call[i] = random() % 4 ? 0 : random();
        }
        if (len >= 32) {
            uint32_t ctl = controls[random() % 5], small = random() % 64;

            memcpy(call + RM_CONTROL_CMD, &ctl, 4);
            memcpy(call + RM_CONTROL_PARAMS_SIZE, &small, 4);
        }
        for (uint32_t i = 0; i < nfix; i++) {
            fix[i] = (struct nvfwd_fix){ random() % 4, random() % 4 ? random() % 96 : random(),
                                         random() % 4 ? random() % 512 : random(), 0 };
        }
        if (rmargs_parse(&c, RM_CMD(nr, size), call, len, fix, nfix)) {
            assert((uint64_t)c.arg_at + c.size <= len);
            n = rmargs_fields(&c, fields, matched);
            for (int i = 0; i < n; i++) {
                assert((uint64_t)fields[i].at + (fields[i].kind == NVFWD_FIX_FD ? 4 : 8) <= len);
                if (matched[i] >= 0 && fields[i].kind == NVFWD_FIX_PTR) {
                    assert((uint64_t)fix[matched[i]].target + fields[i].size <= len);
                }
                accepted++;
            }
        }
        free(call);
    }
    assert(accepted > 1000);
    printf("fuzz: %d calls, %llu fields accepted, all inside their call\n", FUZZ_ROUNDS,
           (unsigned long long)accepted);
}

int main(void)
{
    test_parse();
    test_a_pinned_control_goes_at_its_size_only();
    test_the_cuda_runtimes_controls_go_at_their_sizes_only();
    test_an_owned_control_carries_only_the_devices_bytes();
    test_a_wrong_size_is_said_before_anything_is_read();
    test_xfer_points_at_what_was_read();
    test_control();
    test_alloc_and_memory();
    test_a_branch_picks_its_class_sizes();
    test_uvm();
    fuzz();
    puts("ok");
    return 0;
}
