/*
 * The hardware HEVC encoder, fed straight from the publisher's IOSurfaces.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * Each slot's IOSurface is wrapped once in a CVPixelBuffer
 * (CVPixelBufferCreateWithIOSurface makes no copy), and that buffer is what
 * goes into VideoToolbox. The encoder reads the surface itself; the slot is
 * given back when the frame comes out.
 *
 * The settings follow remoteman's Mac host (apple/Host/HostEncoder.swift at
 * the commit named in README.md): hardware only, no reordering, no periodic
 * keyframes but one every 2000 frames, speed over quality, BT.709. As there,
 * at most two frames are inside the encoder; a third waits, and a newer one
 * replaces it.
 */
#import <Foundation/Foundation.h>
#import <VideoToolbox/VideoToolbox.h>
#import <CoreVideo/CoreVideo.h>
#include <mach/mach_time.h>
#include <pthread.h>

#include "encoder.h"

#define MAX_IN_FLIGHT 2
/* remoteman's reason: FFmpeg's decoder loses its way where the encoder's
 * picture count wraps at 2048 frames after a keyframe. */
#define WHOLE_FRAME_EVERY 2000

typedef struct frame_ref {
    bool valid;
    uint32_t generation, slot;
    uint64_t sequence, host_time;
} frame_ref;

typedef struct in_flight_frame {
    rme *e;
    frame_ref frame;
    bool repeat;
    bool forced;
    uint64_t started;
} in_flight_frame;

struct rme {
    rme_config config;
    rme_callbacks cb;
    dispatch_queue_t queue;
    pthread_mutex_t lock;

    VTCompressionSessionRef session;
    uint32_t width, height, generation, count;
    CVPixelBufferRef buffers[RMF_MAX_SLOTS];

    int in_flight;
    frame_ref waiting;
    frame_ref held;
    bool force_key, active, hardware;
    int since_key;
    uint64_t last_pts;

    uint64_t received, encoded, skipped, failed;
    double *ms;
    size_t ms_count, ms_room;
};

static mach_timebase_info_data_t timebase;

static double host_ms(uint64_t ticks)
{
    return (double)ticks * timebase.numer / timebase.denom / 1e6;
}

static void give_back(rme *e, frame_ref *f)
{
    if (f->valid) {
        e->cb.release(e->cb.ctx, f->generation, f->slot);
        f->valid = false;
    }
}

static void record_ms(rme *e, double ms)
{
    if (e->ms_count == e->ms_room) {
        e->ms_room = e->ms_room ? e->ms_room * 2 : 1024;
        e->ms = realloc(e->ms, e->ms_room * sizeof(double));
    }
    e->ms[e->ms_count++] = ms;
}

static void encode_waiting(void *opaque);

static void output_callback(void *session_ref, void *frame_refcon,
                            OSStatus status, VTEncodeInfoFlags flags,
                            CMSampleBufferRef sample)
{
    in_flight_frame *f = frame_refcon;
    rme *e = f->e;
    rme_output out = { 0 };
    bool ok = status == noErr && sample && !(flags & kVTEncodeInfo_FrameDropped);
    double ms = host_ms(mach_absolute_time() - f->started);

    (void)session_ref;
    pthread_mutex_lock(&e->lock);
    e->in_flight--;
    if (ok) {
        e->encoded++;
        record_ms(e, ms);
    } else {
        e->failed++;
        if (f->forced) {
            e->force_key = true;
        }
    }
    if (e->config.hold_last && ok && f->frame.generation == e->generation) {
        if (!(e->held.valid && e->held.slot == f->frame.slot &&
              e->held.generation == f->frame.generation)) {
            give_back(e, &e->held);
            e->held = f->frame;
        }
    } else if (!f->repeat) {
        give_back(e, &f->frame);
    }
    if (e->waiting.valid) {
        dispatch_async_f(e->queue, e, encode_waiting);
    }
    pthread_mutex_unlock(&e->lock);

    if (ok) {
        CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sample, false);
        bool not_sync = false;
        if (attachments && CFArrayGetCount(attachments) > 0) {
            CFDictionaryRef a = CFArrayGetValueAtIndex(attachments, 0);
            CFBooleanRef v = CFDictionaryGetValue(a, kCMSampleAttachmentKey_NotSync);
            not_sync = v && CFBooleanGetValue(v);
        }
        out.sample = sample;
        out.keyframe = !not_sync;
        out.encode_ms = ms;
        out.host_time = f->frame.host_time;
        out.sequence = f->repeat ? 0 : f->frame.sequence;
        e->cb.on_output(e->cb.ctx, &out);
    }
    free(f);
}

