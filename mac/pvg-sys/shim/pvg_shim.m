// SPDX-License-Identifier: GPL-2.0-or-later
//
// The calls into Apple's ParavirtualizedGraphics framework, as plain C functions for
// src/framework.rs. This is the one file that knows the framework's objects.
//
// It is Objective-C because the framework is: it takes blocks, hands out objects, and throws
// exceptions, and an exception must never travel up into Rust. Every function here catches what is
// thrown under it and says so in its answer.
//
// Three of the classes used are not in the framework's public headers: the list of guest memory
// ranges, the property that hands it to the device, and the surface mapper's device with its
// descriptor. Their declarations follow QEMU's hw/display/apple-gfx.h and apple-gfx-mmio.m with the
// patch "vmapple: apple-gfx: make it work on the latest macOS release" from QEMU's list, which are
// GPL-2.0-or-later as this file is. The mapper's `reset` is in neither: it is a method the class
// answers to at run time. Each is looked up by name, so a macOS build without one gets an error
// here and not a failure to start.

#import <Foundation/Foundation.h>
#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>
#import <ParavirtualizedGraphics/ParavirtualizedGraphics.h>
#import <QuartzCore/CAMetalLayer.h>
#include <objc/objc-exception.h>
#include <stdatomic.h>
#include <stdio.h>
#include <time.h>
#include <sys/sysctl.h>

typedef struct PGGuestPhysicalRange_s {
   uint64_t physicalAddress;
   uint64_t physicalLength;
   void *hostAddress;
} PGGuestPhysicalRange_t;

@interface PGMemoryMapDescriptor : NSObject
- (void)addRange:(PGGuestPhysicalRange_t)range;
@end

@interface PGDeviceDescriptor (MemoryMap)
@property(readwrite, nonatomic, copy, nullable) PGMemoryMapDescriptor *memoryMapDescriptor;
@end

@interface PGIOSurfaceHostDeviceDescriptor : NSObject
@property(readwrite, nonatomic, copy, nullable) PGMemoryMapDescriptor *memoryMapDescriptor;
@property(readwrite, nonatomic) unsigned long long mmioLength;
@property(readwrite, nonatomic, copy, nullable) bool (^raiseInterrupt)(uint32_t vector);
@end

@interface PGIOSurfaceHostDevice : NSObject
- (instancetype)initWithDescriptor:(PGIOSurfaceHostDeviceDescriptor *)descriptor;
- (uint64_t)mmioReadAtOffset:(uint64_t)offset;
- (void)mmioWriteAtOffset:(uint64_t)offset value:(uint64_t)value;
- (void)reset;
@end

// What src/framework.rs declares again, field for field.
typedef struct {
   uint64_t guest_address;
   uint64_t length;
   void *host_address;
} pvg_range;

typedef struct {
   uint16_t width;
   uint16_t height;
   uint16_t hertz;
} pvg_mode;

typedef struct {
   const char *name;
   uint32_t millimetres_wide;
   uint32_t millimetres_high;
   uint32_t serial;
   uint64_t mapper_register_bytes;
} pvg_display_config;

// Called from threads of the framework's choosing, and from inside a register write.
typedef struct {
   void *context;
   void (*interrupt)(void *context, uint32_t source, uint32_t vector);
   void (*mode_changed)(void *context, uint32_t width, uint32_t height, uint32_t format);
   void (*new_frame)(void *context);
   void (*cursor_shown)(void *context, bool shown);
   void (*cursor_moved)(void *context);
} pvg_callbacks;

enum {
   SOURCE_DEVICE = 0,
   SOURCE_MAPPER = 1,
   BYTES_PER_PIXEL = 4,
   FRAME_DONE = 0,
   FRAME_REFUSED = 1,
   FRAME_NO_ROOM = 2,
   FRAME_FAILED = 3,
};

