// SPDX-License-Identifier: MIT
//
// The shader compiler's functions that DXMT's Direct3D side takes from
// winemetal.dll (airconv_public.h): SM50Initialize, SM50Compile and their
// kin.
//
// By default they are DXMT's airconv itself, built for Windows on arm64
// with LLVM 15 from source and linked in (airconv-build.sh): DXBC in, a
// library for the device out, inside the guest, with no compiler of the
// host vendor's anywhere. airconv's own symbols are renamed airconv_SM50*
// at build time (build.sh), so these exports can stand in front of them.
//
// With RUXN_SHADERS=standin in the environment they are the first slice's
// stand-in instead: only the triangle's two DXBC shaders, matched byte for
// byte, answered with the libraries glslang and our airc made
// (libraries.h). It stays so that a run can draw exactly what run rk1 drew
// before trying airconv's libraries on the device.
//
// With RUXN_SHADER_DUMP set to a folder, every shader airconv compiles is
// written there, its DXBC and the library, named by the DXBC's hash, so a
// run's shaders can be looked at on the Mac afterwards.

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define WIN_EXPORT
#include "airconv_public.h"
#include "winemetal_pvg.h"

#include "triangle_dxbc.h"
// Built without the stand-in (build.sh with RUXN_STANDIN=0), there are no
// libraries to answer the triangle with, and RUXN_SHADERS=standin refuses.
#ifndef RUXN_NO_STANDIN
#include "libraries.h"
#else
static const unsigned char tri_vert_metallib[1], tri_frag_metallib[1];
#endif

#define SHADER_API __declspec(dllexport)

// airconv's functions, under the names build.sh gives them.
int airconv_SM50Initialize(const void *, size_t, sm50_shader_t *, struct MTL_SHADER_REFLECTION *,
			   sm50_error_t *);
void airconv_SM50Destroy(sm50_shader_t);
int airconv_SM50Compile(sm50_shader_t, struct SM50_SHADER_COMPILATION_ARGUMENT_DATA *, const char *,
			sm50_bitcode_t *, sm50_error_t *);
void airconv_SM50GetCompiledBitcode(sm50_bitcode_t, struct SM50_COMPILED_BITCODE *);
void airconv_SM50DestroyBitcode(sm50_bitcode_t);
size_t airconv_SM50GetErrorMessage(sm50_error_t, char *, size_t);
void airconv_SM50FreeError(sm50_error_t);
int airconv_SM50CompileTessellationPipelineHull(sm50_shader_t, sm50_shader_t,
						struct SM50_SHADER_COMPILATION_ARGUMENT_DATA *,
						const char *, sm50_bitcode_t *, sm50_error_t *);
int airconv_SM50CompileTessellationPipelineDomain(sm50_shader_t, sm50_shader_t,
						  struct SM50_SHADER_COMPILATION_ARGUMENT_DATA *,
						  const char *, sm50_bitcode_t *, sm50_error_t *);
int airconv_SM50CompileGeometryPipelineVertex(sm50_shader_t, sm50_shader_t,
					      struct SM50_SHADER_COMPILATION_ARGUMENT_DATA *,
					      const char *, sm50_bitcode_t *, sm50_error_t *);
int airconv_SM50CompileGeometryPipelineGeometry(sm50_shader_t, sm50_shader_t,
						struct SM50_SHADER_COMPILATION_ARGUMENT_DATA *,
						const char *, sm50_bitcode_t *, sm50_error_t *);
void airconv_SM50GetArgumentsInfo(sm50_shader_t, struct MTL_SM50_SHADER_ARGUMENT *,
				  struct MTL_SM50_SHADER_ARGUMENT *);

static INIT_ONCE mode_once = INIT_ONCE_STATIC_INIT;
static bool standin;
static char dump_dir[MAX_PATH];

static BOOL CALLBACK read_mode(PINIT_ONCE once, PVOID param, PVOID *context)
{
	char value[16];

	(void)once, (void)param, (void)context;
	standin = GetEnvironmentVariableA("RUXN_SHADERS", value, sizeof(value)) &&
		  !strcmp(value, "standin");
	if (!GetEnvironmentVariableA("RUXN_SHADER_DUMP", dump_dir, sizeof(dump_dir) - 40))
		dump_dir[0] = 0;
	pvg_log("shaders: %s", standin ? "the triangle's stand-in" : "airconv, in the guest");
	return TRUE;
}

