/* SPDX-License-Identifier: MIT */
/* What winemetal_pvg.c shares with the stubs gen_stubs.py writes. */
#ifndef WINEMETAL_PVG_H
#define WINEMETAL_PVG_H

#include <stdbool.h>

void pvg_log(const char *format, ...);
// wm_shader.c: whether the triangle's stand-in answers for the shader
// compiler (RUXN_SHADERS=standin), so that only its two libraries are taken.
bool wm_shader_standin(void);
// Whether the bytes are one of the stand-in's two libraries.
bool wm_shader_standin_library(const void *data, unsigned long long length);

// Shared by winemetal_pvg.c and wm_surface.c: the task controls, the
// object numbers, and objects over surfaces.
#include <windows.h>
#include <stdint.h>
BOOL wm_control(const char *what, DWORD code, const void *in, DWORD in_size, void *out, DWORD out_size);
bool wm_open_device(void);
uint32_t wm_take_number(void);
// An object with no memory of its own: a texture over a surface (kind 11,
// handle the surface's) or a view of one (kind 8, handle 0).
bool wm_create_bare_object(uint32_t number, ULONG kind, ULONG handle, const void *tail, ULONG tail_size);
// wm_place.c: the host's GPU address or resource id of an object, made
// known by listing it (buffers and textures), by binding it (samplers), or
// by binding it with its base listed (views; base is the base texture).
enum wm_place_how { WM_PLACE_LISTED, WM_PLACE_SAMPLER, WM_PLACE_VIEW };
bool wm_place(uint32_t number, enum wm_place_how how, uint32_t base, uint64_t *word);
// A refused placement marks the object (and a view's base) as one the
// device never made: the first such number a list names, or 0; and a
// number taken again is no longer marked.
uint32_t wm_broken_named(const uint32_t *ids, unsigned count);
void wm_broken_clear(uint32_t number);

// wm_refused.c: a refused (or asked for) submit's resources and segments,
// kept on disk as WHAT-PID-N.bin.
void wm_refused_dump(const char *what, const uint32_t *ids, const uint32_t *flags, unsigned resources,
		     const UCHAR *const *segments, const uint32_t *sizes, unsigned count);
// The last few submits that went through, kept while the lab log is on and
// written out before the first refused one.
void wm_recent_keep(const uint32_t *ids, const uint32_t *flags, unsigned resources,
		    const UCHAR *const *segments, const uint32_t *sizes, unsigned count);
// Whether this process's log goes anywhere (a file, or the kernel's log).
bool wm_lab_log_on(void);
// Whether its lines go to the kernel driver's log (a sandboxed process).
bool wm_log_in_kernel(void);

// wm_surface.c: textures over device surfaces, for the user-mode driver.
// Gives the texture's number, the number of the view that gives it a
// resource id (0 if none could be had) and that id.
struct WMTTextureInfo;
bool wm_surface_wanted(const struct WMTTextureInfo *info);
uint32_t wm_surface_texture(const struct WMTTextureInfo *info, uint32_t *view, uint64_t *resource_id,
			    ULONG *handle);
void wm_surface_release(ULONG handle);

#endif