@interface PVGHost : NSObject {
 @public
   id<MTLDevice> metal;
   id<MTLCommandQueue> queue;
   id<PGDevice> device;
   id<PGDisplay> display;
   PGIOSurfaceHostDevice *mapper;
   // The texture the last frame was read through, kept while the size stays.
   id<MTLTexture> texture;
   // The queue the framework calls the display's handlers on, and one of the host's own where
   // mode lists are set.
   dispatch_queue_t display_queue;
   dispatch_queue_t modes_queue;
   // Held by the mode list's setter, and by the thread that tells the device of the guest's
   // commands while it does: the two never overlap.
   dispatch_semaphore_t gate;
   // Given each time the display has presented a frame, while a mode list waits for one.
   dispatch_semaphore_t presented;
   atomic_bool waits_for_present;
   atomic_uint width;
   atomic_uint height;
}
@end

// How the display's queue is doing, for a host program's line when a thread stands still: the
// queue of the process's one display, when it last ran the little work pvg_display_ping hands it,
// and which of the display's handlers of ours it is in, if any.
static _Atomic(void *) watched_display_queue;
static _Atomic uint64_t display_answered;
static _Atomic(const char *) display_in;

@implementation PVGHost
@end

static void
say(char *error, size_t room, NSString *text)
{
   if (error && room)
      snprintf(error, room, "%s", text.UTF8String);
}

static PGMemoryMapDescriptor *
memory_map(const pvg_range *ranges, size_t count)
{
   Class class = NSClassFromString(@"PGMemoryMapDescriptor");
   if (!class || ![class instancesRespondToSelector:@selector(addRange:)])
      return nil;
   PGMemoryMapDescriptor *map = [class new];
   for (size_t i = 0; i < count; ++i) {
      PGGuestPhysicalRange_t range = { ranges[i].guest_address, ranges[i].length, ranges[i].host_address };
      [map addRange:range];
   }
   return map;
}

static PGIOSurfaceHostDevice *
new_mapper(const pvg_range *ranges, size_t count, uint64_t register_bytes, pvg_callbacks callbacks)
{
   Class descriptor_class = NSClassFromString(@"PGIOSurfaceHostDeviceDescriptor");
   Class device_class = NSClassFromString(@"PGIOSurfaceHostDevice");
   if (!descriptor_class || !device_class ||
       ![descriptor_class instancesRespondToSelector:@selector(setMemoryMapDescriptor:)] ||
       ![descriptor_class instancesRespondToSelector:@selector(setMmioLength:)] ||
       ![descriptor_class instancesRespondToSelector:@selector(setRaiseInterrupt:)] ||
       ![device_class instancesRespondToSelector:@selector(initWithDescriptor:)] ||
       ![device_class instancesRespondToSelector:@selector(mmioReadAtOffset:)] ||
       ![device_class instancesRespondToSelector:@selector(mmioWriteAtOffset:value:)] ||
       ![device_class instancesRespondToSelector:@selector(reset)])
      return nil;

   PGIOSurfaceHostDeviceDescriptor *descriptor = [descriptor_class new];
   // A list of its own: the descriptor copies what it is given.
   descriptor.memoryMapDescriptor = memory_map(ranges, count);
   descriptor.mmioLength = register_bytes;
   descriptor.raiseInterrupt = ^bool(uint32_t vector) {
      callbacks.interrupt(callbacks.context, SOURCE_MAPPER, vector);
      return true;
   };
   return [[device_class alloc] initWithDescriptor:descriptor];
}

static NSArray<PGDisplayMode *> *
mode_list(const pvg_mode *modes, size_t count)
{
   NSMutableArray<PGDisplayMode *> *list = [NSMutableArray arrayWithCapacity:count];
   for (size_t i = 0; i < count; ++i) {
      PGDisplayCoord_t size = { modes[i].width, modes[i].height };
      PGDisplayMode *mode = [[PGDisplayMode alloc] initWithSizeInPixels:size refreshRateInHz:modes[i].hertz];
      if (!mode)
         return nil;
      [list addObject:mode];
   }
   return list;
}

// The macOS build, as `sw_vers -buildVersion` gives it.
bool
pvg_host_build(char *build, size_t room)
{
   size_t size = room;
   return room && sysctlbyname("kern.osversion", build, &size, NULL, 0) == 0 && size <= room;
}