bool wm_shader_standin(void)
{
	InitOnceExecuteOnce(&mode_once, read_mode, NULL, NULL);
	return standin;
}

static uint64_t fnv1a(const void *data, size_t length)
{
	const unsigned char *p = data;
	uint64_t h = 0xcbf29ce484222325ull;

	while (length--)
		h = (h ^ *p++) * 0x100000001b3ull;
	return h;
}

static void dump(uint64_t hash, const char *suffix, const void *data, size_t size)
{
	char path[MAX_PATH];
	FILE *f;

	if (!dump_dir[0])
		return;
	snprintf(path, sizeof(path), "%s\\%016llx%s", dump_dir, (unsigned long long)hash, suffix);
	f = fopen(path, "wb");
	if (!f)
		return;
	fwrite(data, 1, size, f);
	fclose(f);
}

// ---------------------------------------------------------------------
// The stand-in: the triangle's two shaders, and errors as plain strings.

struct standin_shader {
	int which;	// 0 vertex, 1 pixel
	struct SM50_COMPILED_BITCODE bitcode;
};

static struct standin_shader standin_shaders[2] = {
	{ 0, { (sm50_ptr64_t)tri_vert_metallib, sizeof(tri_vert_metallib) } },
	{ 1, { (sm50_ptr64_t)tri_frag_metallib, sizeof(tri_frag_metallib) } },
};

static bool same_bytes(const void *a, uint64_t a_length, const void *b, uint64_t b_length)
{
	return a_length == b_length && !memcmp(a, b, a_length);
}

bool wm_shader_standin_library(const void *data, unsigned long long length)
{
	return same_bytes(data, length, tri_vert_metallib, sizeof(tri_vert_metallib)) ||
	       same_bytes(data, length, tri_frag_metallib, sizeof(tri_frag_metallib));
}

static sm50_error_t standin_error(const char *text)
{
	pvg_log("FAILED: %s", text);
	return (sm50_error_t)_strdup(text);
}

static int standin_initialize(const void *code, size_t size, sm50_shader_t *shader,
			      struct MTL_SHADER_REFLECTION *refl, sm50_error_t *error)
{
	int which = -1;
	char text[128];

#ifdef RUXN_NO_STANDIN
	(void)standin_shaders;
	if (error)
		*error = standin_error("SM50Initialize: this winemetal.dll was built without the stand-in");
	return 1;
#endif
	if (same_bytes(code, size, triangle_vs_dxbc, sizeof(triangle_vs_dxbc)))
		which = 0;
	else if (same_bytes(code, size, triangle_ps_dxbc, sizeof(triangle_ps_dxbc)))
		which = 1;
	if (which < 0) {
		snprintf(text, sizeof(text), "SM50Initialize: a shader of %zu bytes (FNV-1a %016llx) that is "
			 "not one of the triangle's", size, (unsigned long long)fnv1a(code, size));
		if (error)
			*error = standin_error(text);
		return 1;
	}
	// As airconv reflects these two: no constant buffers, samplers or
	// views, two output (or input) registers, one render target.
	if (refl) {
		memset(refl, 0, sizeof(*refl));
		refl->ConstanttBufferTableBindIndex = ~0u;
		refl->ArgumentBufferBindIndex = ~0u;
		refl->NumOutputElement = 2;
		if (which == 1)
			refl->PixelShader.ValidRenderTargets = 1;
	}
	*shader = (sm50_shader_t)&standin_shaders[which];
	pvg_log("SM50Initialize: the triangle's %s shader (stand-in)", which ? "pixel" : "vertex");
	return 0;
}

// ---------------------------------------------------------------------
// The exports.

SHADER_API int SM50Initialize(const void *pBytecode, size_t BytecodeSize, sm50_shader_t *ppShader,
			      struct MTL_SHADER_REFLECTION *pRefl, sm50_error_t *ppError)
{
	uint64_t hash = fnv1a(pBytecode, BytecodeSize);
	int failed;

	if (wm_shader_standin())
		return standin_initialize(pBytecode, BytecodeSize, ppShader, pRefl, ppError);
	failed = airconv_SM50Initialize(pBytecode, BytecodeSize, ppShader, pRefl, ppError);
	pvg_log("SM50Initialize: %zu bytes of DXBC, FNV-1a %016llx: %s", BytecodeSize,
		(unsigned long long)hash, failed ? "refused" : "taken");
	dump(hash, ".dxbc", pBytecode, BytecodeSize);
	return failed;
}

