/*
 * rmframes, the reading side. See rmframes.h for the protocol.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 */
#ifndef RMFRAMES_SUB_H
#define RMFRAMES_SUB_H

#include <stddef.h>
#include <stdint.h>
#include <dispatch/dispatch.h>
#include <IOSurface/IOSurfaceRef.h>

#include "rmframes.h"

typedef struct rmf_sub rmf_sub;

/* All called on the queue given to rmf_sub_open, one at a time. */
typedef struct rmf_sub_callbacks {
    void *ctx;
    /* New slots. The surfaces are valid during the call: retain what you keep.
     * Every slot of the old generation counts as given back. */
    void (*on_slots)(void *ctx, uint32_t generation, uint32_t width,
                     uint32_t height, uint32_t count,
                     IOSurfaceRef const *surfaces);
    /* A frame in one of the slots. It stays the reader's until
     * rmf_sub_release. */
    void (*on_frame)(void *ctx, const rmf_frame_msg *frame);
    /* The publisher went away. */
    void (*on_gone)(void *ctx);
    /* The publisher has as many readers as it serves, and turned this one
     * away, with a reason to show. Nothing more comes; close the reader and
     * try again later. May be NULL: the reason is then printed, and the
     * reader just gets no frames. */
    void (*on_refused)(void *ctx, const char *reason);
} rmf_sub_callbacks;

/* Looks up `service` and says hello. NULL if no publisher has that name (yet),
 * with the reason in err. */
rmf_sub *rmf_sub_open(const char *service, dispatch_queue_t queue,
                      rmf_sub_callbacks callbacks, char *err, size_t err_len);

/* Gives a slot back. Any thread. */
void rmf_sub_release(rmf_sub *sub, uint32_t generation, uint32_t slot);

/* Says bye and stops. Callbacks already queued may still run; release
 * nothing through `sub` after this. */
void rmf_sub_close(rmf_sub *sub);

#endif
