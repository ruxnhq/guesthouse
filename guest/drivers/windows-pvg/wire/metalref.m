// SPDX-License-Identifier: GPL-2.0
//
// metalref: the reference for pvgtri and pvgcube, run on the Mac host. It
// draws the same libraries with the host's own Metal, through its public
// interface only, and nothing here goes into the guest.
//
// For pvgtri: a 256 by 256 BGRA8 target cleared to 0.25, the vertex and
// fragment functions as a pipeline, three vertices, the target copied into a
// buffer, and the picture written as pvgtri writes it.
//
// For pvgcube (`cube`): the cube of tools/pvg-cube as mac/cube.m draws it,
// from the same scene.h, with a depth texture, a vertex buffer, constants, a
// picture and a sampler, but into a texture copied into a buffer as pvgcube
// reads it. The sine and cosine are cube_math.h's, which pvgcube uses too:
// the Mac's own sinf and cosf are not correctly rounded for every angle, and
// would give other matrices for some frames. `still` is angle 1.9, `frames N`
// the angle of pvgcube's frame N - 1, 0.02 a frame worked in float; each
// clears the picture and the depth, so only that frame is drawn.
//
// For pvgcomp (`comp`): a picture from a PPM file in a texture described as
// the shared surface's is, and pvgcomp's pass over a target of the same
// size: cleared to pvgcomp's colour, the quad drawn with the picture through
// a nearest sampler, the target copied into a buffer.
//
//   clang -fobjc-arc -framework Metal -framework Foundation metalref.m -o metalref
//   metalref VERTEX.metallib FRAGMENT.metallib OUT.ppm
//   metalref cube VERTEX.metallib FRAGMENT.metallib still OUT.ppm [WIDTH HEIGHT]
//   metalref cube VERTEX.metallib FRAGMENT.metallib frames N OUT.ppm [WIDTH HEIGHT]
//   metalref comp VERTEX.metallib FRAGMENT.metallib IN.ppm OUT.ppm

#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// pvgcube's sine, cosine and tangent, in place of the library's in scene.h.
#define CUBE_MATH_NAME(name) cube_##name##f
#include "cube_math.h"
#define sinf cube_sinf
#define cosf cube_cosf
#define tanf cube_tanf
#include "scene.h"
#undef sinf
#undef cosf
#undef tanf

#define W 256
#define H 256
#define CLEAR 0x40

static id<MTLFunction> function(id<MTLDevice> device, const char *path)
{
	NSError *error = nil;
	NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:path]];
	id<MTLLibrary> library = [device newLibraryWithURL:url error:&error];

	if (!library) {
		fprintf(stderr, "metalref: %s: %s\n", path, error.description.UTF8String);
		return nil;
	}
	// airc names the one function of a library main0.
	return [library newFunctionWithName:library.functionNames.firstObject];
}

// As pvgcube hashes a frame's constants: FNV-1a.
static unsigned constants_hash(const struct cube_constants *constants)
{
	const unsigned char *byte = (const unsigned char *)constants;
	unsigned hash = 2166136261u;

	for (size_t i = 0; i < sizeof(*constants); i++)
		hash = (hash ^ byte[i]) * 16777619u;
	return hash;
}

