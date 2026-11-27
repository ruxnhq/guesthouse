/*
 * rmhost serve: the guest's picture to a remoteman viewer, and the viewer's
 * input back into the guest, through remoteman's core (its C interface,
 * include/remoteman_core.h, linked from a build of the commit README.md
 * names).
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * The core is GPL-3.0-or-later, so an rmhost built with serve in it is a GPL-3
 * program as a whole; this file stays MIT.
 *
 * Usage: rmhost serve [-q QMP_SOCKET] [-n NAME] [-l ADDR:PORT] [-s STATE_DIR]
 *                     [-N HOST_NAME] [-p] [-b BITS] [-8]
 *   -q  QEMU's QMP socket for input; with none, input is dropped
 *   -n  the publisher's bootstrap name (rmhost.frames)
 *   -l  where to listen (0.0.0.0:7791)
 *   -s  where the core keeps its key and trusted devices
 *       (~/Library/Application Support/rmhost)
 *   -N  the name viewers are shown (this Mac's name, "guest")
 *   -p  let the next unknown device in and remember it, with no time limit
 *
 * While it runs, standard input takes "pair open SECONDS" and "pair close" (pairctl.m says
 * what it prints), so pairing can be opened for a while and closed with viewers connected.
 */
#import <Foundation/Foundation.h>
#include <getopt.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <signal.h>

#include "remoteman_core.h"
#include "pairctl.h"
#include "qmp_input.h"
#include "rmhost.h"

#define DEFAULT_SERVICE "rmhost.frames"
#define DEFAULT_LISTEN "0.0.0.0:7791"
#define FRAME_RATE 60
/* How long serve waits before asking again when the publisher has no room for it. */
#define REFUSED_RETRY_SECONDS 2
/* remoteman's Mac host: 25 Mbit/s below 4K, 40 at 4K and above. */
#define BITRATE_BELOW_4K 25000000
#define BITRATE_4K 40000000

typedef struct server {
    rm_host *host;
    rmh *source;               /* under lock: callbacks race with reconnects */
    qmp_input *qmp;
    bool ten_bit;
    int own_bitrate;           /* -b, or 0 to follow the picture size */
    dispatch_semaphore_t wake; /* the publisher went away, or a signal came */
    volatile int stop;

    pthread_mutex_t lock;
    uint32_t width, height;
    bool client;
    rm_accept accept;
    mach_timebase_info_data_t timebase;
} server;

static int bitrate_for(server *s, uint32_t width, uint32_t height)
{
    if (s->own_bitrate > 0) {
        return s->own_bitrate;
    }
    return width * height >= 3840u * 2160u ? BITRATE_4K : BITRATE_BELOW_4K;
}

static void fill_accept(server *s, rm_accept *a)
{
    memset(a, 0, sizeof(*a));
    a->codec = 0;  /* HEVC, Annex-B, parameter sets on every keyframe */
    a->depth = s->ten_bit ? 10 : 8;
    a->chroma = 420;
    a->width = s->width;
    a->height = s->height;
    a->refresh = FRAME_RATE;
    a->hdr = 0;
}

/* Runs fn with the encoder, if the publisher is connected. */
static void with_encoder(server *s, void (^fn)(rme *enc))
{
    pthread_mutex_lock(&s->lock);
    if (s->source) {
        fn(rmh_encoder(s->source));
    }
    pthread_mutex_unlock(&s->lock);
}

/* ---- the core's callbacks, on its handler thread ---- */

static int32_t on_client(void *ctx, const rm_client_info *c, rm_accept *accept)
{
    server *s = ctx;
    int32_t ok = 0;

    printf("rmhost: viewer \"%s\" %ux%u at %u Hz, decoders %s\n", c->name ? c->name : "?",
           c->width, c->height, c->refresh, c->decoders ? c->decoders : "?");
    pthread_mutex_lock(&s->lock);
    if (!c->decoders || !strstr(c->decoders, "hevc")) {
        snprintf(accept->refuse, sizeof(accept->refuse), "this host sends HEVC only");
    } else if (s->width == 0) {
        snprintf(accept->refuse, sizeof(accept->refuse),
                 "no picture from the guest yet: try again once it has shown one");
    } else {
        fill_accept(s, accept);
        s->accept = *accept;
        s->client = true;
        ok = 1;
    }
    pthread_mutex_unlock(&s->lock);
    if (ok) {
        with_encoder(s, ^(rme *enc) {
            rme_set_active(enc, true);
            rme_keyframe(enc);
        });
    }
    if (!ok) {
        printf("rmhost: refused: %s\n", accept->refuse);
    }
    return ok;
}

static void on_keyframe(void *ctx)
{
    with_encoder(ctx, ^(rme *enc) {
        rme_keyframe(enc);
    });
}