// Where the framework keeps the option ROM for the device's PCI function: an EFI driver that a
// guest's firmware may load. The public header calls it no longer used, and still declares it.
bool
pvg_option_rom_path(char *path, size_t room)
{
   @autoreleasepool {
      @try {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
         NSURL *url = PGCopyOptionROMURL();
#pragma clang diagnostic pop
         const char *text = url.fileSystemRepresentation;
         if (!text || !room || strlen(text) >= room)
            return false;
         snprintf(path, room, "%s", text);
         return true;
      } @catch (id thrown) {
         return false;
      }
   }
}

// How many bytes of registers the device has.
uint64_t
pvg_device_register_bytes(void)
{
   @autoreleasepool {
      return [PGDeviceDescriptor new].mmioLength;
   }
}

// Makes the device over these ranges of guest memory, one display on its first port, and the
// surface mapper's device over ranges of its own. Returns the host, or NULL with the reason in
// `error`.
void *
pvg_host_new(const pvg_range *ranges, size_t count, const pvg_range *mapper_ranges,
             size_t mapper_count, const pvg_display_config *config, const pvg_mode *modes,
             size_t mode_count, const pvg_callbacks *given, char *error, size_t error_room)
{
   @autoreleasepool {
      @try {
         pvg_callbacks callbacks = *given;
         PVGHost *host = [PVGHost new];

         host->metal = MTLCreateSystemDefaultDevice();
         if (!host->metal) {
            say(error, error_room, @"this Mac has no Metal device");
            return NULL;
         }
         host->queue = [host->metal newCommandQueue];

         PGDeviceDescriptor *descriptor = [PGDeviceDescriptor new];
         PGMemoryMapDescriptor *map = memory_map(ranges, count);
         if (!map || ![descriptor respondsToSelector:@selector(setMemoryMapDescriptor:)]) {
            say(error, error_room, @"the framework takes no list of guest memory on this macOS");
            return NULL;
         }
         descriptor.device = host->metal;
         descriptor.memoryMapDescriptor = map;
         descriptor.raiseInterrupt = ^(uint32_t vector) {
            callbacks.interrupt(callbacks.context, SOURCE_DEVICE, vector);
         };

         // The mapper first, as the framework's own users make it.
         host->mapper = new_mapper(mapper_ranges, mapper_count, config->mapper_register_bytes, callbacks);
         if (!host->mapper) {
            say(error, error_room, @"the framework has no surface mapper of the known shape on this macOS");
            return NULL;
         }
         host->device = PGCreateDeviceWithDescriptor(descriptor);
         if (!host->device) {
            say(error, error_room, @"the framework made no device");
            return NULL;
         }

         PGDisplayDescriptor *display = [PGDisplayDescriptor new];
         __weak PVGHost *weak = host;
         display.name = @(config->name);
         display.sizeInMillimeters = NSMakeSize(config->millimetres_wide, config->millimetres_high);
         host->display_queue = dispatch_queue_create("guesthouse.pvg.display", DISPATCH_QUEUE_SERIAL);
         host->modes_queue = dispatch_queue_create("guesthouse.pvg.modes", DISPATCH_QUEUE_SERIAL);
         // Made empty and given its one place after, so that it may be let go while it is held:
         // libdispatch ends a process that lets go of a semaphore below the value it was made with.
         host->gate = dispatch_semaphore_create(0);
         dispatch_semaphore_signal(host->gate);
         host->presented = dispatch_semaphore_create(0);
         display.queue = host->display_queue;
         atomic_store(&watched_display_queue, (__bridge void *)host->display_queue);
         display.modeChangeHandler = ^(PGDisplayCoord_t size, OSType format) {
            atomic_store(&display_in, "modeChangeHandler");
            PVGHost *strong = weak;
            if (strong) {
               atomic_store(&strong->width, size.x);
               atomic_store(&strong->height, size.y);
            }
            callbacks.mode_changed(callbacks.context, size.x, size.y, format);
            atomic_store(&display_in, NULL);
         };
         display.newFrameEventHandler = ^{
            atomic_store(&display_in, "newFrameEventHandler");
            PVGHost *strong = weak;
            if (strong && atomic_exchange(&strong->waits_for_present, false))
               dispatch_semaphore_signal(strong->presented);
            callbacks.new_frame(callbacks.context);
            atomic_store(&display_in, NULL);
         };
         display.cursorShowHandler = ^(BOOL shown) {
            atomic_store(&display_in, "cursorShowHandler");
            callbacks.cursor_shown(callbacks.context, shown);
            atomic_store(&display_in, NULL);
         };
         display.cursorMoveHandler = ^{
            atomic_store(&display_in, "cursorMoveHandler");
            callbacks.cursor_moved(callbacks.context);
            atomic_store(&display_in, NULL);
         };
         host->display = [host->device newDisplayWithDescriptor:display port:0 serialNum:config->serial];
         if (!host->display) {
            say(error, error_room, @"the device made no display");
            return NULL;
         }
         NSArray<PGDisplayMode *> *list = mode_list(modes, mode_count);
         if (!list) {
            say(error, error_room, @"the framework refused a display mode");
            return NULL;
         }
         host->display.modeList = list;
         return (__bridge_retained void *)host;
      } @catch (id thrown) {
         say(error, error_room, [NSString stringWithFormat:@"the framework threw: %@", thrown]);
         return NULL;
      }
   }
}