static int cube_main(int argc, char **argv)
{
	if (argc < 6) {
		fprintf(stderr, "usage: metalref cube VERTEX FRAGMENT still OUT.ppm [W H]\n"
				"       metalref cube VERTEX FRAGMENT frames N OUT.ppm [W H]\n");
		return 2;
	}
	BOOL frames = !strcmp(argv[4], "frames");
	int size_at = frames ? 7 : 6;
	const char *out_path = argv[frames ? 6 : 5];
	unsigned width = 640, height = 480;
	float angle = 1.9f;

	if (frames && argc < 7)
		return 2;
	if (frames)
		angle = (float)(atoi(argv[5]) - 1) * 0.02f;
	if (argc >= size_at + 2) {
		width = (unsigned)atoi(argv[size_at]);
		height = (unsigned)atoi(argv[size_at + 1]);
	}

	id<MTLDevice> device = MTLCreateSystemDefaultDevice();
	id<MTLFunction> vertex = function(device, argv[2]);
	id<MTLFunction> fragment = function(device, argv[3]);
	if (!device || !vertex || !fragment)
		return 1;
	NSError *error = nil;

	// As cube_describe_pipeline: the corners from buffer 1, the constants
	// at buffer 0 of both functions.
	MTLRenderPipelineDescriptor *descriptor = [MTLRenderPipelineDescriptor new];
	descriptor.vertexFunction = vertex;
	descriptor.fragmentFunction = fragment;
	descriptor.colorAttachments[0].pixelFormat = MTLPixelFormatBGRA8Unorm;
	descriptor.depthAttachmentPixelFormat = MTLPixelFormatDepth32Float;
	MTLVertexDescriptor *layout = [MTLVertexDescriptor new];
	layout.attributes[0].format = MTLVertexFormatFloat3;
	layout.attributes[0].bufferIndex = 1;
	layout.attributes[1].format = MTLVertexFormatFloat3;
	layout.attributes[1].offset = 12;
	layout.attributes[1].bufferIndex = 1;
	layout.attributes[2].format = MTLVertexFormatFloat2;
	layout.attributes[2].offset = 24;
	layout.attributes[2].bufferIndex = 1;
	layout.layouts[1].stride = CUBE_CORNER_BYTES;
	descriptor.vertexDescriptor = layout;
	id<MTLRenderPipelineState> pipeline =
		[device newRenderPipelineStateWithDescriptor:descriptor error:&error];
	if (!pipeline) {
		fprintf(stderr, "metalref: pipeline: %s\n", error.description.UTF8String);
		return 1;
	}

	MTLDepthStencilDescriptor *depth_descriptor = [MTLDepthStencilDescriptor new];
	depth_descriptor.depthCompareFunction = MTLCompareFunctionLess;
	depth_descriptor.depthWriteEnabled = YES;
	id<MTLDepthStencilState> nearer = [device newDepthStencilStateWithDescriptor:depth_descriptor];
	MTLSamplerDescriptor *sampler_descriptor = [MTLSamplerDescriptor new];
	sampler_descriptor.minFilter = MTLSamplerMinMagFilterLinear;
	sampler_descriptor.magFilter = MTLSamplerMinMagFilterLinear;
	id<MTLSamplerState> sampler = [device newSamplerStateWithDescriptor:sampler_descriptor];

	static float corner_data[CUBE_CORNERS * 8];
	static uint16_t index_data[CUBE_INDICES];
	static uint32_t picture_data[CUBE_PICTURE_SIZE * CUBE_PICTURE_SIZE];
	struct cube_constants constant_data;
	cube_corners(corner_data);
	cube_indices(index_data);
	cube_picture(picture_data);
	cube_constants_at(&constant_data, angle, (float)width / (float)height);
	printf("metalref: angle %.9g, constants hash %08x\n", angle, constants_hash(&constant_data));
	id<MTLBuffer> corners = [device newBufferWithBytes:corner_data length:sizeof(corner_data)
						   options:MTLResourceStorageModeShared];
	id<MTLBuffer> indices = [device newBufferWithBytes:index_data length:sizeof(index_data)
						   options:MTLResourceStorageModeShared];
	id<MTLBuffer> constants = [device newBufferWithBytes:&constant_data length:sizeof(constant_data)
						     options:MTLResourceStorageModeShared];

	MTLTextureDescriptor *picture_descriptor =
		[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
								   width:CUBE_PICTURE_SIZE
								  height:CUBE_PICTURE_SIZE
							       mipmapped:NO];
	id<MTLTexture> picture = [device newTextureWithDescriptor:picture_descriptor];
	[picture replaceRegion:MTLRegionMake2D(0, 0, CUBE_PICTURE_SIZE, CUBE_PICTURE_SIZE)
		   mipmapLevel:0
		     withBytes:picture_data
		   bytesPerRow:CUBE_PICTURE_SIZE * 4];

	MTLTextureDescriptor *target_descriptor =
		[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
								   width:width
								  height:height
							       mipmapped:NO];
	target_descriptor.usage = MTLTextureUsageShaderRead | MTLTextureUsageRenderTarget;
	target_descriptor.storageMode = MTLStorageModePrivate;
	id<MTLTexture> target = [device newTextureWithDescriptor:target_descriptor];
	MTLTextureDescriptor *far_descriptor =
		[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatDepth32Float
								   width:width
								  height:height
							       mipmapped:NO];
	far_descriptor.usage = MTLTextureUsageRenderTarget;
	far_descriptor.storageMode = MTLStorageModePrivate;
	id<MTLTexture> far_texture = [device newTextureWithDescriptor:far_descriptor];
	id<MTLBuffer> readback = [device newBufferWithLength:(NSUInteger)width * height * 4
						     options:MTLResourceStorageModeShared];

	id<MTLCommandQueue> queue = [device newCommandQueue];
	id<MTLCommandBuffer> commands = [queue commandBuffer];
	MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
	pass.colorAttachments[0].texture = target;
	pass.colorAttachments[0].loadAction = MTLLoadActionClear;
	pass.colorAttachments[0].storeAction = MTLStoreActionStore;
	pass.colorAttachments[0].clearColor = MTLClearColorMake(0.1, 0.1, 0.1, 1.0);
	pass.depthAttachment.texture = far_texture;
	pass.depthAttachment.loadAction = MTLLoadActionClear;
	pass.depthAttachment.clearDepth = 1.0;
	id<MTLRenderCommandEncoder> render = [commands renderCommandEncoderWithDescriptor:pass];
	[render setRenderPipelineState:pipeline];
	[render setDepthStencilState:nearer];
	[render setVertexBuffer:constants offset:0 atIndex:0];
	[render setVertexBuffer:corners offset:0 atIndex:1];
	[render setFragmentBuffer:constants offset:0 atIndex:0];
	[render setFragmentTexture:picture atIndex:0];
	[render setFragmentSamplerState:sampler atIndex:0];
	[render drawIndexedPrimitives:MTLPrimitiveTypeTriangle
			   indexCount:CUBE_INDICES
			    indexType:MTLIndexTypeUInt16
			  indexBuffer:indices
		    indexBufferOffset:0];
	[render endEncoding];
	id<MTLBlitCommandEncoder> blit = [commands blitCommandEncoder];
	[blit copyFromTexture:target
		  sourceSlice:0
		  sourceLevel:0
		 sourceOrigin:MTLOriginMake(0, 0, 0)
		   sourceSize:MTLSizeMake(width, height, 1)
		     toBuffer:readback
	    destinationOffset:0
       destinationBytesPerRow:width * 4
     destinationBytesPerImage:(NSUInteger)width * height * 4];
	[blit endEncoding];
	[commands commit];
	[commands waitUntilCompleted];
	if (commands.error) {
		fprintf(stderr, "metalref: %s\n", commands.error.description.UTF8String);
		return 1;
	}

	const unsigned char *pixels = readback.contents;
	FILE *out = fopen(out_path, "wb");
	unsigned long drawn = 0;
	if (!out)
		return 1;
	fprintf(out, "P6\n%u %u\n255\n", width, height);
	for (unsigned long i = 0; i < (unsigned long)width * height; i++) {
		unsigned char rgb[3] = { pixels[4 * i + 2], pixels[4 * i + 1], pixels[4 * i] };
		drawn += (rgb[0] << 16 | rgb[1] << 8 | rgb[2]) != 0x1a1a1a;
		fwrite(rgb, 1, 3, out);
	}
	fclose(out);
	printf("metalref: %lu of %lu pixels are the cube's, written to %s\n", drawn,
	       (unsigned long)width * height, out_path);
	return drawn ? 0 : 1;
}