static void on_input(void *ctx, const rm_input *in)
{
    server *s = ctx;
    if (s->qmp) {
        qmp_input_event(s->qmp, in->kind, in->button, in->down, in->phase, in->usage,
                        in->x, in->y);
    }
}

static void on_client_left(void *ctx)
{
    server *s = ctx;

    pthread_mutex_lock(&s->lock);
    s->client = false;
    pthread_mutex_unlock(&s->lock);
    with_encoder(s, ^(rme *enc) {
        rme_set_active(enc, false);
    });
    if (s->qmp) {
        qmp_input_release_all(s->qmp);
    }
    printf("rmhost: viewer left\n");
}

static void on_bitrate(void *ctx, uint32_t bits)
{
    with_encoder(ctx, ^(rme *enc) {
        rme_set_bitrate(enc, (int)bits);
    });
}

/* ---- frames ---- */

static void on_size(void *ctx, rme *enc, uint32_t width, uint32_t height)
{
    server *s = ctx;
    bool restart;
    rm_accept a;

    pthread_mutex_lock(&s->lock);
    s->width = width;
    s->height = height;
    restart = s->client;
    if (restart) {
        fill_accept(s, &a);
        s->accept = a;
    }
    pthread_mutex_unlock(&s->lock);
    if (s->qmp) {
        qmp_input_set_size(s->qmp, width, height);
    }
    rme_set_bitrate(enc, bitrate_for(s, width, height));
    if (restart && s->host) {
        /* The viewer is told; the next frame is a keyframe (a new size makes a
         * new encoder session, which starts with one). */
        rm_host_restart(s->host, &a);
    }
}

/* Annex-B as the core takes it: start codes, and VPS, SPS and PPS in front of
 * each keyframe. This copies the compressed frame (tens of kilobytes), never
 * the picture. */
static uint8_t *annex_b(const rme_output *out, size_t *len_out)
{
    CMFormatDescriptionRef fmt = CMSampleBufferGetFormatDescription(out->sample);
    CMBlockBufferRef block = CMSampleBufferGetDataBuffer(out->sample);
    size_t len = CMBlockBufferGetDataLength(block);
    size_t params = 0, count = 0, used = 0;
    int nal_len = 4;
    uint8_t *raw, *buf;

    CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fmt, 0, NULL, NULL, &count, &nal_len);
    if (out->keyframe) {
        for (size_t i = 0; i < count; i++) {
            size_t size = 0;
            CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fmt, i, NULL, &size, NULL, NULL);
            params += 4 + size;
        }
    }
    raw = malloc(len);
    CMBlockBufferCopyDataBytes(block, 0, len, raw);
    /* Lengths become start codes of 4 bytes; with shorter lengths the output
     * can be a little longer than the input. */
    buf = malloc(params + len + len / 2 + 16);
    if (out->keyframe) {
        for (size_t i = 0; i < count; i++) {
            const uint8_t *p = NULL;
            size_t size = 0;
            CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fmt, i, &p, &size, NULL, NULL);
            memcpy(buf + used, "\0\0\0\1", 4);
            memcpy(buf + used + 4, p, size);
            used += 4 + size;
        }
    }
    for (size_t at = 0; at + (size_t)nal_len <= len;) {
        size_t size = 0;
        for (int i = 0; i < nal_len; i++) {
            size = size << 8 | raw[at + i];
        }
        at += nal_len;
        if (size == 0 || at + size > len) {
            break;
        }
        memcpy(buf + used, "\0\0\0\1", 4);
        memcpy(buf + used + 4, raw + at, size);
        used += 4 + size;
        at += size;
    }
    free(raw);
    *len_out = used;
    return buf;
}

static void on_output(void *ctx, const rme_output *out)
{
    server *s = ctx;
    bool client;
    size_t len;
    uint8_t *data;
    uint64_t now_us, age_us, capture_us;

    pthread_mutex_lock(&s->lock);
    client = s->client;
    pthread_mutex_unlock(&s->lock);
    if (!client || !s->host) {
        return;
    }
    now_us = rm_host_now_us(s->host);
    if (now_us == 0) {
        return;
    }
    /* The frame's age on this Mac's clock, taken off the session clock. */
    age_us = (mach_absolute_time() - out->host_time) * s->timebase.numer /
             s->timebase.denom / 1000;
    capture_us = now_us > age_us ? now_us - age_us : 0;
    data = annex_b(out, &len);
    if (!rm_host_send_frame(s->host, data, len, out->keyframe, capture_us) && out->keyframe) {
        /* A keyframe the core didn't take has to be made again. */
        with_encoder(s, ^(rme *enc) {
            rme_keyframe(enc);
        });
    }
    free(data);
}

static void on_gone(void *ctx)
{
    server *s = ctx;
    dispatch_semaphore_signal(s->wake);
}

