// SPDX-License-Identifier: MIT
//
// What ruxnumd.dll says each format can do (CheckFormatSupport). The
// runtime checks these answers against what a feature level 10.1 device
// must and may do with each format, so they have to be plausible, not
// generous: run rm13's devices were refused after CreateDevice while every
// format claimed every capability, including "not supported" itself.
//
// The answers are those of an ordinary 10.1 device: colour formats sample,
// render and blend; integer formats render and feed vertex and typed
// buffers but are not filtered; block-compressed formats only sample;
// depth and typeless formats are left to the runtime; the BGRA formats
// the desktop uses can also be shown.

#include "umd.h"

#define CAN_SAMPLE	D3D10_DDI_FORMAT_SUPPORT_SHADER_SAMPLE
#define CAN_RENDER	(D3D10_DDI_FORMAT_SUPPORT_RENDERTARGET | D3D10_DDI_FORMAT_SUPPORT_BLENDABLE)
#define CAN_BUFFER	(D3D11_1DDI_FORMAT_SUPPORT_VERTEX_BUFFER | D3D11_1DDI_FORMAT_SUPPORT_BUFFER)
#define COLOUR		(CAN_SAMPLE | CAN_RENDER | CAN_BUFFER)
#define INTEGER		(D3D10_DDI_FORMAT_SUPPORT_RENDERTARGET | CAN_BUFFER)
#define SHOWN		(COLOUR | D3D11_1DDI_FORMAT_SUPPORT_DISPLAY)

static UINT support_of(DXGI_FORMAT format)
{
	switch (format) {
	case DXGI_FORMAT_B8G8R8A8_UNORM:
	case DXGI_FORMAT_B8G8R8X8_UNORM:
	case DXGI_FORMAT_B8G8R8A8_UNORM_SRGB:
	case DXGI_FORMAT_B8G8R8X8_UNORM_SRGB:
	case DXGI_FORMAT_R8G8B8A8_UNORM:
	case DXGI_FORMAT_R8G8B8A8_UNORM_SRGB:
	case DXGI_FORMAT_R10G10B10A2_UNORM:
	case DXGI_FORMAT_R16G16B16A16_FLOAT:
		return SHOWN;

	case DXGI_FORMAT_R32G32B32A32_FLOAT:
	case DXGI_FORMAT_R16G16B16A16_UNORM:
	case DXGI_FORMAT_R16G16B16A16_SNORM:
	case DXGI_FORMAT_R32G32_FLOAT:
	case DXGI_FORMAT_R11G11B10_FLOAT:
	case DXGI_FORMAT_R8G8B8A8_SNORM:
	case DXGI_FORMAT_R16G16_FLOAT:
	case DXGI_FORMAT_R16G16_UNORM:
	case DXGI_FORMAT_R16G16_SNORM:
	case DXGI_FORMAT_R32_FLOAT:
	case DXGI_FORMAT_R8G8_UNORM:
	case DXGI_FORMAT_R8G8_SNORM:
	case DXGI_FORMAT_R16_FLOAT:
	case DXGI_FORMAT_R16_UNORM:
	case DXGI_FORMAT_R16_SNORM:
	case DXGI_FORMAT_R8_UNORM:
	case DXGI_FORMAT_R8_SNORM:
	case DXGI_FORMAT_A8_UNORM:
	case DXGI_FORMAT_B5G6R5_UNORM:
	case DXGI_FORMAT_B5G5R5A1_UNORM:
		return COLOUR;

	case DXGI_FORMAT_R32G32B32_FLOAT:
		return CAN_SAMPLE | CAN_BUFFER;
	case DXGI_FORMAT_R9G9B9E5_SHAREDEXP:
		return CAN_SAMPLE;

	case DXGI_FORMAT_R32G32B32A32_UINT:
	case DXGI_FORMAT_R32G32B32A32_SINT:
	case DXGI_FORMAT_R16G16B16A16_UINT:
	case DXGI_FORMAT_R16G16B16A16_SINT:
	case DXGI_FORMAT_R32G32_UINT:
	case DXGI_FORMAT_R32G32_SINT:
	case DXGI_FORMAT_R10G10B10A2_UINT:
	case DXGI_FORMAT_R8G8B8A8_UINT:
	case DXGI_FORMAT_R8G8B8A8_SINT:
	case DXGI_FORMAT_R16G16_UINT:
	case DXGI_FORMAT_R16G16_SINT:
	case DXGI_FORMAT_R32_UINT:
	case DXGI_FORMAT_R32_SINT:
	case DXGI_FORMAT_R8G8_UINT:
	case DXGI_FORMAT_R8G8_SINT:
	case DXGI_FORMAT_R16_UINT:
	case DXGI_FORMAT_R16_SINT:
	case DXGI_FORMAT_R8_UINT:
	case DXGI_FORMAT_R8_SINT:
		return INTEGER;

	case DXGI_FORMAT_BC1_UNORM:
	case DXGI_FORMAT_BC1_UNORM_SRGB:
	case DXGI_FORMAT_BC2_UNORM:
	case DXGI_FORMAT_BC2_UNORM_SRGB:
	case DXGI_FORMAT_BC3_UNORM:
	case DXGI_FORMAT_BC3_UNORM_SRGB:
	case DXGI_FORMAT_BC4_UNORM:
	case DXGI_FORMAT_BC4_SNORM:
	case DXGI_FORMAT_BC5_UNORM:
	case DXGI_FORMAT_BC5_SNORM:
	case DXGI_FORMAT_BC6H_UF16:
	case DXGI_FORMAT_BC6H_SF16:
	case DXGI_FORMAT_BC7_UNORM:
	case DXGI_FORMAT_BC7_UNORM_SRGB:
		// Each drawn through the helper as Metal on the Mac draws the
		// same blocks (harness/kinds.sh, scenes bc1, bc3, bc-N); made by
		// winemetal from 0.3.63 with the levels of a whole block or more.
		return CAN_SAMPLE;

	// The one format the DDI says to refuse this way.
	case DXGI_FORMAT_R10G10B10_XR_BIAS_A2_UNORM:
		return D3D10_DDI_FORMAT_SUPPORT_NOT_SUPPORTED;

	default:
		return 0;
	}
}