// A binary PPM of 255 levels with no comments, as the guest's programs and
// the guard write them; its pixels as BGRA words. NULL if it is not one.
static uint32_t *read_ppm(const char *path, unsigned *width, unsigned *height)
{
	FILE *in = fopen(path, "rb");
	unsigned levels = 0;
	uint32_t *pixels = NULL;

	if (!in)
		return NULL;
	if (fscanf(in, "P6 %u %u %u", width, height, &levels) == 3 && levels == 255 &&
	    fgetc(in) != EOF && *width && *height && *width <= 16384 && *height <= 16384) {
		size_t count = (size_t)*width * *height;

		pixels = malloc(count * 4);
		for (size_t i = 0; pixels && i < count; i++) {
			unsigned char rgb[3];

			if (fread(rgb, 1, 3, in) != 3) {
				free(pixels);
				pixels = NULL;
				break;
			}
			pixels[i] = 0xff000000u | (uint32_t)rgb[0] << 16 | (uint32_t)rgb[1] << 8 | rgb[2];
		}
	}
	fclose(in);
	return pixels;
}

// pvgcomp's clear colour, as it writes it.
#define COMP_CLEAR_RED 0.2
#define COMP_CLEAR_GREEN 0.0
#define COMP_CLEAR_BLUE 0.4

