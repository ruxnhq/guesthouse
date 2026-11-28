// SPDX-License-Identifier: GPL-2.0-or-later
//
// The guest's frames for other processes, as plain C functions for src/export.rs.
//
// A few IOSurfaces, called slots, each with a Metal texture over it, are handed to readers
// through rmframes (rmframes/, the same files QEMU's frame-export uses). The display draws a
// frame into a slot's texture on the GPU, and each reader gets the slot's IOSurface by its Mach
// port. Nothing here reads or writes a pixel, and nothing locks a surface.
//
// pvg_export_open may be called from any thread. Every other function is called from the one
// thread src/export.rs keeps, so the slots change only between frames. What readers send, and
// the notice that one died, are taken on rmframes' own queue.

#import <Foundation/Foundation.h>
#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>
#include <mach/mach_time.h>

#include "rmframes/rmframes.h"
#include "rmframes/rmframes_pub.h"

enum {
   // All a SLOTS message carries, as QEMU's frame-export has: rmhost holds up to four (one kept
   // for a viewer, two in its encoder, one waiting), the app up to three, and one is drawn.
   SLOTS = RMF_MAX_SLOTS,
   BYTES_PER_PIXEL = 4,
};

@interface PVGExport : NSObject {
 @public
   rmf_pub *publisher;
   IOSurfaceRef surfaces[SLOTS];
   id<MTLTexture> textures[SLOTS];
   uint32_t count;
}
@end

@implementation PVGExport
- (void)dealloc
{
   for (uint32_t i = 0; i < count; ++i)
      CFRelease(surfaces[i]);
   if (publisher)
      rmf_pub_close(publisher);
}
@end

// Checks in `name` with the bootstrap server, for readers to look up. NULL if the name is taken
// or refused, with the reason in `error`.
void *
pvg_export_open(const char *name, char *error, size_t room)
{
   @autoreleasepool {
      rmf_pub *publisher = rmf_pub_open(name, error, room);
      if (!publisher)
         return NULL;
      PVGExport *export = [PVGExport new];
      export->publisher = publisher;
      return (__bridge_retained void *)export;
   }
}

static IOSurfaceRef
new_surface(uint32_t width, uint32_t height)
{
   size_t row = IOSurfaceAlignProperty(kIOSurfaceBytesPerRow, (size_t)width * BYTES_PER_PIXEL);
   NSDictionary *properties = @{
      (id)kIOSurfaceWidth : @(width),
      (id)kIOSurfaceHeight : @(height),
      (id)kIOSurfaceBytesPerElement : @(BYTES_PER_PIXEL),
      (id)kIOSurfaceBytesPerRow : @(row),
      (id)kIOSurfaceAllocSize : @(IOSurfaceAlignProperty(kIOSurfaceAllocSize, row * height)),
      (id)kIOSurfacePixelFormat : @(RMF_FORMAT_BGRA),
   };
   return IOSurfaceCreate((__bridge CFDictionaryRef)properties);
}

// New slots for a picture of this size, on the display's Metal device and with the usage its
// frames need, announced to the reader with a new generation. Slots of the old size are not
// handed out again; a reader that still holds one keeps its own reference to the surface.
// Returns how many slots were made: none for a size of zero, or if the GPU had no room.
uint32_t
pvg_export_resize(void *opaque, void *opaque_metal, uint64_t usage, uint32_t width, uint32_t height)
{
   PVGExport *export = (__bridge PVGExport *)opaque;
   id<MTLDevice> metal = (__bridge id<MTLDevice>)opaque_metal;
   @autoreleasepool {
      for (uint32_t i = 0; i < export->count; ++i) {
         export->textures[i] = nil;
         CFRelease(export->surfaces[i]);
         export->surfaces[i] = NULL;
      }
      export->count = 0;
      if (width && height) {
         MTLTextureDescriptor *descriptor =
            [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
                                                               width:width
                                                              height:height
                                                           mipmapped:NO];
         descriptor.usage = (MTLTextureUsage)usage;
         descriptor.storageMode = MTLStorageModeShared;
         for (uint32_t i = 0; i < SLOTS; ++i) {
            IOSurfaceRef surface = new_surface(width, height);
            id<MTLTexture> texture =
               surface ? [metal newTextureWithDescriptor:descriptor iosurface:surface plane:0] : nil;
            if (!texture) {
               if (surface)
                  CFRelease(surface);
               break;
            }
            export->surfaces[i] = surface;
            export->textures[i] = texture;
            export->count++;
         }
      }
      rmf_pub_set_slots(export->publisher, export->surfaces, export->count, width, height);
      return export->count;
   }
}

// A slot no reader holds, now the caller's to draw into: its texture, its number and its
// generation. NULL when readers hold every slot. With no reader there the slot keeps the newest
// frame for one that comes later, which rmframes then sends it.
void *
pvg_export_acquire(void *opaque, int32_t *slot, uint32_t *generation)
{
   PVGExport *export = (__bridge PVGExport *)opaque;
   int taken = rmf_pub_acquire(export->publisher, generation);
   if (taken < 0)
      return NULL;
   if ((uint32_t)taken >= export->count) {
      rmf_pub_cancel(export->publisher, *generation, taken);
      return NULL;
   }
   *slot = taken;
   return (__bridge void *)export->textures[taken];
}

// The slot's frame is drawn: every reader is told, without waiting for any.
void
pvg_export_publish(void *opaque, int32_t slot, uint32_t generation)
{
   PVGExport *export = (__bridge PVGExport *)opaque;
   rmf_pub_publish(export->publisher, generation, slot, mach_absolute_time());
}

// Nothing was drawn into the slot after all.
void
pvg_export_cancel(void *opaque, int32_t slot, uint32_t generation)
{
   PVGExport *export = (__bridge PVGExport *)opaque;
   rmf_pub_cancel(export->publisher, generation, slot);
}

// Frame messages readers were sent (a frame to two readers is two), and those that could not be
// sent (a reader's queue was full or it had gone), as rmframes counts them.
void
pvg_export_counts(void *opaque, uint64_t *sent, uint64_t *dropped)
{
   PVGExport *export = (__bridge PVGExport *)opaque;
   rmf_pub_counts(export->publisher, sent, dropped);
}

// How many readers there are now.
uint32_t
pvg_export_readers(void *opaque)
{
   PVGExport *export = (__bridge PVGExport *)opaque;
   return rmf_pub_readers(export->publisher);
}

// Gives the name up and lets the slots go. Readers are told the publisher has gone.
void
pvg_export_close(void *opaque)
{
   @autoreleasepool {
      PVGExport *export = (__bridge_transfer PVGExport *)opaque;
      (void)export;
   }
}