// Each format's answer is logged the first time it is asked in a process
// (run rn48: Edge's GPU process stopped on a check of its own after asking
// questions nobody logged).
#define LOGGED_FORMATS	192

// The typeless format a format casts within, itself for one that casts
// with none: Direct3D's runtime refuses the device unless every format of
// a family, its typeless parent among them, answers the same multisample
// quality (event 1820, "MSAA quality for parent != child", run rn71).
static DXGI_FORMAT family(DXGI_FORMAT format)
{
	switch (format) {
	case DXGI_FORMAT_R8G8B8A8_UNORM: case DXGI_FORMAT_R8G8B8A8_UNORM_SRGB: case DXGI_FORMAT_R8G8B8A8_UINT:
	case DXGI_FORMAT_R8G8B8A8_SNORM: case DXGI_FORMAT_R8G8B8A8_SINT:
		return DXGI_FORMAT_R8G8B8A8_TYPELESS;
	case DXGI_FORMAT_B8G8R8A8_UNORM: case DXGI_FORMAT_B8G8R8A8_UNORM_SRGB:
		return DXGI_FORMAT_B8G8R8A8_TYPELESS;
	case DXGI_FORMAT_B8G8R8X8_UNORM: case DXGI_FORMAT_B8G8R8X8_UNORM_SRGB:
		return DXGI_FORMAT_B8G8R8X8_TYPELESS;
	case DXGI_FORMAT_R10G10B10A2_UNORM: case DXGI_FORMAT_R10G10B10A2_UINT:
		return DXGI_FORMAT_R10G10B10A2_TYPELESS;
	case DXGI_FORMAT_R16G16B16A16_FLOAT: case DXGI_FORMAT_R16G16B16A16_UNORM: case DXGI_FORMAT_R16G16B16A16_UINT:
	case DXGI_FORMAT_R16G16B16A16_SNORM: case DXGI_FORMAT_R16G16B16A16_SINT:
		return DXGI_FORMAT_R16G16B16A16_TYPELESS;
	case DXGI_FORMAT_R16G16_FLOAT: case DXGI_FORMAT_R16G16_UNORM: case DXGI_FORMAT_R16G16_UINT:
	case DXGI_FORMAT_R16G16_SNORM: case DXGI_FORMAT_R16G16_SINT:
		return DXGI_FORMAT_R16G16_TYPELESS;
	case DXGI_FORMAT_R8G8_UNORM: case DXGI_FORMAT_R8G8_UINT: case DXGI_FORMAT_R8G8_SNORM: case DXGI_FORMAT_R8G8_SINT:
		return DXGI_FORMAT_R8G8_TYPELESS;
	case DXGI_FORMAT_R8_UNORM: case DXGI_FORMAT_R8_UINT: case DXGI_FORMAT_R8_SNORM: case DXGI_FORMAT_R8_SINT:
		return DXGI_FORMAT_R8_TYPELESS;
	default:
		return format;
	}
}

// Formats drawn into at four samples a pixel: the colour families above
// and R11G11B10, whole. The device draws and resolves at 4 (helper scene
// msaa-small, against Metal's own picture); this Mac's GPU has no 8
// (Metal: "rasterSampleCount (8) is not supported by device"), and the
// families with a depth format (R32, R16, R24G8, R32G8X24) and the wider
// float ones wait for their own scenes.
BOOL umd_multisamples(DXGI_FORMAT format, UINT samples)
{
	switch (family(format)) {
	case DXGI_FORMAT_R8G8B8A8_TYPELESS:
	case DXGI_FORMAT_B8G8R8A8_TYPELESS:
	case DXGI_FORMAT_B8G8R8X8_TYPELESS:
	case DXGI_FORMAT_R10G10B10A2_TYPELESS:
	case DXGI_FORMAT_R16G16B16A16_TYPELESS:
	case DXGI_FORMAT_R16G16_TYPELESS:
	case DXGI_FORMAT_R8G8_TYPELESS:
	case DXGI_FORMAT_R8_TYPELESS:
	case DXGI_FORMAT_R11G11B10_FLOAT:
		return samples == 4;
	default:
		return FALSE;
	}
}

void APIENTRY umd_check_format_support(D3D10DDI_HDEVICE h, DXGI_FORMAT format, UINT *caps)
{
	static volatile LONG asked[LOGGED_FORMATS];

	UNREFERENCED_PARAMETER(h);
	*caps = support_of(format);
	if (umd_multisamples(format, 4) && (*caps & D3D10_DDI_FORMAT_SUPPORT_RENDERTARGET))
		*caps |= D3D10_DDI_FORMAT_SUPPORT_MULTISAMPLE_RENDERTARGET | D3D10_DDI_FORMAT_SUPPORT_MULTISAMPLE_LOAD;
	if ((UINT)format < LOGGED_FORMATS && !InterlockedExchange(&asked[format], 1))
		umd_log("CheckFormatSupport format, answer", (ULONG64)format << 32 | *caps);
}