static int comp_main(int argc, char **argv)
{
	unsigned width, height;

	if (argc != 6) {
		fprintf(stderr, "usage: metalref comp VERTEX FRAGMENT IN.ppm OUT.ppm\n");
		return 2;
	}
	uint32_t *source = read_ppm(argv[4], &width, &height);
	if (!source) {
		fprintf(stderr, "metalref: %s: not a PPM this reads\n", argv[4]);
		return 1;
	}
	id<MTLDevice> device = MTLCreateSystemDefaultDevice();
	id<MTLFunction> vertex = function(device, argv[2]);
	id<MTLFunction> fragment = function(device, argv[3]);
	if (!device || !vertex || !fragment)
		return 1;
	NSError *error = nil;

	// As pvgcomp's pipeline: no vertex buffer, one BGRA8 colour, no depth.
	MTLRenderPipelineDescriptor *descriptor = [MTLRenderPipelineDescriptor new];
	descriptor.vertexFunction = vertex;
	descriptor.fragmentFunction = fragment;
	descriptor.colorAttachments[0].pixelFormat = MTLPixelFormatBGRA8Unorm;
	id<MTLRenderPipelineState> pipeline =
		[device newRenderPipelineStateWithDescriptor:descriptor error:&error];
	if (!pipeline) {
		fprintf(stderr, "metalref: pipeline: %s\n", error.description.UTF8String);
		return 1;
	}
	// The wire's sampler as pvgcomp describes it: nearest, clamped, the
	// coordinates normalised.
	MTLSamplerDescriptor *sampler_descriptor = [MTLSamplerDescriptor new];
	sampler_descriptor.minFilter = MTLSamplerMinMagFilterNearest;
	sampler_descriptor.magFilter = MTLSamplerMinMagFilterNearest;
	id<MTLSamplerState> sampler = [device newSamplerStateWithDescriptor:sampler_descriptor];

	// The shared surface's texture: BGRA8, a render target the shaders
	// may read, in shared storage.
	MTLTextureDescriptor *picture_descriptor =
		[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
								   width:width
								  height:height
							       mipmapped:NO];
	picture_descriptor.usage = MTLTextureUsageShaderRead | MTLTextureUsageRenderTarget;
	picture_descriptor.storageMode = MTLStorageModeShared;
	id<MTLTexture> picture = [device newTextureWithDescriptor:picture_descriptor];
	[picture replaceRegion:MTLRegionMake2D(0, 0, width, height)
		   mipmapLevel:0
		     withBytes:source
		   bytesPerRow:width * 4];
	free(source);

	id<MTLTexture> target = [device newTextureWithDescriptor:picture_descriptor];
	id<MTLBuffer> readback = [device newBufferWithLength:(NSUInteger)width * height * 4
						     options:MTLResourceStorageModeShared];
	id<MTLCommandQueue> queue = [device newCommandQueue];
	id<MTLCommandBuffer> commands = [queue commandBuffer];
	MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
	pass.colorAttachments[0].texture = target;
	pass.colorAttachments[0].loadAction = MTLLoadActionClear;
	pass.colorAttachments[0].storeAction = MTLStoreActionStore;
	pass.colorAttachments[0].clearColor =
		MTLClearColorMake(COMP_CLEAR_RED, COMP_CLEAR_GREEN, COMP_CLEAR_BLUE, 1.0);
	id<MTLRenderCommandEncoder> render = [commands renderCommandEncoderWithDescriptor:pass];
	[render setRenderPipelineState:pipeline];
	[render setFragmentTexture:picture atIndex:0];
	[render setFragmentSamplerState:sampler atIndex:0];
	[render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:6];
	[render endEncoding];
	id<MTLBlitCommandEncoder> blit = [commands blitCommandEncoder];
	[blit copyFromTexture:target
		  sourceSlice:0
		  sourceLevel:0
		 sourceOrigin:MTLOriginMake(0, 0, 0)
		   sourceSize:MTLSizeMake(width, height, 1)
		     toBuffer:readback
	    destinationOffset:0
       destinationBytesPerRow:width * 4
     destinationBytesPerImage:(NSUInteger)width * height * 4];
	[blit endEncoding];
	[commands commit];
	[commands waitUntilCompleted];
	if (commands.error) {
		fprintf(stderr, "metalref: %s\n", commands.error.description.UTF8String);
		return 1;
	}

	const unsigned char *pixels = readback.contents;
	FILE *out = fopen(argv[5], "wb");
	if (!out)
		return 1;
	fprintf(out, "P6\n%u %u\n255\n", width, height);
	for (unsigned long i = 0; i < (unsigned long)width * height; i++) {
		unsigned char rgb[3] = { pixels[4 * i + 2], pixels[4 * i + 1], pixels[4 * i] };
		fwrite(rgb, 1, 3, out);
	}
	fclose(out);
	printf("metalref: %ux%u composed, written to %s\n", width, height, argv[5]);
	return 0;
}

