/*
 * The hardware HEVC encoder, fed straight from the publisher's IOSurfaces.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 */
#ifndef RMHOST_ENCODER_H
#define RMHOST_ENCODER_H

#include <stdbool.h>
#include <stdint.h>
#include <CoreMedia/CoreMedia.h>
#include <IOSurface/IOSurfaceRef.h>

#include "rmframes.h"

typedef struct rme rme;

typedef struct rme_output {
    CMSampleBufferRef sample;  /* valid during the call; retain to keep */
    bool keyframe;
    double encode_ms;          /* from handing the frame in to getting it out */
    uint64_t host_time;        /* the frame's mach_absolute_time() */
    uint64_t sequence;         /* the publisher's, 0 for a repeat */
} rme_output;

typedef struct rme_config {
    int bitrate;      /* bits a second */
    int fps;
    bool ten_bit;     /* Main10, as remoteman sends; else Main */
    /* Keep the last frame encoded until a newer one is, so that a keyframe can
     * be made of it when the picture is still (rmhost serve). */
    bool hold_last;
} rme_config;

typedef struct rme_callbacks {
    void *ctx;
    /* Each frame out of the encoder, in order, on the encoder's thread. */
    void (*on_output)(void *ctx, const rme_output *out);
    /* Done with a slot: give it back to the publisher. Any thread. */
    void (*release)(void *ctx, uint32_t generation, uint32_t slot);
} rme_callbacks;

typedef struct rme_stats {
    uint64_t received;   /* frames offered */
    uint64_t encoded;    /* frames out of the encoder */
    uint64_t skipped;    /* replaced by a newer one while the encoder was busy */
    uint64_t failed;
    double ms_min, ms_median, ms_max;
    bool hardware;       /* the session said it is on the hardware encoder */
    uint32_t width, height;
} rme_stats;

/* `queue` is where rme_set_slots and rme_frame are called from; frames that
 * waited for the encoder go in from there too. */
rme *rme_create(rme_config config, rme_callbacks callbacks,
                dispatch_queue_t queue);

/* New slots (rmframes on_slots). A new size makes a new session. Returns
 * false if no session could be made. */
bool rme_set_slots(rme *e, uint32_t generation, uint32_t width,
                   uint32_t height, uint32_t count,
                   IOSurfaceRef const *surfaces);

/* A frame (rmframes on_frame). It is encoded, waits for the encoder, or is
 * given back at once when `active` is off. */
void rme_frame(rme *e, const rmf_frame_msg *frame);

/* Encode frames (on) or give them back unencoded (off). On by default. */
void rme_set_active(rme *e, bool active);

/* Make the next frame a keyframe; with hold_last and nothing newer coming,
 * the last frame is encoded again for it. Any thread. */
void rme_keyframe(rme *e);

void rme_set_bitrate(rme *e, int bitrate);

/* Waits for every frame inside the encoder to come out, and gives back any
 * slot still held. */
void rme_flush(rme *e);

void rme_get_stats(rme *e, rme_stats *out);

void rme_destroy(rme *e);

#endif