// Lets the host's objects go. Not for a host whose display the guest left half handed over: see
// src/device.rs.
void
pvg_host_free(void *opaque)
{
   @autoreleasepool {
      @try {
         CFBridgingRelease(opaque);
      } @catch (id thrown) {
      }
   }
}

bool
pvg_device_read(void *opaque, uint64_t offset, uint32_t *value)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   @autoreleasepool {
      @try {
         *value = [host->device mmioReadAtOffset:offset];
         return true;
      } @catch (id thrown) {
         return false;
      }
   }
}

bool
pvg_device_write(void *opaque, uint64_t offset, uint32_t value)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   @autoreleasepool {
      @try {
         [host->device mmioWriteAtOffset:offset value:value];
         return true;
      } @catch (id thrown) {
         return false;
      }
   }
}

bool
pvg_mapper_read(void *opaque, uint64_t offset, uint64_t *value)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   @autoreleasepool {
      @try {
         *value = [host->mapper mmioReadAtOffset:offset];
         return true;
      } @catch (id thrown) {
         return false;
      }
   }
}

bool
pvg_mapper_write(void *opaque, uint64_t offset, uint64_t value)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   @autoreleasepool {
      @try {
         [host->mapper mmioWriteAtOffset:offset value:value];
         return true;
      } @catch (id thrown) {
         return false;
      }
   }
}

// The device's reset, and then the mapper's, which the device's doesn't reach.
// Takes the gate between the mode list's setter and the first ring, waiting at most milliseconds
// for it; false if it is not had in that time. pvg_host_gate_give gives back one that was had.
bool
pvg_host_gate_take(void *opaque, uint64_t milliseconds)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   return dispatch_semaphore_wait(host->gate, dispatch_time(DISPATCH_TIME_NOW, (int64_t)(milliseconds * NSEC_PER_MSEC))) == 0;
}

void
pvg_host_gate_give(void *opaque)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   dispatch_semaphore_signal(host->gate);
}

bool
pvg_host_reset(void *opaque)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   @autoreleasepool {
      @try {
         [host->device reset];
         [host->mapper reset];
         return true;
      } @catch (id thrown) {
         return false;
      }
   }
}