int main(int argc, char **argv)
{
	@autoreleasepool {
		if (argc > 1 && !strcmp(argv[1], "cube"))
			return cube_main(argc, argv);
		if (argc > 1 && !strcmp(argv[1], "comp"))
			return comp_main(argc, argv);
		if (argc != 4) {
			fprintf(stderr, "usage: metalref VERTEX.metallib FRAGMENT.metallib OUT.ppm\n"
					"       metalref cube VERTEX FRAGMENT still|frames ...\n");
			return 2;
		}
		id<MTLDevice> device = MTLCreateSystemDefaultDevice();
		id<MTLFunction> vertex = function(device, argv[1]);
		id<MTLFunction> fragment = function(device, argv[2]);
		if (!device || !vertex || !fragment)
			return 1;

		MTLRenderPipelineDescriptor *pipeline_desc = [MTLRenderPipelineDescriptor new];
		pipeline_desc.vertexFunction = vertex;
		pipeline_desc.fragmentFunction = fragment;
		pipeline_desc.colorAttachments[0].pixelFormat = MTLPixelFormatBGRA8Unorm;
		NSError *error = nil;
		id<MTLRenderPipelineState> pipeline =
			[device newRenderPipelineStateWithDescriptor:pipeline_desc error:&error];
		if (!pipeline) {
			fprintf(stderr, "metalref: pipeline: %s\n", error.description.UTF8String);
			return 1;
		}

		MTLTextureDescriptor *texture_desc =
			[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
									  width:W
									 height:H
								      mipmapped:NO];
		texture_desc.usage = MTLTextureUsageShaderRead | MTLTextureUsageRenderTarget;
		texture_desc.storageMode = MTLStorageModePrivate;
		id<MTLTexture> target = [device newTextureWithDescriptor:texture_desc];
		id<MTLBuffer> readback = [device newBufferWithLength:W * H * 4
							      options:MTLResourceStorageModeShared];

		id<MTLCommandQueue> queue = [device newCommandQueue];
		id<MTLCommandBuffer> commands = [queue commandBuffer];
		MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
		pass.colorAttachments[0].texture = target;
		pass.colorAttachments[0].loadAction = MTLLoadActionClear;
		pass.colorAttachments[0].storeAction = MTLStoreActionStore;
		pass.colorAttachments[0].clearColor = MTLClearColorMake(0.25, 0.25, 0.25, 1.0);
		id<MTLRenderCommandEncoder> render = [commands renderCommandEncoderWithDescriptor:pass];
		[render setRenderPipelineState:pipeline];
		[render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
		[render endEncoding];
		id<MTLBlitCommandEncoder> blit = [commands blitCommandEncoder];
		[blit copyFromTexture:target
			  sourceSlice:0
			  sourceLevel:0
			 sourceOrigin:MTLOriginMake(0, 0, 0)
			   sourceSize:MTLSizeMake(W, H, 1)
			     toBuffer:readback
		    destinationOffset:0
	       destinationBytesPerRow:W * 4
	     destinationBytesPerImage:W * H * 4];
		[blit endEncoding];
		[commands commit];
		[commands waitUntilCompleted];
		if (commands.error) {
			fprintf(stderr, "metalref: %s\n", commands.error.description.UTF8String);
			return 1;
		}

		const unsigned char *pixels = readback.contents;
		FILE *out = fopen(argv[3], "wb");
		unsigned lit = 0;
		if (!out)
			return 1;
		fprintf(out, "P6\n%d %d\n255\n", W, H);
		for (unsigned i = 0; i < W * H; i++) {
			unsigned char rgb[3] = { pixels[4 * i + 2], pixels[4 * i + 1], pixels[4 * i] };
			lit += rgb[0] != CLEAR || rgb[1] != CLEAR || rgb[2] != CLEAR;
			fwrite(rgb, 1, 3, out);
		}
		fclose(out);
		printf("metalref: %u of %u pixels not the clear colour, written to %s\n", lit, W * H,
		       argv[3]);
		return lit ? 0 : 1;
	}
}
