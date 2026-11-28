// SPDX-License-Identifier: MIT
//
// DXBC containers for the inner device. The Direct3D runtime hands a
// user-mode driver a shader's tokens and its input and output signatures
// as separate pieces; DXMT, like the Direct3D API, takes a DXBC container.
// This rebuilds one: an ISGN, an OSGN and the program chunk (SHDR for
// shader model 4, SHEX for 5), checksummed as fxc does, so that any reader
// may check it.
//
// The runtime's signatures carry system values and registers but no
// semantic names, so names are made from them: a system value gets its own
// name (SV_Position and so on), a pixel shader's output SV_Target with the
// register as index, and anything else TEXCOORD with the register as
// index. Stages are linked by register, so a vertex shader's output and the
// pixel shader's input of the same register get the same name. Input
// layouts are matched to vertex shaders by the same names (umd_shader.c).
//
// Triton (tritonDxbc.c, MIT, Copyright Turing Software LLC) rebuilds
// containers the same way for its inner device; this is a smaller one of
// our own, without its workarounds for another inner device.

#include "umd_inner.h"

#define FOURCC(a, b, c, d)	((UINT)(a) | (UINT)(b) << 8 | (UINT)(c) << 16 | (UINT)(d) << 24)
#define DXBC_HEAD_BYTES		32	// magic, checksum, version, size, chunk count
#define CHUNK_HEAD_BYTES	8
#define SIGNATURE_HEAD_BYTES	8
#define SIGNATURE_ELEMENT_BYTES	24
#define MOST_NAME_BYTES		28	// "SV_RenderTargetArrayIndex" and its end
#define TARGET_SYSTEM_VALUE	64	// D3D_NAME_TARGET, which signatures use

// The runtime leaves each element's component type unknown (run rn2: 0 in
// every entry, which airconv took as uint, so a pixel shader's colour came
// out as uint4 and the device refused the pipeline). Integer system values
// are uint; everything else is taken as float, which is what interpolated
// values and colours are.
static UINT component_type(const D3D11_1DDIARG_SIGNATURE_ENTRY2 *e)
{
	if (e->RegisterComponentType != D3D10_SB_REGISTER_COMPONENT_UNKNOWN)
		return (UINT)e->RegisterComponentType;
	switch (e->SystemValue) {
	case D3D10_SB_NAME_RENDER_TARGET_ARRAY_INDEX:
	case D3D10_SB_NAME_VIEWPORT_ARRAY_INDEX:
	case D3D10_SB_NAME_VERTEX_ID:
	case D3D10_SB_NAME_PRIMITIVE_ID:
	case D3D10_SB_NAME_INSTANCE_ID:
	case D3D10_SB_NAME_IS_FRONT_FACE:
	case D3D10_SB_NAME_SAMPLE_INDEX:
		return D3D10_SB_REGISTER_COMPONENT_UINT32;
	default:
		return D3D10_SB_REGISTER_COMPONENT_FLOAT32;
	}
}

// The names of D3D10_SB_NAME 1 to 10, which signatures share.
static const char *const system_names[] = {
	NULL, "SV_Position", "SV_ClipDistance", "SV_CullDistance", "SV_RenderTargetArrayIndex",
	"SV_ViewportArrayIndex", "SV_VertexID", "SV_PrimitiveID", "SV_InstanceID", "SV_IsFrontFace",
	"SV_SampleIndex",
};

static SIZE_T text_length(const char *s)
{
	SIZE_T n = 0;

	while (s[n])
		n++;
	return n;
}

static void put32(UCHAR *at, UINT value)
{
	at[0] = (UCHAR)value;
	at[1] = (UCHAR)(value >> 8);
	at[2] = (UCHAR)(value >> 16);
	at[3] = (UCHAR)(value >> 24);
}

// The name and index an element gets, and its system value as signatures
// write it.
static const char *element_name(const D3D11_1DDIARG_SIGNATURE_ENTRY2 *e, BOOL pixel_output, UINT *index,
				UINT *system_value)
{
	if (pixel_output && e->SystemValue == D3D10_SB_NAME_UNDEFINED) {
		*index = e->Register;
		*system_value = TARGET_SYSTEM_VALUE;
		return "SV_Target";
	}
	if (e->SystemValue > 0 && (UINT)e->SystemValue < ARRAYSIZE(system_names)) {
		*index = 0;
		*system_value = (UINT)e->SystemValue;
		return system_names[e->SystemValue];
	}
	*index = e->Register;
	*system_value = 0;
	return "TEXCOORD";
}