// Gives the display a new mode list, from a serial queue of the host's own and not the caller's
// thread, and returns at once; done is called there with whether the framework took the list and
// how long its setter took, in microseconds. Lists are set in the order given.
//
// The setter once in about 100 lists did not come back, wherever it was called: on the thread that
// serves the guest's registers, on the display's queue (where QEMU's apple-gfx sets it) and on this
// queue. It had not written the list into the state page, and the device then took none of the
// guest's swaps, until the guest turned its ring off (seen 2026-11-27, macOS 27.2.1, by our own
// lines, with full screen in and out every 2.8 s and GL clients drawing). So it is set only right
// after the display has presented a frame (or after 50 ms with none, a display at rest), with the
// gate held, so that the device is not told of the guest's next swap while the setter is in. With
// that, 400 lists in a row went through. Nothing waits on this queue: a setter that stands holds
// this queue and the gate, and a kick then waits for the gate 100 ms and goes on without it.
//
// False, and no call of done, for a list the framework refuses to make.
bool
pvg_host_set_modes(void *opaque, const pvg_mode *modes, size_t count,
                   void (*done)(void *context, bool taken, uint64_t microseconds), void *context)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   @autoreleasepool {
      NSArray<PGDisplayMode *> *list = nil;
      @try {
         list = mode_list(modes, count);
      } @catch (id thrown) {
      }
      if (!list)
         return false;
      dispatch_async(host->modes_queue, ^{
         @autoreleasepool {
            // Right after a present, and then with the gate: see above.
            while (dispatch_semaphore_wait(host->presented, DISPATCH_TIME_NOW) == 0)
               ;
            atomic_store(&host->waits_for_present, true);
            dispatch_semaphore_wait(host->presented, dispatch_time(DISPATCH_TIME_NOW, 50 * NSEC_PER_MSEC));
            atomic_store(&host->waits_for_present, false);
            dispatch_semaphore_wait(host->gate, DISPATCH_TIME_FOREVER);
            uint64_t start = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
            bool taken = true;
            @try {
               host->display.modeList = list;
            } @catch (id thrown) {
               taken = false;
            }
            uint64_t took = (clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - start) / 1000;
            dispatch_semaphore_signal(host->gate);
            done(context, taken, took);
         }
      });
      return true;
   }
}

// Asks the display's queue to note that it has run, as soon as it runs what was handed to it.
void
pvg_display_ping(void)
{
   dispatch_queue_t queue = (__bridge dispatch_queue_t)atomic_load(&watched_display_queue);
   if (!queue)
      return;
   uint64_t first = 0;
   atomic_compare_exchange_strong(&display_answered, &first, clock_gettime_nsec_np(CLOCK_UPTIME_RAW));
   dispatch_async(queue, ^{
      atomic_store(&display_answered, clock_gettime_nsec_np(CLOCK_UPTIME_RAW));
   });
}

// How the display's queue is doing, from any thread: false before there is a display, else how
// many milliseconds ago it last ran pvg_display_ping's work, and the handler of ours it is in
// (NULL for none).
bool
pvg_display_look(uint64_t *answered_ago, const char **inside)
{
   uint64_t answered = atomic_load(&display_answered), now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
   if (!atomic_load(&watched_display_queue))
      return false;
   *answered_ago = answered && now > answered ? (now - answered) / 1000000 : 0;
   *inside = atomic_load(&display_in);
   return true;
}

// The size of the picture the guest last swapped to, zero by zero before the first.
void
pvg_host_frame_size(void *opaque, uint32_t *width, uint32_t *height)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   *width = atomic_load(&host->width);
   *height = atomic_load(&host->height);
}

// How many pictures the guest has presented, and how many the host has taken.
void
pvg_host_presents(void *opaque, uint64_t *by_guest, uint64_t *by_host)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   @autoreleasepool {
      *by_guest = host->display.guestPresentCount;
      *by_host = host->display.hostPresentCount;
   }
}

// Has the display draw its current frame into a texture and copies that out: four bytes a pixel,
// blue first, rows from the top with no padding.
int
pvg_host_frame(void *opaque, uint32_t width, uint32_t height, uint8_t *pixels, size_t room)
{
   PVGHost *host = (__bridge PVGHost *)opaque;
   if (!width || !height || room / BYTES_PER_PIXEL / width < height)
      return FRAME_NO_ROOM;
   @autoreleasepool {
      @try {
         if (!host->texture || host->texture.width != width || host->texture.height != height) {
            MTLTextureDescriptor *descriptor =
               [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
                                                                  width:width
                                                                 height:height
                                                              mipmapped:NO];
            descriptor.usage = host->display.minimumTextureUsage;
            descriptor.storageMode = MTLStorageModeShared;
            host->texture = [host->metal newTextureWithDescriptor:descriptor];
            if (!host->texture)
               return FRAME_FAILED;
         }
         id<MTLCommandBuffer> commands = [host->queue commandBuffer];
         MTLRegion region = MTLRegionMake2D(0, 0, width, height);
         if (![host->display encodeCurrentFrameToCommandBuffer:commands texture:host->texture region:region])
            return FRAME_REFUSED;
         [commands commit];
         [commands waitUntilCompleted];
         if (commands.status != MTLCommandBufferStatusCompleted)
            return FRAME_FAILED;
         [host->texture getBytes:pixels
                     bytesPerRow:(NSUInteger)width * BYTES_PER_PIXEL
                      fromRegion:region
                     mipmapLevel:0];
         return FRAME_DONE;
      } @catch (id thrown) {
         return FRAME_FAILED;
      }
   }
}

