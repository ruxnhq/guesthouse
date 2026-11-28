/* SPDX-License-Identifier: MIT */
/*
 * airconvtest: convert one DXBC shader into a Metal library with DXMT's
 * airconv, through its exported C functions, the way DXMT's d3d11 side does
 * for a plain draw (src/d3d11/d3d11_shader.cpp, d3d11_pipeline.cpp at
 * faa73e4):
 *
 *   SM50Initialize(bytecode, size, &shader, &reflection, &err)
 *   vertex shader: GS_PASS_THROUGH { DataEncoded = ~0u (no geometry shader),
 *                  RasterizationDisabled = false } -> COMMON
 *   pixel shader:  PSO_PIXEL_SHADER { sample_mask = 0xffffffff, no dual source
 *                  blending, disable_depth_output = 1 (no depth buffer),
 *                  unorm_output_reg_mask = 1 (RT0 is an 8-bit unorm format) }
 *                  -> COMMON
 *   COMMON { metal_version = 320, flags = 0 }
 *   SM50Compile(shader, args, function name, &bitcode, &err)
 *   SM50GetCompiledBitcode(bitcode, &data) -> data.Data, data.Size
 *
 * No input layout is passed (the triangle uses SV_VertexID only).
 * The shader kind comes from the DXBC program header (version token).
 *
 * Usage: airconvtest input.dxbc output.metallib [function_name]
 * Builds for macOS (native) and for aarch64-w64-mingw32; see airconv-build.sh.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "airconv_public.h"

static void *read_file(const char *path, size_t *size) {
  FILE *f = fopen(path, "rb");
  if (!f) return NULL;
  fseek(f, 0, SEEK_END);
  long n = ftell(f);
  fseek(f, 0, SEEK_SET);
  void *buf = n > 0 ? malloc((size_t)n) : NULL;
  if (buf && fread(buf, 1, (size_t)n, f) != (size_t)n) { free(buf); buf = NULL; }
  fclose(f);
  *size = (size_t)n;
  return buf;
}

static void print_error(const char *what, sm50_error_t err) {
  char msg[1024];
  size_t n = SM50GetErrorMessage(err, msg, sizeof(msg) - 1);
  msg[n < sizeof(msg) ? n : sizeof(msg) - 1] = 0;
  fprintf(stderr, "%s failed: %s\n", what, msg);
  SM50FreeError(err);
}

/* Program type from the SHDR/SHEX chunk's version token: 0 pixel, 1 vertex. */
static int program_type(const unsigned char *dxbc, size_t size) {
  if (size < 32 || memcmp(dxbc, "DXBC", 4)) return -1;
  unsigned count = *(const unsigned *)(dxbc + 28);
  for (unsigned i = 0; i < count && 32 + 4 * i + 4 <= size; i++) {
    unsigned off = *(const unsigned *)(dxbc + 32 + 4 * i);
    if (off + 12 > size) continue;
    if (!memcmp(dxbc + off, "SHDR", 4) || !memcmp(dxbc + off, "SHEX", 4))
      return (int)(*(const unsigned *)(dxbc + off + 8) >> 16);
  }
  return -1;
}

int main(int argc, char **argv) {
  if (argc < 3) {
    fprintf(stderr, "usage: %s input.dxbc output.metallib [function_name]\n", argv[0]);
    return 2;
  }
  size_t size = 0;
  void *dxbc = read_file(argv[1], &size);
  if (!dxbc) { fprintf(stderr, "cannot read %s\n", argv[1]); return 1; }
  int type = program_type(dxbc, size);
  const char *name = argc > 3 ? argv[3] : (type == 0 ? "ps_main" : "vs_main");

  sm50_shader_t shader;
  sm50_error_t err;
  struct MTL_SHADER_REFLECTION refl;
  if (SM50Initialize(dxbc, size, &shader, &refl, &err)) { print_error("SM50Initialize", err); return 1; }

  struct SM50_SHADER_COMMON_DATA common = {0};
  common.type = SM50_SHADER_COMMON;
  common.metal_version = SM50_SHADER_METAL_320;
  common.flags = (enum SM50_SHADER_FLAG)0;
  common.next = NULL;

  struct SM50_SHADER_GS_PASS_THROUGH_DATA vs = {0};
  vs.type = SM50_SHADER_GS_PASS_THROUGH;
  vs.DataEncoded = ~0u;
  vs.RasterizationDisabled = false;
  vs.next = &common;

  struct SM50_SHADER_PSO_PIXEL_SHADER_DATA ps = {0};
  ps.type = SM50_SHADER_PSO_PIXEL_SHADER;
  ps.sample_mask = 0xffffffffu;
  ps.dual_source_blending = false;
  ps.disable_depth_output = true;
  ps.unorm_output_reg_mask = 1;
  ps.next = &common;

  void *args = type == 0 ? (void *)&ps : type == 1 ? (void *)&vs : (void *)&common;
  sm50_bitcode_t bitcode;
  if (SM50Compile(shader, (struct SM50_SHADER_COMPILATION_ARGUMENT_DATA *)args, name, &bitcode, &err)) {
    print_error("SM50Compile", err);
    return 1;
  }
  struct SM50_COMPILED_BITCODE data;
  SM50GetCompiledBitcode(bitcode, &data);

  FILE *out = fopen(argv[2], "wb");
  if (!out || fwrite((const void *)data.Data, 1, data.Size, out) != data.Size) {
    fprintf(stderr, "cannot write %s\n", argv[2]);
    return 1;
  }
  fclose(out);
  printf("%s: %s shader, function %s, %llu bytes\n", argv[2],
         type == 0 ? "pixel" : type == 1 ? "vertex" : "other", name,
         (unsigned long long)data.Size);
  SM50DestroyBitcode(bitcode);
  SM50Destroy(shader);
  free(dxbc);
  return 0;
}
