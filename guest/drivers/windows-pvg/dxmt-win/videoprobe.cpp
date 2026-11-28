// SPDX-License-Identifier: MIT
//
// videoprobe: what a program finds of Direct3D 11 video on the default
// adapter (item D). Our user-mode driver answers the runtime's request for
// the video table honestly: no decoder profiles and no video processor.
// Prints one line a question, then "videoprobe: done".
//
// Pass on our adapter: the video device and context are had (S_OK), 0
// decoder profiles, and the processor enumerator refused with an error, not
// a crash. Usage: videoprobe

#include <windows.h>
#include <d3d11.h>
#include <stdio.h>

int main(void)
{
	ID3D11Device *device = NULL;
	ID3D11DeviceContext *context = NULL;
	ID3D11VideoDevice *video = NULL;
	ID3D11VideoContext *video_context = NULL;
	ID3D11VideoProcessorEnumerator *processors = NULL;
	D3D11_VIDEO_PROCESSOR_CONTENT_DESC desc;
	D3D_FEATURE_LEVEL level;
	HRESULT hr;

	hr = D3D11CreateDevice(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, NULL, 0,
			       D3D11_SDK_VERSION, &device, &level, &context);
	printf("D3D11CreateDevice with video support: 0x%08lx, level 0x%x\n", (unsigned long)hr, (unsigned)level);
	if (FAILED(hr)) {
		hr = D3D11CreateDevice(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, NULL, 0, D3D11_SDK_VERSION, &device,
				       &level, &context);
		printf("D3D11CreateDevice without: 0x%08lx\n", (unsigned long)hr);
		if (FAILED(hr))
			return 1;
	}
	hr = device->QueryInterface(__uuidof(ID3D11VideoDevice), (void **)&video);
	printf("ID3D11VideoDevice: 0x%08lx\n", (unsigned long)hr);
	if (SUCCEEDED(hr)) {
		printf("decoder profiles: %u\n", video->GetVideoDecoderProfileCount());
		ZeroMemory(&desc, sizeof(desc));
		desc.InputFrameFormat = D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE;
		desc.InputWidth = 640;
		desc.InputHeight = 360;
		desc.OutputWidth = 1280;
		desc.OutputHeight = 720;
		desc.Usage = D3D11_VIDEO_USAGE_PLAYBACK_NORMAL;
		hr = video->CreateVideoProcessorEnumerator(&desc, &processors);
		printf("CreateVideoProcessorEnumerator: 0x%08lx\n", (unsigned long)hr);
		if (processors)
			processors->Release();
		video->Release();
	}
	hr = context->QueryInterface(__uuidof(ID3D11VideoContext), (void **)&video_context);
	printf("ID3D11VideoContext: 0x%08lx\n", (unsigned long)hr);
	if (video_context)
		video_context->Release();
	context->Release();
	device->Release();
	printf("videoprobe: done\n");
	return 0;
}