// A hold on the host for a thread that draws the display's picture while the device's own thread
// serves the guest. It is the host itself, kept once more.
void *
pvg_frame_source_new(void *opaque)
{
   return (void *)CFBridgingRetain((__bridge PVGHost *)opaque);
}

void
pvg_frame_source_free(void *source)
{
   @autoreleasepool {
      @try {
         CFBridgingRelease(source);
      } @catch (id thrown) {
      }
   }
}

// Makes a layer one the display can draw its frame into.
void
pvg_frame_source_set_up(void *source, void *opaque_layer)
{
   PVGHost *host = (__bridge PVGHost *)source;
   CAMetalLayer *layer = (__bridge CAMetalLayer *)opaque_layer;
   @autoreleasepool {
      layer.device = host->metal;
      layer.pixelFormat = MTLPixelFormatBGRA8Unorm;
      // The display writes the frame with whatever its minimum usage names, which is more than
      // a layer's textures allow by default.
      layer.framebufferOnly = NO;
   }
}

// Where draw_frame is, for a host program's line when its drawing thread stands still: NULL
// outside it.
static _Atomic(const char *) draw_step;

const char *
pvg_frame_source_step(void)
{
   return atomic_load(&draw_step);
}

// Draws the display's current frame into the layer's next drawable and shows it. With room for
// it, the drawable's pixels are also copied out before it is shown: four bytes a pixel, blue
// first, rows from the top, as many as *width and *height then say.
static bool
draw_frame(PVGHost *host, CAMetalLayer *layer, uint8_t *pixels, size_t room, uint32_t *copied_width,
           uint32_t *copied_height)
{
   @autoreleasepool {
      @try {
         uint32_t width = atomic_load(&host->width), height = atomic_load(&host->height);
         if (!width || !height)
            return false;
         atomic_store(&draw_step, "the layer's drawable size");
         CGSize size = layer.drawableSize;
         if (size.width != width || size.height != height)
            layer.drawableSize = CGSizeMake(width, height);
         atomic_store(&draw_step, "the layer's next drawable");
         id<CAMetalDrawable> drawable = [layer nextDrawable];
         atomic_store(&draw_step, NULL);
         if (!drawable)
            return false;
         atomic_store(&draw_step, "the frame encoded");
         id<MTLCommandBuffer> commands = [host->queue commandBuffer];
         MTLRegion region = MTLRegionMake2D(0, 0, drawable.texture.width, drawable.texture.height);
         bool encoded = [host->display encodeCurrentFrameToCommandBuffer:commands
                                                                  texture:drawable.texture
                                                                   region:region];
         atomic_store(&draw_step, NULL);
         if (!encoded)
            return false;
         size_t row = (size_t)drawable.texture.width * 4, bytes = row * drawable.texture.height;
         id<MTLBuffer> copy = nil;
         if (pixels && bytes <= room) {
            copy = [host->metal newBufferWithLength:bytes options:MTLResourceStorageModeShared];
            id<MTLBlitCommandEncoder> blit = [commands blitCommandEncoder];
            [blit copyFromTexture:drawable.texture
                             sourceSlice:0
                             sourceLevel:0
                            sourceOrigin:MTLOriginMake(0, 0, 0)
                              sourceSize:MTLSizeMake(drawable.texture.width, drawable.texture.height, 1)
                                toBuffer:copy
                       destinationOffset:0
                  destinationBytesPerRow:row
                destinationBytesPerImage:bytes];
            [blit endEncoding];
         }
         atomic_store(&draw_step, "the drawable presented");
         [commands presentDrawable:drawable];
         [commands commit];
         atomic_store(&draw_step, NULL);
         if (copy) {
            atomic_store(&draw_step, "the copy waited for");
            [commands waitUntilCompleted];
            atomic_store(&draw_step, NULL);
            if (commands.status != MTLCommandBufferStatusCompleted)
               return false;
            memcpy(pixels, copy.contents, bytes);
            *copied_width = (uint32_t)drawable.texture.width;
            *copied_height = (uint32_t)drawable.texture.height;
         }
         return true;
      } @catch (id thrown) {
         return false;
      }
   }
}