/* With the lock held; drops it while the frame goes in. */
static void encode_locked(rme *e, frame_ref frame, bool repeat)
{
    in_flight_frame *f = calloc(1, sizeof(*f));
    VTCompressionSessionRef session = e->session;
    CVPixelBufferRef buffer = e->buffers[frame.slot];
    CFDictionaryRef props = NULL;
    uint64_t when = repeat ? mach_absolute_time() : frame.host_time;
    OSStatus st;

    /* Presentation times must rise, also for a repeat of an old frame. */
    if (when <= e->last_pts) {
        when = e->last_pts + 1;
    }
    e->last_pts = when;
    e->since_key++;
    f->e = e;
    f->frame = frame;
    f->repeat = repeat;
    f->forced = e->force_key || e->since_key >= WHOLE_FRAME_EVERY;
    if (f->forced) {
        const void *k = kVTEncodeFrameOptionKey_ForceKeyFrame;
        const void *v = kCFBooleanTrue;
        props = CFDictionaryCreate(NULL, &k, &v, 1, &kCFTypeDictionaryKeyCallBacks,
                                   &kCFTypeDictionaryValueCallBacks);
        e->since_key = 0;
    }
    e->force_key = false;
    e->in_flight++;
    CFRetain(session);
    CVPixelBufferRetain(buffer);
    f->started = mach_absolute_time();
    pthread_mutex_unlock(&e->lock);

    st = VTCompressionSessionEncodeFrame(session, buffer,
                                         CMClockMakeHostTimeFromSystemUnits(when),
                                         kCMTimeInvalid, props, f, NULL);
    CVPixelBufferRelease(buffer);
    CFRelease(session);
    if (props) {
        CFRelease(props);
    }
    pthread_mutex_lock(&e->lock);
    if (st != noErr) {
        /* The callback will not come for this one. */
        e->in_flight--;
        e->failed++;
        if (f->forced) {
            e->force_key = true;
        }
        if (!repeat) {
            give_back(e, &f->frame);
        }
        free(f);
    }
}

static void encode_waiting(void *opaque)
{
    rme *e = opaque;

    pthread_mutex_lock(&e->lock);
    if (e->waiting.valid && e->in_flight < MAX_IN_FLIGHT && e->session) {
        frame_ref f = e->waiting;
        e->waiting.valid = false;
        encode_locked(e, f, false);
    }
    pthread_mutex_unlock(&e->lock);
}

static void repeat_held(void *opaque)
{
    rme *e = opaque;

    pthread_mutex_lock(&e->lock);
    if (e->held.valid && e->held.generation == e->generation && e->session &&
        e->in_flight == 0 && !e->waiting.valid && e->active) {
        encode_locked(e, e->held, true);
    }
    pthread_mutex_unlock(&e->lock);
}

static void set_property(VTCompressionSessionRef s, CFStringRef key, CFTypeRef value)
{
    OSStatus st = VTSessionSetProperty(s, key, value);
    if (st != noErr) {
        fprintf(stderr, "rmhost: encoder refused %s (%d)\n",
                CFStringGetCStringPtr(key, kCFStringEncodingUTF8), (int)st);
    }
}

static void set_rate(VTCompressionSessionRef s, int bitrate)
{
    /* As remoteman: a ceiling of 1.5 times the average over one second. */
    set_property(s, kVTCompressionPropertyKey_AverageBitRate, (__bridge CFTypeRef)@(bitrate));
    set_property(s, kVTCompressionPropertyKey_DataRateLimits,
                 (__bridge CFTypeRef)@[@(bitrate * 3 / 16), @1]);
}

