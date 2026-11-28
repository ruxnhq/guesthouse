// SPDX-License-Identifier: MIT
//
// Textures over device surfaces, for ruxnwddm's user-mode driver.
//
// When a swap chain's buffer or a shared texture is made, the user-mode
// driver (umd_resource_inner.c in ruxnwddm) asks, in its own thread and
// just before it has DXMT make the texture, that the next texture of that
// size lie over a device surface: RuxnSurfaceNext makes a new surface in
// this process's task (never mapped into the process: only the GPU writes
// it) and exports it, RuxnSurfaceImportNext takes another process's surface
// by its token instead. The next newTexture of that size in this thread is
// then a texture of kind 11 over the surface, as pvgcomp's targets are in
// stage F, and RuxnSurfaceToken hands the surface's token back for the
// kernel allocation. Nothing of the pixels passes through the CPU.

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winioctl.h>
#include <stdbool.h>
#include <string.h>

#include "winemetal.h"
#include "ruxnpvg_ioctl.h"
#include "ruxnwddm_umd.h"
#include "pvg_wire.h"
#include "winemetal_pvg.h"

#define USAGE_TARGET_AND_READ	(WMTTextureUsageRenderTarget | WMTTextureUsageShaderRead)
#define STORAGE_SHARED		0	// MTLStorageModeShared

struct request {
	bool pending;
	bool import;
	UINT width, height;
	ULONG64 token;		// in for an import; out for both
};

static _Thread_local struct request request;

// The surfaces made in this process, by token: a process that opens a
// resource it shared itself gets its own surface again, which the driver
// will not import into the task that made it.
#define MOST_OWN_SURFACES 512
static struct { ULONG64 token; ULONG handle; } own[MOST_OWN_SURFACES];
static unsigned own_count;

// How many textures of this process lie over each surface handle: the
// handle is given back to the driver when the last one goes (runs rn53,
// rn55: none was, and the compositor's imports ran out of handles).
static unsigned handle_refs[RUXNPVG_MAX_SURFACES + 1];

static ULONG own_surface(ULONG64 token)
{
	unsigned i;

	for (i = 0; i < own_count; i++)
		if (own[i].token == token)
			return own[i].handle;
	return 0;
}

__declspec(dllexport) BOOL WINAPI RuxnSurfaceNext(UINT width, UINT height)
{
	if (!wm_open_device())
		return FALSE;
	memset(&request, 0, sizeof(request));
	request.pending = true;
	request.width = width;
	request.height = height;
	return TRUE;
}

__declspec(dllexport) BOOL WINAPI RuxnSurfaceImportNext(ULONG64 token, UINT width, UINT height)
{
	if (!RuxnSurfaceNext(width, height))
		return FALSE;
	request.import = true;
	request.token = token;
	return TRUE;
}

// The token of the surface the last texture asked for lies over, 0 if none
// was made; asking takes it.
__declspec(dllexport) ULONG64 WINAPI RuxnSurfaceToken(void)
{
	ULONG64 token = request.pending ? 0 : request.token;

	memset(&request, 0, sizeof(request));
	return token;
}

// Whether the texture DXMT just made in this thread lies over a surface
// imported from another process; asking takes the answer. Such a texture
// already holds the maker's pixels, and DXMT, which clears every new
// texture made with no data, must leave it as it is (run rn61: the
// compositor's opens cleared the first frame of every flip-model window,
// and what a window drew once and never again stayed black).
__declspec(dllexport) BOOL WINAPI RuxnSurfaceTakeImported(void)
{
	BOOL imported = !request.pending && request.import && request.token;

	if (imported)
		memset(&request, 0, sizeof(request));
	return imported;
}

// A surface is four bytes a pixel. A texture of one byte a pixel (A8, R8:
// DirectComposition's glyph masks, which every process shares with the
// compositor) lies over a surface a quarter as wide, read with the
// surface's own row length (helper scene surface-r8: an R8 texture four
// times as wide over a 640-pixel surface covered every byte of it).
static bool one_byte(enum WMTPixelFormat format)
{
	return format == WMTPixelFormatA8Unorm || format == WMTPixelFormatR8Unorm;
}

// A surface is a multiple of 4 pixels wide (ruxnwddm_umd.h): its rows are
// then a multiple of 16 bytes apart, which the device needs for a texture
// over it. The texture keeps its own width.
static UINT surface_width(const struct WMTTextureInfo *info)
{
	return one_byte(info->pixel_format) ? RUXNWDDM_SURFACE_WIDTH_ONE_BYTE((UINT)info->width) :
					      RUXNWDDM_SURFACE_WIDTH((UINT)info->width);
}

bool wm_surface_wanted(const struct WMTTextureInfo *info)
{
	return request.pending && info->type == WMTTextureType2D && info->width == request.width &&
	       info->height == request.height &&
	       (info->pixel_format == WMTPixelFormatBGRA8Unorm ||
		info->pixel_format == WMTPixelFormatBGRA8Unorm_sRGB || one_byte(info->pixel_format) ||
		info->pixel_format == WMTPixelFormatRGBA8Unorm || info->pixel_format == WMTPixelFormatRGBA8Unorm_sRGB);
}

