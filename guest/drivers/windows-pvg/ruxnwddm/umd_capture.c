// SPDX-License-Identifier: MIT
//
// Lab capture of the shaders the Direct3D runtime hands ruxnumd.dll, so
// that a run shows what the compositor would ask the shader compiler for:
// each vertex, geometry and pixel shader's tokens and its input and output
// signatures go to one file in C:\ruxnlog (made, writable by everyone, by
// wddm.ps1), named by a hash of the tokens. Nothing is written if the
// folder does not exist. The format, read by a script on the Mac:
//
//   "RXSH", the stage (0 vertex, 1 geometry, 2 pixel), the token count,
//   the input entries' count and the output entries' count (five 32-bit
//   words), then the tokens, then the input and the output signature
//   entries as the runtime laid them out.

#include "umd.h"

#define CAPTURE_MAGIC	0x48535852u	// "RXSH"
#define STAGE_VERTEX	0
#define STAGE_GEOMETRY	1
#define STAGE_PIXEL	2

static ULONG64 fnv1a(const void *data, SIZE_T bytes)
{
	const UCHAR *p = data;
	ULONG64 h = 0xcbf29ce484222325ull;

	while (bytes--)
		h = (h ^ *p++) * 0x100000001b3ull;
	return h;
}

static void write_all(HANDLE f, const void *data, SIZE_T bytes)
{
	DWORD written;

	if (data && bytes)
		WriteFile(f, data, (DWORD)bytes, &written, NULL);
}

static void capture(UINT stage, const UINT *code, const D3D11_1DDIARG_STAGE_IO_SIGNATURES *sig)
{
	static const WCHAR hex[] = L"0123456789abcdef";
	WCHAR path[] = L"C:\\ruxnlog\\sh-0000000000000000.bin";
	UINT count = code ? code[1] : 0;
	UINT in = sig ? sig->NumInputSignatureEntries : 0;
	UINT out = sig ? sig->NumOutputSignatureEntries : 0;
	UINT head[5];
	ULONG64 h;
	HANDLE f;
	int i;

	if (!count)
		return;
	h = fnv1a(code, (SIZE_T)count * sizeof(UINT));
	for (i = 0; i < 16; i++)
		path[14 + i] = hex[(h >> (60 - 4 * i)) & 0xf];
	f = CreateFileW(path, GENERIC_WRITE, 0, NULL, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, NULL);
	if (f == INVALID_HANDLE_VALUE)
		return;		// no folder, or captured already
	head[0] = CAPTURE_MAGIC;
	head[1] = stage;
	head[2] = count;
	head[3] = in;
	head[4] = out;
	write_all(f, head, sizeof(head));
	write_all(f, code, (SIZE_T)count * sizeof(UINT));
	write_all(f, in ? sig->pInputSignature : NULL, (SIZE_T)in * sizeof(*sig->pInputSignature));
	write_all(f, out ? sig->pOutputSignature : NULL, (SIZE_T)out * sizeof(*sig->pOutputSignature));
	CloseHandle(f);
}

void APIENTRY umd_capture_vertex(D3D10DDI_HDEVICE h, const UINT *code, D3D10DDI_HSHADER s,
				 D3D10DDI_HRTSHADER rt, const D3D11_1DDIARG_STAGE_IO_SIGNATURES *sig)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(s);
	UNREFERENCED_PARAMETER(rt);
	capture(STAGE_VERTEX, code, sig);
}

void APIENTRY umd_capture_geometry(D3D10DDI_HDEVICE h, const UINT *code, D3D10DDI_HSHADER s,
				   D3D10DDI_HRTSHADER rt, const D3D11_1DDIARG_STAGE_IO_SIGNATURES *sig)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(s);
	UNREFERENCED_PARAMETER(rt);
	capture(STAGE_GEOMETRY, code, sig);
}

void APIENTRY umd_capture_pixel(D3D10DDI_HDEVICE h, const UINT *code, D3D10DDI_HSHADER s,
				D3D10DDI_HRTSHADER rt, const D3D11_1DDIARG_STAGE_IO_SIGNATURES *sig)
{
	UNREFERENCED_PARAMETER(h);
	UNREFERENCED_PARAMETER(s);
	UNREFERENCED_PARAMETER(rt);
	capture(STAGE_PIXEL, code, sig);
}