/* Kept here: under ARC a source nobody holds is freed, and stops. */
static dispatch_source_t signal_sources[2];

static void watch_signal(server *s, int index, int sig)
{
    dispatch_source_t src = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, sig, 0,
                                                   dispatch_get_global_queue(0, 0));
    signal_sources[index] = src;
    signal(sig, SIG_IGN);
    dispatch_source_set_event_handler(src, ^{
        s->stop = 1;
        dispatch_semaphore_signal(s->wake);
    });
    dispatch_resume(src);
}

int rmhost_serve(int argc, char **argv)
{
    server s = { .ten_bit = true };
    const char *service = DEFAULT_SERVICE, *listen = DEFAULT_LISTEN, *qmp_path = NULL;
    const char *name = NULL;
    NSString *state = [NSHomeDirectory() stringByAppendingPathComponent:
                                            @"Library/Application Support/rmhost"];
    int pair = 0, opt;
    char err[512], info[256];

    while ((opt = getopt(argc, argv, "q:n:l:s:N:pb:8")) != -1) {
        switch (opt) {
        case 'q': qmp_path = optarg; break;
        case 'n': service = optarg; break;
        case 'l': listen = optarg; break;
        case 's': state = @(optarg); break;
        case 'N': name = optarg; break;
        case 'p': pair = 1; break;
        case 'b': s.own_bitrate = atoi(optarg); break;
        case '8': s.ten_bit = false; break;
        default: return 2;
        }
    }
    if (optind != argc) {
        return 2;
    }
    if (!name) {
        name = [[NSString stringWithFormat:@"%@ guest",
                 [[NSHost currentHost] localizedName] ?: @"Mac"] UTF8String];
    }
    mach_timebase_info(&s.timebase);
    pthread_mutex_init(&s.lock, NULL);
    s.wake = dispatch_semaphore_create(0);

    if (qmp_path) {
        s.qmp = qmp_input_open(qmp_path, err, sizeof(err));
        if (!s.qmp) {
            fprintf(stderr, "rmhost: %s\n", err);
            return 1;
        }
        printf("rmhost: input goes to %s\n", qmp_path);
    } else {
        printf("rmhost: no QMP socket given: input is dropped\n");
    }
    watch_signal(&s, 0, SIGINT);
    watch_signal(&s, 1, SIGTERM);

    [[NSFileManager defaultManager] createDirectoryAtPath:state withIntermediateDirectories:YES
                                               attributes:nil error:nil];
    rm_host_callbacks cb = {
        .ctx = &s,
        .on_client = on_client,
        .on_keyframe = on_keyframe,
        .on_input = on_input,
        .on_client_left = on_client_left,
    };
    s.host = rm_host_start(listen, state.UTF8String, name, "windows", "video,input", pair, cb,
                           err, sizeof(err));
    if (!s.host) {
        fprintf(stderr, "rmhost: remoteman's core didn't start: %s\n", err);
        return 1;
    }
    rm_host_info(s.host, info, sizeof(info));
    printf("rmhost: remoteman host \"%s\" listening: %s (address, fingerprint), pairing %s\n",
           name, info, rm_host_pairing(s.host) ? "on" : "off");
    rm_host_set_bitrate_callback(s.host, BITRATE_4K, on_bitrate, &s);
    pairctl_start(s.host, pair);

    rme_config cfg = {
        .bitrate = s.own_bitrate > 0 ? s.own_bitrate : BITRATE_BELOW_4K,
        .fps = FRAME_RATE,
        .ten_bit = s.ten_bit,
        .hold_last = true,
    };
    rmh_hooks hooks = { .ctx = &s, .on_size = on_size, .on_output = on_output,
                        .on_gone = on_gone };
    /* Frames wait for a viewer: until one comes they are given straight back. */
    while (!s.stop) {
        rmh *h = rmh_open(service, 1e9, &s.stop, cfg, hooks);
        if (!h) {
            break;
        }
        pthread_mutex_lock(&s.lock);
        rme_set_active(rmh_encoder(h), s.client);
        s.source = h;
        pthread_mutex_unlock(&s.lock);
        dispatch_semaphore_wait(s.wake, DISPATCH_TIME_FOREVER);
        pthread_mutex_lock(&s.lock);
        s.source = NULL;
        pthread_mutex_unlock(&s.lock);
        rmh_close(h);
        bool refused = rmh_refused(h);
        rmh_free(h);
        if (refused && !s.stop) {
            /* Not at once: a place comes free only when another reader goes. */
            sleep(REFUSED_RETRY_SECONDS);
        }
        if (!s.stop) {
            printf("rmhost: waiting for the publisher to come back\n");
        }
    }
    printf("rmhost: stopping\n");
    rm_host_stop(s.host);
    if (s.qmp) {
        qmp_input_close(s.qmp);
    }
    return 0;
}
