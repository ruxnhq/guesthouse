/*
 * rmhost: frames from the publisher, through the hardware encoder.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 */
#ifndef RMHOST_H
#define RMHOST_H

#include <stdbool.h>
#include <stdint.h>
#include <sys/resource.h>

#include "encoder.h"

typedef struct rmh rmh;

typedef struct rmh_hooks {
    void *ctx;
    /* The picture has a (new) size; called on rmhost's queue. */
    void (*on_size)(void *ctx, rme *enc, uint32_t width, uint32_t height);
    /* An encoded frame, on the encoder's thread. */
    void (*on_output)(void *ctx, const rme_output *out);
    /* The publisher went away. */
    void (*on_gone)(void *ctx);
} rmh_hooks;

/* Connects to the publisher named `service`, trying again for up to
 * `wait_seconds` or until *stop is set (stop may be NULL), and starts feeding
 * its frames to an encoder. NULL if none was found. */
rmh *rmh_open(const char *service, double wait_seconds, volatile int *stop,
              rme_config config, rmh_hooks hooks);

rme *rmh_encoder(rmh *h);

/* Whether the publisher turned this reader away for want of room. */
bool rmh_refused(rmh *h);

/* Lets every frame inside the encoder out, then gives everything back and
 * says bye to the publisher. */
void rmh_close(rmh *h);

/* Frees what rmh_open made, after rmh_close. */
void rmh_free(rmh *h);

/* CPU time this process has used so far, in seconds. */
double rmh_cpu_seconds(void);

/* Prints frames received and encoded, encode times and CPU use. */
void rmh_print_stats(rmh *h, double wall_seconds, double cpu_seconds);

#ifdef RMHOST_SERVE
int rmhost_serve(int argc, char **argv);
#endif

#endif