// A signature chunk's bytes: the count, where the elements start, the
// elements, then the names. Returns its length; out may be NULL to measure.
static SIZE_T write_signature(UCHAR *out, const D3D11_1DDIARG_SIGNATURE_ENTRY2 *entries, UINT count,
			      BOOL output, BOOL pixel_output)
{
	SIZE_T names_at = SIGNATURE_HEAD_BYTES + (SIZE_T)count * SIGNATURE_ELEMENT_BYTES;
	SIZE_T at = names_at;
	UINT i;

	if (out) {
		put32(out, count);
		put32(out + 4, SIGNATURE_HEAD_BYTES);
	}
	for (i = 0; i < count; i++) {
		const D3D11_1DDIARG_SIGNATURE_ENTRY2 *e = &entries[i];
		UCHAR *el = out ? out + SIGNATURE_HEAD_BYTES + (SIZE_T)i * SIGNATURE_ELEMENT_BYTES : NULL;
		UINT index, system_value;
		const char *name = element_name(e, pixel_output, &index, &system_value);
		SIZE_T length = text_length(name) + 1;

		if (el) {
			put32(el, (UINT)at);
			put32(el + 4, index);
			put32(el + 8, system_value);
			put32(el + 12, component_type(e));
			put32(el + 16, e->Register);
			el[20] = e->Mask;
			// Inputs: what the shader reads; outputs: what it never
			// writes. The runtime does not say, so all and none.
			el[21] = output ? 0 : e->Mask;
			el[22] = 0;
			el[23] = 0;
			umd_copy(out + at, name, length);
		}
		at += length;
	}
	while (at % 4) {
		if (out)
			out[at] = 0xab;	// padding, as fxc writes it
		at++;
	}
	return at;
}

// A container with only an input signature of elements named as above, for
// the inner device to match an input layout against (umd_shader.c).
void *umd_dxbc_input_signature(const D3D11_1DDIARG_SIGNATURE_ENTRY2 *entries, UINT count, SIZE_T *bytes)
{
	SIZE_T sig = write_signature(NULL, entries, count, FALSE, FALSE);
	SIZE_T total = DXBC_HEAD_BYTES + 4 + CHUNK_HEAD_BYTES + sig;
	UCHAR *out = umd_alloc(total);

	if (!out)
		return NULL;
	put32(out, FOURCC('D', 'X', 'B', 'C'));
	put32(out + 20, 1);
	put32(out + 24, (UINT)total);
	put32(out + 28, 1);
	put32(out + 32, DXBC_HEAD_BYTES + 4);
	put32(out + 36, FOURCC('I', 'S', 'G', 'N'));
	put32(out + 40, (UINT)sig);
	write_signature(out + 44, entries, count, FALSE, FALSE);
	umd_dxbc_checksum(out, total, out + 4);
	*bytes = total;
	return out;
}

// A container for a shader of the given stage. pixel says the output
// signature is a pixel shader's (render targets). Freed with umd_free.
void *umd_dxbc_build(const UINT *code, const D3D11_1DDIARG_STAGE_IO_SIGNATURES *sig, BOOL pixel,
		     SIZE_T *bytes)
{
	UINT in = sig ? sig->NumInputSignatureEntries : 0;
	UINT out_count = sig ? sig->NumOutputSignatureEntries : 0;
	SIZE_T isgn = write_signature(NULL, in ? sig->pInputSignature : NULL, in, FALSE, FALSE);
	SIZE_T osgn = write_signature(NULL, out_count ? sig->pOutputSignature : NULL, out_count, TRUE, pixel);
	SIZE_T program = (SIZE_T)code[1] * sizeof(UINT);
	SIZE_T chunks_at = DXBC_HEAD_BYTES + 3 * 4;
	SIZE_T isgn_at = chunks_at, osgn_at = isgn_at + CHUNK_HEAD_BYTES + isgn;
	SIZE_T program_at = osgn_at + CHUNK_HEAD_BYTES + osgn;
	SIZE_T total = program_at + CHUNK_HEAD_BYTES + program;
	UINT major = (code[0] >> 4) & 0xf;
	UCHAR *out = umd_alloc(total);

	if (!out)
		return NULL;
	put32(out, FOURCC('D', 'X', 'B', 'C'));
	put32(out + 20, 1);
	put32(out + 24, (UINT)total);
	put32(out + 28, 3);
	put32(out + 32, (UINT)isgn_at);
	put32(out + 36, (UINT)osgn_at);
	put32(out + 40, (UINT)program_at);
	put32(out + isgn_at, FOURCC('I', 'S', 'G', 'N'));
	put32(out + isgn_at + 4, (UINT)isgn);
	write_signature(out + isgn_at + CHUNK_HEAD_BYTES, in ? sig->pInputSignature : NULL, in, FALSE, FALSE);
	put32(out + osgn_at, FOURCC('O', 'S', 'G', 'N'));
	put32(out + osgn_at + 4, (UINT)osgn);
	write_signature(out + osgn_at + CHUNK_HEAD_BYTES, out_count ? sig->pOutputSignature : NULL, out_count,
			TRUE, pixel);
	put32(out + program_at, major >= 5 ? FOURCC('S', 'H', 'E', 'X') : FOURCC('S', 'H', 'D', 'R'));
	put32(out + program_at + 4, (UINT)program);
	umd_copy(out + program_at + CHUNK_HEAD_BYTES, code, program);
	umd_dxbc_checksum(out, total, out + 4);
	*bytes = total;
	return out;
}