SHADER_API void SM50Destroy(sm50_shader_t pShader)
{
	if (!wm_shader_standin())
		airconv_SM50Destroy(pShader);
}

SHADER_API int SM50Compile(sm50_shader_t pShader, struct SM50_SHADER_COMPILATION_ARGUMENT_DATA *pArgs,
			   const char *FunctionName, sm50_bitcode_t *ppBitcode, sm50_error_t *ppError)
{
	struct SM50_COMPILED_BITCODE out;
	int failed;

	if (wm_shader_standin()) {
		*ppBitcode = (sm50_bitcode_t)&((struct standin_shader *)pShader)->bitcode;
		return 0;
	}
	failed = airconv_SM50Compile(pShader, pArgs, FunctionName, ppBitcode, ppError);
	if (failed) {
		pvg_log("FAILED: SM50Compile %s", FunctionName);
		return failed;
	}
	airconv_SM50GetCompiledBitcode(*ppBitcode, &out);
	pvg_log("SM50Compile %s: a library of %llu bytes, FNV-1a %016llx", FunctionName,
		(unsigned long long)out.Size,
		(unsigned long long)fnv1a((const void *)out.Data, (size_t)out.Size));
	dump(fnv1a((const void *)out.Data, (size_t)out.Size), ".metallib", (const void *)out.Data,
	     (size_t)out.Size);
	return 0;
}

SHADER_API void SM50GetCompiledBitcode(sm50_bitcode_t pBitcode, struct SM50_COMPILED_BITCODE *pData)
{
	if (wm_shader_standin())
		*pData = *(struct SM50_COMPILED_BITCODE *)pBitcode;
	else
		airconv_SM50GetCompiledBitcode(pBitcode, pData);
}

SHADER_API void SM50DestroyBitcode(sm50_bitcode_t pBitcode)
{
	if (!wm_shader_standin())
		airconv_SM50DestroyBitcode(pBitcode);
}

SHADER_API size_t SM50GetErrorMessage(sm50_error_t pError, char *pBuffer, size_t BufferSize)
{
	size_t n;

	if (!wm_shader_standin())
		return airconv_SM50GetErrorMessage(pError, pBuffer, BufferSize);
	if (!pError || !BufferSize)
		return 0;
	n = strlen((const char *)pError);
	if (n > BufferSize)
		n = BufferSize;
	memcpy(pBuffer, (const char *)pError, n);
	return n;
}

SHADER_API void SM50FreeError(sm50_error_t pError)
{
	if (wm_shader_standin())
		free((void *)pError);
	else
		airconv_SM50FreeError(pError);
}

// Geometry and tessellation go to airconv as they are; the device has no
// mesh stage, so a pipeline that needs one fails when it is made.
static int standin_none(const char *what, sm50_error_t *error)
{
	char text[96];

	snprintf(text, sizeof(text), "%s: not in the stand-in", what);
	if (error)
		*error = standin_error(text);
	return 1;
}

#define PIPELINE_SHADER(name)                                                                     \
	SHADER_API int name(sm50_shader_t a, sm50_shader_t b,                                       \
			    struct SM50_SHADER_COMPILATION_ARGUMENT_DATA *args, const char *fn,     \
			    sm50_bitcode_t *out, sm50_error_t *error)                               \
	{                                                                                           \
		if (wm_shader_standin())                                                            \
			return standin_none(#name, error);                                          \
		pvg_log("%s %s", #name, fn);                                                        \
		return airconv_##name(a, b, args, fn, out, error);                                  \
	}

PIPELINE_SHADER(SM50CompileTessellationPipelineHull)
PIPELINE_SHADER(SM50CompileTessellationPipelineDomain)
PIPELINE_SHADER(SM50CompileGeometryPipelineVertex)
PIPELINE_SHADER(SM50CompileGeometryPipelineGeometry)

SHADER_API void SM50GetArgumentsInfo(sm50_shader_t pShader, struct MTL_SM50_SHADER_ARGUMENT *pConstantBuffers,
				     struct MTL_SM50_SHADER_ARGUMENT *pArguments)
{
	// The stand-in's two shaders have no arguments: nothing to write.
	if (!wm_shader_standin())
		airconv_SM50GetArgumentsInfo(pShader, pConstantBuffers, pArguments);
}
