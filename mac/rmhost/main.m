/*
 * rmhost: a guest's display, from QEMU's apple-gfx device, through the Mac's
 * hardware HEVC encoder, with nothing in the guest.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * Usage:
 *   rmhost record OUT.mov SECONDS [-n NAME] [-b BITS] [-8] [-w SECONDS]
 *   rmhost stats SECONDS [-n NAME] [-b BITS] [-8] [-w SECONDS]
 *   rmhost serve -q QMP_SOCKET [options]   (see serve.m)
 *
 *   -n  the publisher's bootstrap name: frame-export= on the device (rmhost.frames)
 *   -b  bits a second (25000000)
 *   -8  HEVC Main, 8 bits; the default is Main10, as remoteman sends
 *   -w  how long to wait for the publisher to appear (10)
 */
#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#include <getopt.h>
#include <signal.h>
#include <stdatomic.h>

#include "rmhost.h"

#define DEFAULT_SERVICE "rmhost.frames"
#define DEFAULT_BITRATE 25000000
#define FRAME_RATE 60

typedef struct recorder {
    dispatch_queue_t queue;
    AVAssetWriter *writer;
    AVAssetWriterInput *input;
    CMVideoDimensions size;
    uint64_t written, not_written, keyframes;
} recorder;

/* The encoder's thread: hand the sample to the writer's queue, in order. */
static void record_output(void *ctx, const rme_output *out)
{
    recorder *r = ctx;
    CMSampleBufferRef sample = (CMSampleBufferRef)CFRetain(out->sample);
    bool key = out->keyframe;

    dispatch_async(r->queue, ^{
        CMFormatDescriptionRef fmt = CMSampleBufferGetFormatDescription(sample);
        CMVideoDimensions dims = CMVideoFormatDescriptionGetDimensions(fmt);

        if (!r->input) {
            if (!key) {
                r->not_written++;  /* a file starts with a keyframe */
            } else {
                r->input = [AVAssetWriterInput assetWriterInputWithMediaType:AVMediaTypeVideo
                                                              outputSettings:nil
                                                            sourceFormatHint:fmt];
                r->input.expectsMediaDataInRealTime = YES;
                /* Frame times are kept to 1/60000 s, not the default 1/600. */
                r->input.mediaTimeScale = 60000;
                [r->writer addInput:r->input];
                [r->writer startWriting];
                [r->writer startSessionAtSourceTime:CMSampleBufferGetPresentationTimeStamp(sample)];
                r->size = dims;
            }
        }
        if (r->input) {
            if (dims.width != r->size.width || dims.height != r->size.height ||
                !r->input.readyForMoreMediaData || ![r->input appendSampleBuffer:sample]) {
                /* One size to a file: a new size is left out. */
                r->not_written++;
            } else {
                r->written++;
                r->keyframes += key;
            }
        }
        CFRelease(sample);
    });
}

static bool parse_common(int argc, char **argv, const char **service, rme_config *cfg,
                         double *wait)
{
    int opt;

    optind = 1;
    while ((opt = getopt(argc, argv, "n:b:8w:")) != -1) {
        switch (opt) {
        case 'n': *service = optarg; break;
        case 'b': cfg->bitrate = atoi(optarg); break;
        case '8': cfg->ten_bit = false; break;
        case 'w': *wait = atof(optarg); break;
        default: return false;
        }
    }
    return optind == argc && cfg->bitrate > 0;
}

/* record and stats: the same, with or without a file. */
static int run_for(const char *out_path, double seconds, int argc, char **argv)
{
    const char *service = DEFAULT_SERVICE;
    rme_config cfg = { .bitrate = DEFAULT_BITRATE, .fps = FRAME_RATE, .ten_bit = true };
    double wait = 10;
    recorder rec = { 0 };
    rmh_hooks hooks = { 0 };
    NSError *error = nil;

    if (seconds <= 0 || !parse_common(argc, argv, &service, &cfg, &wait)) {
        return 2;
    }
    if (out_path) {
        NSURL *url = [NSURL fileURLWithPath:@(out_path)];
        [[NSFileManager defaultManager] removeItemAtURL:url error:nil];
        rec.writer = [AVAssetWriter assetWriterWithURL:url fileType:AVFileTypeQuickTimeMovie
                                                 error:&error];
        if (!rec.writer) {
            fprintf(stderr, "rmhost: %s: %s\n", out_path, error.description.UTF8String);
            return 1;
        }
        rec.queue = dispatch_queue_create("rmhost.writer", NULL);
        hooks.ctx = &rec;
        hooks.on_output = record_output;
    }

    rmh *h = rmh_open(service, wait, NULL, cfg, hooks);
    if (!h) {
        return 1;
    }
    double cpu0 = rmh_cpu_seconds();
    NSDate *t0 = [NSDate date];
    [NSThread sleepForTimeInterval:seconds];
    rmh_close(h);
    double wall = -[t0 timeIntervalSinceNow];
    double cpu = rmh_cpu_seconds() - cpu0;

    if (out_path) {
        dispatch_sync(rec.queue, ^{});
        if (rec.input) {
            dispatch_semaphore_t done = dispatch_semaphore_create(0);
            [rec.input markAsFinished];
            [rec.writer finishWritingWithCompletionHandler:^{
                dispatch_semaphore_signal(done);
            }];
            dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
        }
        if (rec.writer.status != AVAssetWriterStatusCompleted) {
            fprintf(stderr, "rmhost: %s not written: %s\n", out_path,
                    rec.writer.error.description.UTF8String ?: "no frames");
        } else {
            printf("rmhost: %s: %llu frames written (%llu keyframes), %llu left out\n",
                   out_path, rec.written, rec.keyframes, rec.not_written);
        }
    }
    rmh_print_stats(h, wall, cpu);
    rmh_free(h);
    return 0;
}

static void usage(void)
{
    fprintf(stderr,
            "usage: rmhost record OUT.mov SECONDS [-n NAME] [-b BITS] [-8] [-w SECONDS]\n"
            "       rmhost stats SECONDS [-n NAME] [-b BITS] [-8] [-w SECONDS]\n"
#ifdef RMHOST_SERVE
            "       rmhost serve -q QMP_SOCKET [-n NAME] [-l ADDR:PORT] [-s STATE_DIR]\n"
            "                    [-N HOST_NAME] [-p] [-b BITS] [-8]\n"
#else
            "       (serve: not built in; build.sh builds it with remoteman's core)\n"
#endif
    );
}

int main(int argc, char **argv)
{
    int r = 2;

    signal(SIGPIPE, SIG_IGN);
    setvbuf(stdout, NULL, _IOLBF, 0);
    @autoreleasepool {
        if (argc >= 4 && !strcmp(argv[1], "record")) {
            r = run_for(argv[2], atof(argv[3]), argc - 3, argv + 3);
        } else if (argc >= 3 && !strcmp(argv[1], "stats")) {
            r = run_for(NULL, atof(argv[2]), argc - 2, argv + 2);
#ifdef RMHOST_SERVE
        } else if (argc >= 2 && !strcmp(argv[1], "serve")) {
            r = rmhost_serve(argc - 1, argv + 1);
#endif
        }
    }
    if (r == 2) {
        usage();
    }
    return r;
}