bool
pvg_frame_source_draw(void *source, void *opaque_layer)
{
   return draw_frame((__bridge PVGHost *)source, (__bridge CAMetalLayer *)opaque_layer, NULL, 0, NULL, NULL);
}

// The size of the display's current frame, in pixels.
void
pvg_frame_source_size(void *source, uint32_t *width, uint32_t *height)
{
   PVGHost *host = (__bridge PVGHost *)source;
   *width = atomic_load(&host->width);
   *height = atomic_load(&host->height);
}

// As pvg_frame_source_draw, and copies what went into the layer to pixels, which has room for
// room bytes. False if it had too little.
bool
pvg_frame_source_draw_copy(void *source, void *opaque_layer, uint8_t *pixels, size_t room, uint32_t *width,
                           uint32_t *height)
{
   *width = *height = 0;
   return draw_frame((__bridge PVGHost *)source, (__bridge CAMetalLayer *)opaque_layer, pixels, room, width,
                     height) &&
          *width;
}

// The Metal device the display draws with. A texture the frame is drawn into has to be this
// device's. The host holds it for as long as the source lives.
void *
pvg_frame_source_metal(void *source)
{
   PVGHost *host = (__bridge PVGHost *)source;
   return (__bridge void *)host->metal;
}

// What a texture the frame is drawn into has to allow, as an MTLTextureUsage.
uint64_t
pvg_frame_source_texture_usage(void *source)
{
   PVGHost *host = (__bridge PVGHost *)source;
   @autoreleasepool {
      @try {
         return host->display.minimumTextureUsage;
      } @catch (id thrown) {
         return 0;
      }
   }
}

// Draws the display's current frame into a texture of the caller's, such as one over an IOSurface
// another process reads, and returns once the GPU is done with it. Only a texture of the
// picture's own size is drawn into: the picture is never scaled here.
bool
pvg_frame_source_draw_texture(void *source, void *opaque_texture)
{
   PVGHost *host = (__bridge PVGHost *)source;
   id<MTLTexture> texture = (__bridge id<MTLTexture>)opaque_texture;
   @autoreleasepool {
      @try {
         uint32_t width = atomic_load(&host->width), height = atomic_load(&host->height);
         if (!width || texture.width != width || texture.height != height)
            return false;
         id<MTLCommandBuffer> commands = [host->queue commandBuffer];
         MTLRegion region = MTLRegionMake2D(0, 0, width, height);
         if (![host->display encodeCurrentFrameToCommandBuffer:commands texture:texture region:region])
            return false;
         [commands commit];
         [commands waitUntilCompleted];
         return commands.status == MTLCommandBufferStatusCompleted;
      } @catch (id thrown) {
         return false;
      }
   }
}

void
pvg_frame_source_cursor(void *source, int32_t *x, int32_t *y)
{
   PVGHost *host = (__bridge PVGHost *)source;
   @autoreleasepool {
      PGDisplayCoord_t place = host->display.cursorPosition;
      *x = place.x;
      *y = place.y;
   }
}

// The framework throws when it refuses a guest's command, catches that itself, and logs it with
// the reason blanked. The runtime shows every exception thrown in the process to one function
// first, and this is how a host program hears the reasons: set once, for the whole process.
static void (*reason_seen)(void *context, const char *reason);
static void *reason_context;
static objc_exception_preprocessor reason_next;

static id
show_reason(id thrown)
{
   if ([thrown isKindOfClass:[NSException class]]) {
      const char *reason = [[thrown reason] UTF8String];
      if (reason)
         reason_seen(reason_context, reason);
   }
   return reason_next ? reason_next(thrown) : thrown;
}

void
pvg_watch_reasons(void (*seen)(void *context, const char *reason), void *context)
{
   reason_seen = seen;
   reason_context = context;
   reason_next = objc_setExceptionPreprocessor(show_reason);
}