static VTCompressionSessionRef make_session(rme *e, uint32_t width, uint32_t height)
{
    NSDictionary *spec = @{
        (id)kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: @YES,
    };
    VTCompressionSessionRef s = NULL;
    CFBooleanRef hw = NULL;
    OSStatus st = VTCompressionSessionCreate(NULL, (int32_t)width, (int32_t)height,
                                             kCMVideoCodecType_HEVC,
                                             (__bridge CFDictionaryRef)spec, NULL, NULL,
                                             output_callback, e, &s);
    if (st != noErr) {
        fprintf(stderr, "rmhost: no hardware HEVC session for %ux%u (%d)\n",
                width, height, (int)st);
        return NULL;
    }
    set_property(s, kVTCompressionPropertyKey_RealTime, kCFBooleanFalse);
    set_property(s, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
    set_property(s, kVTCompressionPropertyKey_ExpectedFrameRate,
                 (__bridge CFTypeRef)@(e->config.fps));
    set_rate(s, e->config.bitrate);
    set_property(s, kVTCompressionPropertyKey_MaxKeyFrameInterval,
                 (__bridge CFTypeRef)@(INT32_MAX));
    set_property(s, kVTCompressionPropertyKey_ProfileLevel,
                 e->config.ten_bit ? kVTProfileLevel_HEVC_Main10_AutoLevel
                                   : kVTProfileLevel_HEVC_Main_AutoLevel);
    set_property(s, kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality,
                 kCFBooleanTrue);
    set_property(s, kVTCompressionPropertyKey_ColorPrimaries,
                 kCVImageBufferColorPrimaries_ITU_R_709_2);
    set_property(s, kVTCompressionPropertyKey_TransferFunction,
                 kCVImageBufferTransferFunction_ITU_R_709_2);
    set_property(s, kVTCompressionPropertyKey_YCbCrMatrix,
                 kCVImageBufferYCbCrMatrix_ITU_R_709_2);
    VTCompressionSessionPrepareToEncodeFrames(s);

    if (VTSessionCopyProperty(s, kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
                              NULL, &hw) == noErr && hw) {
        e->hardware = CFBooleanGetValue(hw);
        CFRelease(hw);
    } else {
        e->hardware = false;
    }
    return s;
}

rme *rme_create(rme_config config, rme_callbacks callbacks, dispatch_queue_t queue)
{
    rme *e = calloc(1, sizeof(*e));

    if (timebase.denom == 0) {
        mach_timebase_info(&timebase);
    }
    e->config = config;
    e->cb = callbacks;
    e->queue = queue;
    e->active = true;
    e->force_key = true;
    pthread_mutex_init(&e->lock, NULL);
    return e;
}

bool rme_set_slots(rme *e, uint32_t generation, uint32_t width, uint32_t height,
                   uint32_t count, IOSurfaceRef const *surfaces)
{
    VTCompressionSessionRef old = NULL;

    pthread_mutex_lock(&e->lock);
    give_back(e, &e->waiting);
    give_back(e, &e->held);
    if (!e->session || width != e->width || height != e->height) {
        old = e->session;
        e->session = NULL;
    }
    pthread_mutex_unlock(&e->lock);

    if (old) {
        /* Frames still inside give their slots back as they come out. */
        VTCompressionSessionCompleteFrames(old, kCMTimeInvalid);
        VTCompressionSessionInvalidate(old);
        CFRelease(old);
    }

    pthread_mutex_lock(&e->lock);
    for (uint32_t i = 0; i < e->count; i++) {
        CVPixelBufferRelease(e->buffers[i]);
        e->buffers[i] = NULL;
    }
    e->count = 0;
    for (uint32_t i = 0; i < count; i++) {
        if (CVPixelBufferCreateWithIOSurface(NULL, surfaces[i], NULL, &e->buffers[i]) !=
            kCVReturnSuccess) {
            fprintf(stderr, "rmhost: slot %u can't be wrapped\n", i);
            break;
        }
        e->count = i + 1;
    }
    e->generation = generation;
    e->force_key = true;
    if (!e->session) {
        e->session = make_session(e, width, height);
        e->width = width;
        e->height = height;
    }
    bool ok = e->session != NULL && e->count == count;
    pthread_mutex_unlock(&e->lock);
    return ok;
}

void rme_frame(rme *e, const rmf_frame_msg *msg)
{
    frame_ref f = {
        .valid = true,
        .generation = msg->generation,
        .slot = msg->slot,
        .sequence = msg->sequence,
        .host_time = msg->host_time,
    };

    pthread_mutex_lock(&e->lock);
    e->received++;
    if (!e->session || msg->generation != e->generation || msg->slot >= e->count) {
        give_back(e, &f);
    } else if (!e->active) {
        /* Nobody to send to. With hold_last the newest is kept unencoded, so
         * a viewer that comes while the picture is still gets it at once. */
        if (e->config.hold_last) {
            give_back(e, &e->held);
            e->held = f;
        } else {
            give_back(e, &f);
        }
    } else if (e->in_flight >= MAX_IN_FLIGHT) {
        if (e->waiting.valid) {
            e->skipped++;
            give_back(e, &e->waiting);
        }
        e->waiting = f;
    } else {
        encode_locked(e, f, false);
    }
    pthread_mutex_unlock(&e->lock);
}

void rme_set_active(rme *e, bool active)
{
    pthread_mutex_lock(&e->lock);
    e->active = active;
    if (active) {
        e->force_key = true;
    } else {
        give_back(e, &e->waiting);
    }
    pthread_mutex_unlock(&e->lock);
}

void rme_keyframe(rme *e)
{
    pthread_mutex_lock(&e->lock);
    e->force_key = true;
    if (e->config.hold_last) {
        dispatch_async_f(e->queue, e, repeat_held);
    }
    pthread_mutex_unlock(&e->lock);
}

void rme_set_bitrate(rme *e, int bitrate)
{
    pthread_mutex_lock(&e->lock);
    e->config.bitrate = bitrate;
    if (e->session) {
        set_rate(e->session, bitrate);
    }
    pthread_mutex_unlock(&e->lock);
}

void rme_flush(rme *e)
{
    VTCompressionSessionRef s;

    pthread_mutex_lock(&e->lock);
    e->active = false;
    give_back(e, &e->waiting);
    s = e->session;
    if (s) {
        CFRetain(s);
    }
    pthread_mutex_unlock(&e->lock);
    if (s) {
        VTCompressionSessionCompleteFrames(s, kCMTimeInvalid);
        CFRelease(s);
    }
    pthread_mutex_lock(&e->lock);
    give_back(e, &e->waiting);
    give_back(e, &e->held);
    pthread_mutex_unlock(&e->lock);
}

static int compare_double(const void *a, const void *b)
{
    double x = *(const double *)a, y = *(const double *)b;
    return x < y ? -1 : x > y;
}

void rme_get_stats(rme *e, rme_stats *out)
{
    pthread_mutex_lock(&e->lock);
    memset(out, 0, sizeof(*out));
    out->received = e->received;
    out->encoded = e->encoded;
    out->skipped = e->skipped;
    out->failed = e->failed;
    out->hardware = e->hardware;
    out->width = e->width;
    out->height = e->height;
    if (e->ms_count) {
        double *sorted = malloc(e->ms_count * sizeof(double));
        memcpy(sorted, e->ms, e->ms_count * sizeof(double));
        qsort(sorted, e->ms_count, sizeof(double), compare_double);
        out->ms_min = sorted[0];
        out->ms_median = sorted[e->ms_count / 2];
        out->ms_max = sorted[e->ms_count - 1];
        free(sorted);
    }
    pthread_mutex_unlock(&e->lock);
}

void rme_destroy(rme *e)
{
    rme_flush(e);
    if (e->session) {
        VTCompressionSessionInvalidate(e->session);
        CFRelease(e->session);
    }
    for (uint32_t i = 0; i < e->count; i++) {
        CVPixelBufferRelease(e->buffers[i]);
    }
    pthread_mutex_destroy(&e->lock);
    free(e->ms);
    e->queue = nil;
    free(e);
}
