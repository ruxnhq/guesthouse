// The one GPU pass of each frame: Windows' surface in, a pitch-linear slot of the ring out, with the transform Windows asked for
// and the hardware cursor, if there is one, applied on the way.
//
// The slot is a raw buffer because the daemon can only export pitch-linear memory, and a buffer is never tiled.

#include "ColourMath.hlsli"

static const uint GROUP_SIZE = 8;
// The copy layout: each thread moves this many pixels of a row at a time, so a group covers COPY_SPAN pixels of each of its
// rows.
static const uint COPY_PIXELS = 4;
static const uint COPY_SPAN = GROUP_SIZE * GROUP_SIZE * COPY_PIXELS;
static const uint LUT_ENTRIES = 4096;

cbuffer PassInputs : register(b0)
{
	uint width;
	uint height;
	uint pitch;
	uint source;

	uint wire;
	uint lutEnabled;
	uint cursorEnabled;
	float sdrWhite; // The SDR white level over 80 nits.

	int cursorX;
	int cursorY;
	uint cursorWidth;
	uint cursorHeight;

	float4 sourceToWire[3];
	float4 cursorToWire[3];

	// Set when the pass would hand each pixel on as it came: an sRGB surface, an SDR wire, and a transform from Windows that
	// changes nothing.
	uint unchanged;
	// Nonzero picks the copy layout: each group moves COPY_SPAN pixels of copyRows rows, with no colour work and no cursor. Only
	// for a frame with unchanged set. The cursor then gets a second, small run of the normal layout over its rectangle.
	uint copyRows;
	// Where the normal layout's first thread sits on the screen. Zero but for the cursor's run.
	uint originX;
	uint originY;
};

Texture2D<float4> surface : register(t0);
Texture2D<float4> cursor : register(t1);
Buffer<float4> lut : register(t2);
RWByteAddressBuffer slot : register(u0);

float LutChannel(float encoded, uint channel)
{
	float position = saturate(encoded) * (LUT_ENTRIES - 1);
	uint below = (uint)position;
	uint above = min(below + 1, LUT_ENTRIES - 1);
	return lerp(lut[below][channel], lut[above][channel], position - below);
}

bool UnderCursor(uint2 pixel)
{
	int2 inside = int2(pixel) - int2(cursorX, cursorY);
	return cursorEnabled != 0 && inside.x >= 0 && inside.y >= 0 && inside.x < (int)cursorWidth && inside.y < (int)cursorHeight;
}

float3 OverCursor(uint2 pixel, float3 colour)
{
	int2 inside = int2(pixel) - int2(cursorX, cursorY);
	if (inside.x < 0 || inside.y < 0 || inside.x >= (int)cursorWidth || inside.y >= (int)cursorHeight)
	{
		return colour;
	}

	// Cursor pixels are always SDR, so they go through the same door as an SDR surface.
	float4 shape = cursor.Load(int3(inside, 0));
	float3 light = DecodeSource(SOURCE_SRGB, shape.rgb, sdrWhite);
	light = ApplyRows(cursorToWire[0], cursorToWire[1], cursorToWire[2], light);
	return lerp(colour, light, shape.a);
}

// Most of a plain pass's cost is starting a group for every 64 pixels, not the pixels' work, so this layout starts one for
// every COPY_SPAN * copyRows. Each store is 16 bytes. A row's last store can run past the width, but never past the pitch,
// which is a multiple of 256 bytes; past the width, Load gives zero, so the row's padding gets black.
void Copy(uint2 group, uint index)
{
	uint column = (group.x * GROUP_SIZE * GROUP_SIZE + index) * COPY_PIXELS;
	if (column >= width)
	{
		return;
	}

	uint top = group.y * copyRows;
	uint bottom = min(top + copyRows, height);
	[loop]
	for (uint y = top; y < bottom; ++y)
	{
		uint4 packed;
		packed.x = PackXrgb8888(surface.Load(int3(column, y, 0)).rgb);
		packed.y = PackXrgb8888(surface.Load(int3(column + 1, y, 0)).rgb);
		packed.z = PackXrgb8888(surface.Load(int3(column + 2, y, 0)).rgb);
		packed.w = PackXrgb8888(surface.Load(int3(column + 3, y, 0)).rgb);
		slot.Store4(y * pitch + column * 4, packed);
	}
}

[numthreads(GROUP_SIZE, GROUP_SIZE, 1)]
void main(uint3 id : SV_DispatchThreadID, uint3 group : SV_GroupID, uint index : SV_GroupIndex)
{
	if (copyRows != 0)
	{
		Copy(group.xy, index);
		return;
	}

	uint2 pixel = id.xy + uint2(originX, originY);
	if (pixel.x >= width || pixel.y >= height)
	{
		return;
	}

	float3 texel = surface.Load(int3(pixel, 0)).rgb;
	if (unchanged != 0 && !UnderCursor(pixel))
	{
		slot.Store(pixel.y * pitch + pixel.x * 4, PackXrgb8888(texel));
		return;
	}

	float3 light = DecodeSource(source, texel, sdrWhite);
	light = ApplyRows(sourceToWire[0], sourceToWire[1], sourceToWire[2], light);
	if (cursorEnabled != 0)
	{
		light = OverCursor(pixel, light);
	}

	float3 encoded = EncodeWire(wire, light, sdrWhite);
	if (lutEnabled != 0)
	{
		encoded = float3(LutChannel(encoded.x, 0), LutChannel(encoded.y, 1), LutChannel(encoded.z, 2));
	}

	uint packed = wire == WIRE_HDR ? PackXbgr2101010(encoded) : PackXrgb8888(encoded);
	slot.Store(pixel.y * pitch + pixel.x * 4, packed);
}
