// SPDX-License-Identifier: MIT
//
// The video device's table (item D). The runtime asks the device for it
// through the sub-object entry it is given at CreateDevice
// (ppfnRetrieveSubObject); with none, ID3D11VideoDevice could not be had
// and Edge logged "Failed to retrieve video device" in every run since
// rn48.
//
// This answers honestly that there is no video hardware: no decoder
// profile, and no video processor (its enumerator is refused), so callers
// take their software paths. Every way to make a video object is refused
// with E_NOTIMPL; the entries that only set state do nothing, as there is
// nothing they could be called on. A video processor that draws on the
// inner device is the next step, not this one.

#include "umd.h"

// The entries that only set or destroy state: the table's stubs (zero,
// nothing). The ones a caller can reach without a video object are below.

static void APIENTRY decoder_profile_count(D3D10DDI_HDEVICE device, UINT *count)
{
	(void)device;
	umd_log("video: decoder profiles asked", 0);
	*count = 0;
}

static void APIENTRY decoder_profile(D3D10DDI_HDEVICE device, UINT index, GUID *profile)
{
	(void)device;
	(void)index;
	umd_zero(profile, sizeof(*profile));
}

static void APIENTRY decoder_format(D3D10DDI_HDEVICE device, const GUID *profile, DXGI_FORMAT format,
				    BOOL *supported)
{
	(void)device;
	(void)profile;
	(void)format;
	*supported = FALSE;
}

static void APIENTRY decoder_config_count(D3D10DDI_HDEVICE device, const D3D11_1DDI_VIDEO_DECODER_DESC *desc,
					  UINT *count)
{
	(void)device;
	(void)desc;
	*count = 0;
}

static void APIENTRY decoder_buffer_type_count(D3D10DDI_HDEVICE device, const D3D11_1DDI_VIDEO_DECODER_DESC *desc,
					       UINT *count)
{
	(void)device;
	(void)desc;
	*count = 0;
}

static void APIENTRY capture_handle(D3D10DDI_HDEVICE device, D3D11_1DDI_GETCAPTUREHANDLEDATA *data)
{
	(void)device;
	(void)data;
}

static void APIENTRY query_video_caps(D3D10DDI_HDEVICE device, D3DWDDM2_0DDI_VIDEO_CAPABILITY_QUERY query,
				      UINT bytes, void *data)
{
	(void)device;
	(void)query;
	if (data)
		umd_zero(data, bytes);
}

// Every entry that makes a video object or answers a content protection
// question: there is none to make.
static HRESULT APIENTRY not_made(void)
{
	umd_log("video: an object refused", 0);
	return E_NOTIMPL;
}

#define REFUSE(table, entry) ((table)->entry = (void *)not_made)

static void fill_11_1(D3D11_1DDI_VIDEODEVICEFUNCS *t)
{
	t->pfnGetVideoDecoderProfileCount = decoder_profile_count;
	t->pfnGetVideoDecoderProfile = decoder_profile;
	t->pfnCheckVideoDecoderFormat = decoder_format;
	t->pfnGetVideoDecoderConfigCount = decoder_config_count;
	t->pfnGetVideoDecoderBufferTypeCount = decoder_buffer_type_count;
	t->pfnGetCaptureHandle = capture_handle;
	REFUSE(t, pfnCreateVideoDecoder);
	REFUSE(t, pfnCreateVideoProcessorEnum);
	REFUSE(t, pfnCreateVideoProcessor);
	REFUSE(t, pfnCreateVideoDecoderOutputView);
	REFUSE(t, pfnCreateVideoProcessorInputView);
	REFUSE(t, pfnCreateVideoProcessorOutputView);
	REFUSE(t, pfnGetContentProtectionCaps);
	REFUSE(t, pfnGetCryptoKeyExchangeType);
	REFUSE(t, pfnCreateCryptoSession);
	REFUSE(t, pfnCreateAuthenticatedChannel);
}

// The sub-object entry: the video tables of 11.1 and of WDDM 2.0, and
// nothing else.
static HRESULT APIENTRY retrieve_sub_object(D3D10DDI_HDEVICE device, UINT32 id, SIZE_T in_bytes, void *in,
					    SIZE_T out_bytes, void *out)
{
	(void)device;
	(void)out_bytes;
	(void)out;
	umd_log("video: sub-object asked", id);
	if (id == D3D11_1DDI_VIDEO_FUNCTIONS && in && in_bytes >= sizeof(D3D11_1DDI_VIDEO_INPUT)) {
		D3D11_1DDI_VIDEODEVICEFUNCS *t = ((D3D11_1DDI_VIDEO_INPUT *)in)->p11VideoDeviceFuncs;

		if (!t)
			return E_INVALIDARG;
		umd_fill_stubs(t, sizeof(*t), UMD_VIDEO_FIRST_SLOT);
		fill_11_1(t);
		return S_OK;
	}
	if (id == D3DWDDM2_0DDI_VIDEO_FUNCTIONS && in && in_bytes >= sizeof(D3DWDDM2_0DDI_VIDEO_INPUT)) {
		D3DWDDM2_0DDI_VIDEODEVICEFUNCS *t = ((D3DWDDM2_0DDI_VIDEO_INPUT *)in)->pWDDM2_0VideoDeviceFuncs;

		if (!t)
			return E_INVALIDARG;
		umd_fill_stubs(t, sizeof(*t), UMD_VIDEO_FIRST_SLOT);
		// The 2.0 table begins with the 11.1 table's entries.
		fill_11_1((D3D11_1DDI_VIDEODEVICEFUNCS *)t);
		t->pfnQueryVideoCapabilities = query_video_caps;
		return S_OK;
	}
	return E_NOTIMPL;
}

// At CreateDevice: where the runtime asks for the video tables.
void umd_video_init(D3D10DDIARG_CREATEDEVICE *args)
{
	if (args->ppfnRetrieveSubObject)
		*args->ppfnRetrieveSubObject = retrieve_sub_object;
}
