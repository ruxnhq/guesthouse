/*
 * rmhost: the publisher's frames into the encoder.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 */
#import <Foundation/Foundation.h>
#include <stdatomic.h>
#include <sys/resource.h>

#include "rmframes_sub.h"
#include "rmhost.h"

struct rmh {
    dispatch_queue_t queue;
    rmf_sub *sub;
    rme *enc;
    rmh_hooks hooks;
    _Atomic bool gone;
    _Atomic bool refused;
    uint32_t width, height;
};

static void h_on_slots(void *ctx, uint32_t generation, uint32_t width, uint32_t height,
                       uint32_t count, IOSurfaceRef const *surfaces)
{
    rmh *h = ctx;

    if (!rme_set_slots(h->enc, generation, width, height, count, surfaces)) {
        fprintf(stderr, "rmhost: can't encode the slots of generation %u\n", generation);
    }
    if (width != h->width || height != h->height) {
        h->width = width;
        h->height = height;
        printf("rmhost: picture %ux%u, %u slots\n", width, height, count);
        fflush(stdout);
        if (h->hooks.on_size) {
            h->hooks.on_size(h->hooks.ctx, h->enc, width, height);
        }
    }
}

static void h_on_frame(void *ctx, const rmf_frame_msg *frame)
{
    rmh *h = ctx;
    rme_frame(h->enc, frame);
}

static void h_on_gone(void *ctx)
{
    rmh *h = ctx;

    atomic_store(&h->gone, true);
    printf("rmhost: the publisher went away\n");
    fflush(stdout);
    if (h->hooks.on_gone) {
        h->hooks.on_gone(h->hooks.ctx);
    }
}

/* The publisher has as many readers as it serves. Nothing more comes, as if
 * it had gone; the caller may try again later. */
static void h_on_refused(void *ctx, const char *reason)
{
    rmh *h = ctx;

    atomic_store(&h->refused, true);
    atomic_store(&h->gone, true);
    printf("rmhost: refused: %s\n", reason);
    fflush(stdout);
    if (h->hooks.on_gone) {
        h->hooks.on_gone(h->hooks.ctx);
    }
}

static void h_output(void *ctx, const rme_output *out)
{
    rmh *h = ctx;
    if (h->hooks.on_output) {
        h->hooks.on_output(h->hooks.ctx, out);
    }
}

static void h_release(void *ctx, uint32_t generation, uint32_t slot)
{
    rmh *h = ctx;
    if (h->sub && !atomic_load(&h->gone)) {
        rmf_sub_release(h->sub, generation, slot);
    }
}

rmh *rmh_open(const char *service, double wait_seconds, volatile int *stop,
              rme_config config, rmh_hooks hooks)
{
    rmh *h = calloc(1, sizeof(*h));
    rmf_sub_callbacks cb = {
        .ctx = h, .on_slots = h_on_slots, .on_frame = h_on_frame, .on_gone = h_on_gone,
        .on_refused = h_on_refused,
    };
    rme_callbacks ecb = { .ctx = h, .on_output = h_output, .release = h_release };
    NSDate *until = [NSDate dateWithTimeIntervalSinceNow:wait_seconds];
    char err[256];
    bool said = false;

    h->hooks = hooks;
    h->queue = dispatch_queue_create("rmhost.frames", NULL);
    h->enc = rme_create(config, ecb, h->queue);
    /* The slots come straight after hello, and their handler needs h->sub
     * set: hold the queue until rmf_sub_open has returned. */
    dispatch_suspend(h->queue);
    for (;;) {
        h->sub = rmf_sub_open(service, h->queue, cb, err, sizeof(err));
        if (h->sub || [until timeIntervalSinceNow] <= 0 || (stop && *stop)) {
            break;
        }
        if (!said) {
            printf("rmhost: waiting for %s\n", service);
            fflush(stdout);
            said = true;
        }
        [NSThread sleepForTimeInterval:0.25];
    }
    dispatch_resume(h->queue);
    if (!h->sub) {
        fprintf(stderr, "rmhost: %s\n", err);
        rmh_free(h);
        return NULL;
    }
    printf("rmhost: connected to %s\n", service);
    fflush(stdout);
    return h;
}

bool rmh_refused(rmh *h)
{
    return atomic_load(&h->refused);
}

rme *rmh_encoder(rmh *h)
{
    return h->enc;
}

void rmh_close(rmh *h)
{
    rme_flush(h->enc);
    dispatch_sync(h->queue, ^{});
    rme_flush(h->enc);
    rmf_sub_close(h->sub);
    h->sub = NULL;
    dispatch_sync(h->queue, ^{});
}

void rmh_free(rmh *h)
{
    rme_destroy(h->enc);
    h->queue = nil;
    free(h);
}

double rmh_cpu_seconds(void)
{
    struct rusage ru;

    getrusage(RUSAGE_SELF, &ru);
    return ru.ru_utime.tv_sec + ru.ru_utime.tv_usec / 1e6 +
           ru.ru_stime.tv_sec + ru.ru_stime.tv_usec / 1e6;
}

void rmh_print_stats(rmh *h, double wall_seconds, double cpu_seconds)
{
    rme_stats s;

    rme_get_stats(h->enc, &s);
    printf("rmhost: %ux%u, %.1f s: %llu frames received, %llu encoded (%.1f a second), "
           "%llu skipped while the encoder was busy, %llu failed\n",
           s.width, s.height, wall_seconds, s.received, s.encoded,
           wall_seconds > 0 ? s.encoded / wall_seconds : 0, s.skipped, s.failed);
    printf("rmhost: encode ms per frame: min %.2f, median %.2f, max %.2f; hardware encoder: %s\n",
           s.ms_min, s.ms_median, s.ms_max, s.hardware ? "yes" : "no");
    printf("rmhost: own CPU %.2f s in %.1f s, %.1f%% of one core\n", cpu_seconds,
           wall_seconds, wall_seconds > 0 ? 100.0 * cpu_seconds / wall_seconds : 0);
    fflush(stdout);
}
