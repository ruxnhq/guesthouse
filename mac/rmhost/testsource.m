/*
 * testsource: a stand-in for QEMU's apple-gfx device, for trying rmhost with
 * no guest. It draws a moving test pattern at 60 Hz into IOSurface slots with
 * Metal and publishes each frame through rmframes_pub.c and rmframes_recv.c,
 * the same code the QEMU diff builds in.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * Usage: testsource [-n NAME] [-t SECONDS] [-s WxH] [-r WxH]
 *   -n  the bootstrap name to publish under (rmhost.frames)
 *   -t  how long to run, 0 for ever (0)
 *   -s  the picture size (1920x1080)
 *   -r  change to this size halfway through, to try a new generation
 */
#import <Foundation/Foundation.h>
#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>
#include <mach/mach_time.h>
#include <stdatomic.h>
#include <getopt.h>

#include "rmframes.h"
#include "rmframes_pub.h"

/* As many as a SLOTS message carries: room for several readers. */
#define SLOT_COUNT RMF_MAX_SLOTS
#define FRAME_RATE 60

/* Bars that scroll, a box that crosses the screen once a second, and the frame
 * number as 16 white or black squares along the top, so a recording can be
 * checked frame by frame. */
static NSString *const kPatternSource =
    @"#include <metal_stdlib>\n"
     "using namespace metal;\n"
     "kernel void pattern(texture2d<float, access::write> out [[texture(0)]],\n"
     "                    constant uint &frame [[buffer(0)]],\n"
     "                    uint2 p [[thread_position_in_grid]]) {\n"
     "    uint w = out.get_width(), h = out.get_height();\n"
     "    if (p.x >= w || p.y >= h) return;\n"
     "    float4 c = float4(float((p.x + frame * 4) % 256) / 255.0,\n"
     "                      float(p.y % 256) / 255.0, 0.5, 1.0);\n"
     "    uint box = (frame % 60) * (w - 200) / 59;\n"
     "    if (p.x >= box && p.x < box + 200 && p.y >= h / 2 - 100 && p.y < h / 2 + 100)\n"
     "        c = float4(1.0, 1.0, 1.0, 1.0);\n"
     "    if (p.y < 40 && p.x < 16 * 40) {\n"
     "        uint bit = 15 - p.x / 40;\n"
     "        c = ((frame >> bit) & 1) ? float4(1.0) : float4(0.0, 0.0, 0.0, 1.0);\n"
     "    }\n"
     "    out.write(c.bgra, p);\n"
     "}\n";

typedef struct Slots {
    uint32_t width, height;
    IOSurfaceRef surface[SLOT_COUNT];
    id<MTLTexture> texture[SLOT_COUNT];
} Slots;

static id<MTLDevice> device;
static id<MTLCommandQueue> queue;
static id<MTLComputePipelineState> pipeline;
static rmf_pub *pub;
static Slots *slots;
static uint32_t frame_number;
static _Atomic uint64_t drawn, no_slot;

static IOSurfaceRef make_surface(uint32_t width, uint32_t height)
{
    size_t row = IOSurfaceAlignProperty(kIOSurfaceBytesPerRow, width * 4);
    NSDictionary *props = @{
        (id)kIOSurfaceWidth: @(width),
        (id)kIOSurfaceHeight: @(height),
        (id)kIOSurfaceBytesPerElement: @4,
        (id)kIOSurfaceBytesPerRow: @(row),
        (id)kIOSurfaceAllocSize: @(IOSurfaceAlignProperty(kIOSurfaceAllocSize, row * height)),
        (id)kIOSurfacePixelFormat: @(RMF_FORMAT_BGRA),
    };
    return IOSurfaceCreate((__bridge CFDictionaryRef)props);
}

static Slots *make_slots(uint32_t width, uint32_t height)
{
    Slots *s = calloc(1, sizeof(*s));
    MTLTextureDescriptor *desc =
        [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
                                                           width:width
                                                          height:height
                                                       mipmapped:NO];
    desc.usage = MTLTextureUsageShaderWrite | MTLTextureUsageShaderRead;
    desc.storageMode = MTLStorageModeShared;
    s->width = width;
    s->height = height;
    for (int i = 0; i < SLOT_COUNT; i++) {
        s->surface[i] = make_surface(width, height);
        s->texture[i] = [device newTextureWithDescriptor:desc iosurface:s->surface[i] plane:0];
    }
    rmf_pub_set_slots(pub, s->surface, SLOT_COUNT, width, height);
    return s;
}