// A new surface of this process's, `width` pixels wide, exported: its
// handle and token.
static bool new_surface(UINT width, ULONG *handle, ULONG64 *token)
{
	struct ruxnpvg_bo_create bo;
	struct ruxnpvg_surface_export exp;

	memset(&bo, 0, sizeof(bo));
	bo.flags = RUXNPVG_BO_SURFACE | RUXNPVG_BO_NO_CPU;
	bo.width = width;
	bo.height = request.height;
	if (!wm_control("surface_create", RUXNPVG_IOCTL_BO_CREATE, &bo, sizeof(bo), &bo, sizeof(bo)))
		return false;
	memset(&exp, 0, sizeof(exp));
	exp.handle = bo.handle;
	if (!wm_control("surface_export", RUXNPVG_IOCTL_SURFACE_EXPORT, &exp, sizeof(exp), &exp, sizeof(exp)))
		return false;
	*handle = bo.handle;
	*token = exp.token;
	if (own_count < MOST_OWN_SURFACES) {
		own[own_count].token = exp.token;
		own[own_count].handle = bo.handle;
		own_count++;
	}
	return true;
}

static bool imported_surface(ULONG64 token, UINT width, ULONG *handle)
{
	struct ruxnpvg_surface_import imp;

	*handle = own_surface(token);
	if (*handle)
		return true;
	memset(&imp, 0, sizeof(imp));
	imp.token = token;
	if (!wm_control("surface_import", RUXNPVG_IOCTL_SURFACE_IMPORT, &imp, sizeof(imp), &imp, sizeof(imp)))
		return false;
	if (imp.width != width || imp.height != request.height) {
		pvg_log("FAILED: surface_import: %ux%u, not %ux%u", imp.width, imp.height, width, request.height);
		return false;
	}
	*handle = imp.handle;
	return true;
}

// A view of the whole texture over a surface, placed: its number, and its
// resource id in *id. 0 if either failed; the texture is still drawn into,
// only shaders cannot reach it by number. Under winemetal's lock.
static uint32_t surface_view(uint32_t texture, uint32_t format, uint64_t *id)
{
	UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	uint32_t number = wm_take_number();
	size_t length;

	*id = 0;
	if (!number)
		return 0;
	length = pvg_wire_texture_view(description, sizeof(description), number, texture, format);
	if (!length || !wm_create_bare_object(number, RUXNPVG_OBJECT_TEXTURE_VIEW, 0, description, (ULONG)length))
		return 0;
	if (!wm_place(number, WM_PLACE_VIEW, texture, id)) {
		pvg_log("FAILED: the placement of view %u of surface texture %u", number, texture);
		return number;
	}
	return number;
}

// The texture's object number, or 0; the view's number and resource id
// (both 0 if it has none). Under winemetal's lock.
uint32_t wm_surface_texture(const struct WMTTextureInfo *info, uint32_t *view, uint64_t *resource_id,
			    ULONG *handle_out)
{
	static UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	struct pvg_wire_texture texture;
	ULONG handle = 0;
	ULONG64 token = request.token;
	uint32_t number;
	size_t length;
	bool ok;

	*view = 0;
	*resource_id = 0;
	*handle_out = 0;
	// The request's token is cleared before anything can fail, and the
	// import is handed the one it asked for (run rn8: it read the cleared
	// one, and every import went to the kernel as token 0).
	request.pending = false;
	request.token = 0;
	ok = request.import ? imported_surface(token, surface_width(info), &handle) :
			      new_surface(surface_width(info), &handle, &token);
	if (!ok)
		return 0;
	pvg_wire_texture_init(&texture);
	texture.format = info->pixel_format;
	texture.width = info->width;
	texture.height = info->height;
	texture.usage = USAGE_TARGET_AND_READ;
	texture.storage = STORAGE_SHARED;
	number = wm_take_number();
	length = pvg_wire_surface_texture(description, sizeof(description), number, &texture, 0, 0);
	if (!number || !length ||
	    !wm_create_bare_object(number, RUXNPVG_OBJECT_SURFACE_TEXTURE, handle,
				   description + PVG_WIRE_HEAD_SURFACE_TEXTURE,
				   (ULONG)(length - PVG_WIRE_HEAD_SURFACE_TEXTURE))) {
		// The surface handle is given back, as when the texture goes (run
		// rn74: each failed texture kept one, and a resized window's
		// retries used them all up).
		if (handle && handle <= RUXNPVG_MAX_SURFACES) {
			handle_refs[handle]++;
			wm_surface_release(handle);
		}
		return 0;
	}
	request.token = token;
	*handle_out = handle;
	if (handle <= RUXNPVG_MAX_SURFACES)
		handle_refs[handle]++;
	*view = surface_view(number, info->pixel_format, resource_id);
	pvg_log("surface texture %u: %ux%u, surface %lu, token %016llx (%s), view %u, resource id %llu", number,
		info->width, info->height, handle, (unsigned long long)token,
		request.import ? "imported" : "made here", *view, (unsigned long long)*resource_id);
	return number;
}

// A texture over a surface has gone: its surface handle is given back when
// no other texture of this process lies over it, and a surface this
// process made is no longer offered to its own opens. Under winemetal's
// lock; the texture's objects are already deleted (wm_objects.c).
void wm_surface_release(ULONG handle)
{
	struct ruxnpvg_surface_handle h;
	unsigned i;

	if (!handle || handle > RUXNPVG_MAX_SURFACES || !handle_refs[handle] || --handle_refs[handle])
		return;
	for (i = 0; i < own_count; i++) {
		if (own[i].handle == handle) {
			own[i] = own[--own_count];
			break;
		}
	}
	memset(&h, 0, sizeof(h));
	h.handle = handle;
	wm_control("surface_destroy", RUXNPVG_IOCTL_SURFACE_DESTROY, &h, sizeof(h), NULL, 0);
}