static void draw_frame(void)
{
    uint32_t generation;
    int slot = rmf_pub_acquire(pub, &generation);

    frame_number++;
    if (slot < 0) {
        /* The readers hold every slot: QEMU draws into its own texture
         * then, and so this frame is just not published. */
        atomic_fetch_add(&no_slot, 1);
        return;
    }
    @autoreleasepool {
        id<MTLCommandBuffer> cb = [queue commandBuffer];
        id<MTLComputeCommandEncoder> enc = [cb computeCommandEncoder];
        id<MTLTexture> tex = slots->texture[slot];
        uint32_t n = frame_number;

        [enc setComputePipelineState:pipeline];
        [enc setTexture:tex atIndex:0];
        [enc setBytes:&n length:sizeof(n) atIndex:0];
        [enc dispatchThreads:MTLSizeMake(tex.width, tex.height, 1)
            threadsPerThreadgroup:MTLSizeMake(16, 16, 1)];
        [enc endEncoding];
        [cb addCompletedHandler:^(id<MTLCommandBuffer> done) {
            if (done.status == MTLCommandBufferStatusCompleted) {
                atomic_fetch_add(&drawn, 1);
                rmf_pub_publish(pub, generation, slot, mach_absolute_time());
            } else {
                rmf_pub_cancel(pub, generation, slot);
            }
        }];
        [cb commit];
    }
}

static bool parse_size(const char *text, uint32_t *w, uint32_t *h)
{
    return sscanf(text, "%ux%u", w, h) == 2 && *w >= 64 && *h >= 64 &&
           *w <= 8192 && *h <= 8192;
}

int main(int argc, char **argv)
{
    const char *name = "rmhost.frames";
    double seconds = 0;
    uint32_t width = 1920, height = 1080, width2 = 0, height2 = 0;
    char err[256];
    int opt;

    while ((opt = getopt(argc, argv, "n:t:s:r:")) != -1) {
        switch (opt) {
        case 'n': name = optarg; break;
        case 't': seconds = atof(optarg); break;
        case 's':
            if (!parse_size(optarg, &width, &height)) goto usage;
            break;
        case 'r':
            if (!parse_size(optarg, &width2, &height2)) goto usage;
            break;
        default:
            goto usage;
        }
    }

    @autoreleasepool {
        NSError *error = nil;
        device = MTLCreateSystemDefaultDevice();
        queue = [device newCommandQueue];
        id<MTLLibrary> lib = [device newLibraryWithSource:kPatternSource options:nil error:&error];
        if (!lib) {
            fprintf(stderr, "testsource: shader: %s\n", error.description.UTF8String);
            return 1;
        }
        pipeline = [device newComputePipelineStateWithFunction:[lib newFunctionWithName:@"pattern"]
                                                         error:&error];
        pub = rmf_pub_open(name, err, sizeof(err));
        if (!pub) {
            fprintf(stderr, "testsource: %s\n", err);
            return 1;
        }
        slots = make_slots(width, height);
        printf("testsource: publishing %ux%u at %d Hz as %s\n", width, height, FRAME_RATE, name);
        fflush(stdout);

        dispatch_queue_t q = dispatch_queue_create("testsource.draw", NULL);
        dispatch_source_t timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, q);
        uint64_t start = mach_absolute_time();
        __block bool resized = false;
        dispatch_source_set_timer(timer, DISPATCH_TIME_NOW, NSEC_PER_SEC / FRAME_RATE, 0);
        dispatch_source_set_event_handler(timer, ^{
            if (width2 && !resized && frame_number >= seconds * FRAME_RATE / 2) {
                resized = true;
                slots = make_slots(width2, height2);  /* the old ones are left: a test */
                printf("testsource: now %ux%u\n", width2, height2);
                fflush(stdout);
            }
            draw_frame();
        });
        dispatch_resume(timer);

        if (seconds > 0) {
            [NSThread sleepForTimeInterval:seconds];
        } else {
            dispatch_main();
        }
        dispatch_source_cancel(timer);
        dispatch_sync(q, ^{});
        [NSThread sleepForTimeInterval:0.1];

        mach_timebase_info_data_t tb;
        mach_timebase_info(&tb);
        uint64_t sent, dropped;
        rmf_pub_counts(pub, &sent, &dropped);
        printf("testsource: %u ticks in %.1f s, %llu drawn, %llu frame messages sent, "
               "%llu dropped (a reader's queue full), %llu with no free slot; "
               "%u readers, %llu messages ignored\n",
               frame_number, (mach_absolute_time() - start) * tb.numer / tb.denom / 1e9,
               atomic_load(&drawn), sent, dropped, atomic_load(&no_slot),
               rmf_pub_readers(pub), rmf_pub_ignored(pub));
        rmf_pub_close(pub);
    }
    return 0;

usage:
    fprintf(stderr, "usage: testsource [-n NAME] [-t SECONDS] [-s WxH] [-r WxH]\n");
    return 2;
}
